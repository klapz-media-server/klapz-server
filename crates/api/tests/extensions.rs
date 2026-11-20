//! Connecting users through an extension whose clients sign in with a password — Jellyfin —
//! beside Plex's, whose clients are handed the connection's token.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use open_plex_api::{build_schema, ApiContext, ApiSchema, GlobalId};
use open_plex_core::access::{token_digest, AccessRepository, Extension, NewUser, ServerRole};
use open_plex_core::testing::{InMemoryAccess, InMemoryLibrary, InMemoryMetadata};
use open_plex_core::{Principal, UserId};
use serde_json::{json, Value};

const PASSWORD: &str = "correct horse";

struct Fixture {
    schema: ApiSchema,
    access: Arc<InMemoryAccess>,
    sessions: Arc<open_plex_api::Sessions>,
    alice: UserId,
}

/// `alice`, an active user with a password and no connection yet.
async fn fixture() -> Fixture {
    let access = Arc::new(InMemoryAccess::default());
    let hash = open_plex_core::account::hash_password(PASSWORD).unwrap();
    let alice = access
        .create_user(
            NewUser {
                username: "alice",
                email: Some("alice@example.org"),
                role: ServerRole::User,
                active: true,
                password_hash: Some(&hash),
            },
            0,
        )
        .await
        .unwrap()
        .id;
    let schema = build_schema(ApiContext {
        capabilities: open_plex_core::ServerCapabilities::new(
            open_plex_core::ServerIdentity::new(
                "0000000000000000000000000000000000000000",
                "0.1.0",
            ),
            0,
        ),
        library: Arc::new(InMemoryLibrary::new(Vec::new())),
        metadata: Arc::new(InMemoryMetadata::default()),
        playback: Arc::new(open_plex_core::testing::InMemoryPlayback::default()),
        play_queues: Arc::new(open_plex_core::testing::InMemoryPlayQueues::default()),
        playlists: Arc::new(open_plex_core::testing::InMemoryPlaylists::default()),
        collections: Arc::new(open_plex_core::testing::InMemoryCollections::default()),
        access: Arc::clone(&access) as Arc<dyn AccessRepository>,
        directory: Arc::new(open_plex_core::testing::StubDirectory::default()),
        watch: Arc::new(open_plex_core::testing::InMemoryWatch::default()),
        streams: Arc::new(open_plex_core::testing::StubStreams::default()),
        session_locks: Arc::default(),
        scans: Arc::new(open_plex_core::testing::RecordedScans::default()),
    });
    let sessions = Arc::new(open_plex_api::Sessions::new(
        Arc::new(open_plex_core::account::SessionKey::generate()),
        true,
    ));
    Fixture {
        schema,
        access,
        sessions,
        alice,
    }
}

impl Fixture {
    async fn run(&self, request: async_graphql::Request) -> Value {
        let response = self
            .schema
            .execute(request.data(Arc::clone(&self.sessions)))
            .await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        response.data.into_json().unwrap()
    }

    async fn as_owner(&self, query: &str, variables: Value) -> Value {
        self.run(
            async_graphql::Request::new(query)
                .data(open_plex_core::testing::owner())
                .variables(async_graphql::Variables::from_json(variables)),
        )
        .await
    }

    async fn connect(&self, extension: &str) -> Value {
        self.as_owner(
            "mutation($user: ID!, $extension: ExtensionType!) {
                connectExtension(userId: $user, extension: $extension) {
                    token userExtension { extension enabled }
                }
            }",
            json!({ "user": GlobalId::encode("User", self.alice.get()), "extension": extension }),
        )
        .await
    }

    async fn disable(&self, extension: &str) {
        self.as_owner(
            "mutation($user: ID!, $extension: ExtensionType!) {
                disableExtension(userId: $user, extension: $extension)
            }",
            json!({ "user": GlobalId::encode("User", self.alice.get()), "extension": extension }),
        )
        .await;
    }

    /// Signs `alice` in from a device, as a client of `extension`.
    async fn sign_in(&self, extension: &str, password: &str, device: &str) -> Value {
        self.run(
            async_graphql::Request::new(
                "mutation($input: ExtensionSignInInput!) {
                    signInThroughExtension(input: $input) {
                        clientMutationId token user { username }
                        userErrors { field code }
                    }
                }",
            )
            .variables(async_graphql::Variables::from_json(json!({ "input": {
                "clientMutationId": "m1",
                "extension": extension,
                "login": "Alice",
                "password": password,
                "deviceId": device,
                "deviceName": "Chrome",
                "client": "Jellyfin Web",
                "clientVersion": "10.11.0",
            }}))),
        )
        .await["signInThroughExtension"]
            .clone()
    }

    async fn opens(&self, extension: Extension, token: &str) -> Option<UserId> {
        self.access
            .grant(extension, &token_digest(token))
            .await
            .unwrap()
            .map(|grant| grant.user.id)
    }

    fn as_alice(&self) -> Principal {
        Principal {
            user: self.alice,
            name: "alice".to_owned(),
            role: ServerRole::User,
            libraries: open_plex_core::access::LibraryAccess::Only(std::collections::HashMap::new()),
            server_identifier: None,
        }
    }

    async fn sign_out(&self, as_user: Principal, token: &str) -> bool {
        self.run(
            async_graphql::Request::new(
                "mutation($token: String!) {
                    signOutOfExtension(extension: JELLYFIN, token: $token)
                }",
            )
            .data(as_user)
            .variables(async_graphql::Variables::from_json(
                json!({ "token": token }),
            )),
        )
        .await["signOutOfExtension"]
            .as_bool()
            .unwrap()
    }
}

fn token_of(payload: &Value) -> String {
    payload["token"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn a_user_connected_through_jellyfin_signs_in_and_its_device_token_opens_its_access() {
    let fixture = fixture().await;
    let connected = fixture.connect("JELLYFIN").await;
    assert_eq!(
        connected["connectExtension"]["userExtension"],
        json!({ "extension": "JELLYFIN", "enabled": true })
    );

    let signed_in = fixture.sign_in("JELLYFIN", PASSWORD, "device-1").await;
    assert_eq!(signed_in["clientMutationId"], "m1");
    assert_eq!(signed_in["user"], json!({ "username": "alice" }));
    assert_eq!(signed_in["userErrors"], json!([]));
    let token = token_of(&signed_in);
    assert_eq!(token.len(), 64);
    assert_eq!(
        fixture.opens(Extension::Jellyfin, &token).await,
        Some(fixture.alice)
    );
    // A Jellyfin token is no Plex token.
    assert_eq!(fixture.opens(Extension::Plex, &token).await, None);
}

#[tokio::test]
async fn the_connection_token_is_a_jellyfin_key_as_well() {
    let fixture = fixture().await;
    let connected = fixture.connect("JELLYFIN").await;
    let key = connected["connectExtension"]["token"].as_str().unwrap();
    assert_eq!(
        fixture.opens(Extension::Jellyfin, key).await,
        Some(fixture.alice)
    );
}

#[tokio::test]
async fn refuses_a_user_without_a_jellyfin_extension() {
    let fixture = fixture().await;
    // A Plex connection opens nothing on Jellyfin.
    fixture.connect("PLEX").await;
    let refused = fixture.sign_in("JELLYFIN", PASSWORD, "device-1").await;
    assert!(refused["token"].is_null());
    assert!(refused["user"].is_null());
    assert_eq!(
        refused["userErrors"],
        json!([{ "field": ["input", "extension"], "code": "NOT_CONNECTED" }])
    );
}

#[tokio::test]
async fn a_wrong_password_says_nothing_of_the_connection() {
    let fixture = fixture().await;
    for connect in [false, true] {
        if connect {
            fixture.connect("JELLYFIN").await;
        }
        let refused = fixture
            .sign_in("JELLYFIN", "wrong horse!", "device-1")
            .await;
        assert!(refused["token"].is_null());
        assert_eq!(
            refused["userErrors"][0]["code"], "INVALID_CREDENTIALS",
            "connected: {connect}"
        );
    }
}

#[tokio::test]
async fn a_disabled_extension_refuses_sign_in_and_its_devices_tokens() {
    let fixture = fixture().await;
    fixture.connect("JELLYFIN").await;
    let token = token_of(&fixture.sign_in("JELLYFIN", PASSWORD, "device-1").await);
    fixture.disable("JELLYFIN").await;
    assert_eq!(fixture.opens(Extension::Jellyfin, &token).await, None);
    let refused = fixture.sign_in("JELLYFIN", PASSWORD, "device-1").await;
    assert_eq!(refused["userErrors"][0]["code"], "NOT_CONNECTED");

    // Back on, the device signs in again: its old token stays forgotten.
    fixture.connect("JELLYFIN").await;
    assert_eq!(fixture.opens(Extension::Jellyfin, &token).await, None);
    let again = token_of(&fixture.sign_in("JELLYFIN", PASSWORD, "device-1").await);
    assert_eq!(
        fixture.opens(Extension::Jellyfin, &again).await,
        Some(fixture.alice)
    );
}

#[tokio::test]
async fn an_inactive_user_opens_nothing_through_its_device() {
    let fixture = fixture().await;
    fixture.connect("JELLYFIN").await;
    let token = token_of(&fixture.sign_in("JELLYFIN", PASSWORD, "device-1").await);
    fixture
        .access
        .set_active(fixture.alice, false, 1)
        .await
        .unwrap();
    assert_eq!(fixture.opens(Extension::Jellyfin, &token).await, None);
    let refused = fixture.sign_in("JELLYFIN", PASSWORD, "device-1").await;
    assert_eq!(refused["userErrors"][0]["code"], "INACTIVE");
}

#[tokio::test]
async fn signing_in_again_from_a_device_replaces_its_token_and_leaves_the_others() {
    let fixture = fixture().await;
    fixture.connect("JELLYFIN").await;
    let first = token_of(&fixture.sign_in("JELLYFIN", PASSWORD, "device-1").await);
    let other = token_of(&fixture.sign_in("JELLYFIN", PASSWORD, "device-2").await);
    let second = token_of(&fixture.sign_in("JELLYFIN", PASSWORD, "device-1").await);
    assert_ne!(first, second);
    assert_eq!(fixture.opens(Extension::Jellyfin, &first).await, None);
    for token in [&second, &other] {
        assert_eq!(
            fixture.opens(Extension::Jellyfin, token).await,
            Some(fixture.alice)
        );
    }
}

#[tokio::test]
async fn signing_out_forgets_the_callers_own_device_only() {
    let fixture = fixture().await;
    fixture.connect("JELLYFIN").await;
    let token = token_of(&fixture.sign_in("JELLYFIN", PASSWORD, "device-1").await);

    let mut stranger = fixture.as_alice();
    stranger.user = UserId::new(99);
    assert!(!fixture.sign_out(stranger, &token).await);
    assert_eq!(
        fixture.opens(Extension::Jellyfin, &token).await,
        Some(fixture.alice)
    );

    assert!(fixture.sign_out(fixture.as_alice(), &token).await);
    assert_eq!(fixture.opens(Extension::Jellyfin, &token).await, None);
    assert!(!fixture.sign_out(fixture.as_alice(), &token).await);
}

#[tokio::test]
async fn signing_out_needs_a_caller() {
    let fixture = fixture().await;
    let response = fixture
        .schema
        .execute(async_graphql::Request::new(
            r#"mutation { signOutOfExtension(extension: JELLYFIN, token: "x") }"#,
        ))
        .await;
    assert!(!response.errors.is_empty());
}

#[tokio::test]
async fn a_plex_connection_keeps_working_beside_a_jellyfin_one() {
    let fixture = fixture().await;
    let plex = fixture.connect("PLEX").await;
    let plex_token = plex["connectExtension"]["token"]
        .as_str()
        .unwrap()
        .to_owned();
    fixture.connect("JELLYFIN").await;
    let device = token_of(&fixture.sign_in("JELLYFIN", PASSWORD, "device-1").await);
    fixture.disable("JELLYFIN").await;
    assert_eq!(
        fixture.opens(Extension::Plex, &plex_token).await,
        Some(fixture.alice)
    );
    assert_eq!(fixture.opens(Extension::Jellyfin, &device).await, None);
    // Plex's clients are handed the connection's token: none signs in with a password.
    let plex_sign_in = fixture.sign_in("PLEX", PASSWORD, "device-1").await;
    assert!(plex_sign_in["token"].is_null());
    assert_eq!(
        plex_sign_in["userErrors"],
        json!([{ "field": ["input", "extension"], "code": "NO_PASSWORD_SIGN_IN" }])
    );
}
