//! Connecting users through an extension whose clients sign in with a password — Jellyfin —
//! beside Plex's, whose clients are handed the connection's token.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use klapz_api::{build_schema, ApiContext, ApiSchema, GlobalId};
use klapz_core::access::{token_digest, AccessRepository, Extension, NewUser, ServerRole};
use klapz_core::testing::{InMemoryAccess, InMemoryLibrary, InMemoryMetadata};
use klapz_core::{Principal, UserId};
use serde_json::{json, Value};

const PASSWORD: &str = "correct horse";

struct Fixture {
    schema: ApiSchema,
    access: Arc<InMemoryAccess>,
    sessions: Arc<klapz_api::Sessions>,
    alice: UserId,
}

/// `alice`, an active user with a password and no connection yet.
async fn fixture() -> Fixture {
    let access = Arc::new(InMemoryAccess::default());
    let hash = klapz_core::account::hash_password(PASSWORD).unwrap();
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
        capabilities: klapz_core::ServerCapabilities::new(
            klapz_core::ServerIdentity::new("0000000000000000000000000000000000000000", "0.1.0"),
            0,
        ),
        library: Arc::new(InMemoryLibrary::new(Vec::new())),
        metadata: Arc::new(InMemoryMetadata::default()),
        playback: Arc::new(klapz_core::testing::InMemoryPlayback::default()),
        play_queues: Arc::new(klapz_core::testing::InMemoryPlayQueues::default()),
        playlists: Arc::new(klapz_core::testing::InMemoryPlaylists::default()),
        collections: Arc::new(klapz_core::testing::InMemoryCollections::default()),
        access: Arc::clone(&access) as Arc<dyn AccessRepository>,
        directory: Arc::new(klapz_core::testing::StubDirectory::default()),
        watch: Arc::new(klapz_core::testing::InMemoryWatch::default()),
        streams: Arc::new(klapz_core::testing::StubStreams::default()),
        session_locks: Arc::default(),
        scans: Arc::new(klapz_core::testing::RecordedScans::default()),
        matching: None,
    });
    let sessions = Arc::new(klapz_api::Sessions::new(
        Arc::new(klapz_core::account::SessionKey::generate()),
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
                .data(klapz_core::testing::owner())
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
            libraries: klapz_core::access::LibraryAccess::Only(std::collections::HashMap::new()),
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

impl Fixture {
    async fn as_user(
        &self,
        caller: Principal,
        query: &str,
        variables: Value,
    ) -> async_graphql::Response {
        self.schema
            .execute(
                async_graphql::Request::new(query)
                    .data(caller)
                    .data(Arc::clone(&self.sessions))
                    .variables(async_graphql::Variables::from_json(variables)),
            )
            .await
    }

    fn alice_id(&self) -> String {
        GlobalId::encode("User", self.alice.get())
    }
}

const CONNECT: &str = "mutation($user: ID!, $extension: ExtensionType!) {
    connectExtension(userId: $user, extension: $extension) { userExtension { extension enabled } }
}";
const DISABLE: &str = "mutation($user: ID!, $extension: ExtensionType!) {
    disableExtension(userId: $user, extension: $extension)
}";

fn bob() -> Principal {
    Principal {
        user: UserId::new(77),
        name: "bob".to_owned(),
        role: ServerRole::User,
        libraries: klapz_core::access::LibraryAccess::Only(std::collections::HashMap::new()),
        server_identifier: None,
    }
}

#[tokio::test]
async fn a_user_switches_its_own_jellyfin_on_and_off_but_not_plex_nor_another_s() {
    let fixture = fixture().await;
    let jellyfin = json!({ "user": fixture.alice_id(), "extension": "JELLYFIN" });
    let on = fixture
        .as_user(fixture.as_alice(), CONNECT, jellyfin.clone())
        .await;
    assert!(on.errors.is_empty(), "{:?}", on.errors);
    assert_eq!(
        on.data.into_json().unwrap()["connectExtension"]["userExtension"],
        json!({ "extension": "JELLYFIN", "enabled": true })
    );
    let signed_in = fixture.sign_in("JELLYFIN", PASSWORD, "device-1").await;
    assert_eq!(signed_in["userErrors"], json!([]));

    let plex = json!({ "user": fixture.alice_id(), "extension": "PLEX" });
    assert!(!fixture
        .as_user(fixture.as_alice(), CONNECT, plex)
        .await
        .errors
        .is_empty());
    assert!(!fixture
        .as_user(bob(), CONNECT, jellyfin.clone())
        .await
        .errors
        .is_empty());
    assert!(!fixture
        .as_user(bob(), DISABLE, jellyfin.clone())
        .await
        .errors
        .is_empty());

    let off = fixture.as_user(fixture.as_alice(), DISABLE, jellyfin).await;
    assert_eq!(
        off.data.into_json().unwrap()["disableExtension"],
        json!(true)
    );
    let token = token_of(&signed_in);
    assert_eq!(fixture.opens(Extension::Jellyfin, &token).await, None);
}

#[tokio::test]
async fn a_user_lists_its_jellyfin_devices_and_signs_one_out() {
    let fixture = fixture().await;
    fixture.connect("JELLYFIN").await;
    let first = token_of(&fixture.sign_in("JELLYFIN", PASSWORD, "device-1").await);
    let second = token_of(&fixture.sign_in("JELLYFIN", PASSWORD, "device-2").await);
    let devices = "query($user: ID!) { node(id: $user) { ... on User {
        extensions { nodes { extension devices { deviceId deviceName client clientVersion } } }
    } } }";
    let listed = fixture
        .as_user(
            fixture.as_alice(),
            devices,
            json!({ "user": fixture.alice_id() }),
        )
        .await
        .data
        .into_json()
        .unwrap();
    let mut ids: Vec<String> = listed["node"]["extensions"]["nodes"][0]["devices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|device| device["deviceId"].as_str().unwrap().to_owned())
        .collect();
    ids.sort();
    assert_eq!(ids, ["device-1", "device-2"]);

    let forget = "mutation($user: ID!, $device: String!) {
        forgetExtensionDevice(userId: $user, extension: JELLYFIN, deviceId: $device)
    }";
    let target = json!({ "user": fixture.alice_id(), "device": "device-1" });
    assert!(!fixture
        .as_user(bob(), forget, target.clone())
        .await
        .errors
        .is_empty());
    let forgotten = fixture.as_user(fixture.as_alice(), forget, target).await;
    assert_eq!(
        forgotten.data.into_json().unwrap()["forgetExtensionDevice"],
        json!(true)
    );
    assert_eq!(fixture.opens(Extension::Jellyfin, &first).await, None);
    assert_eq!(
        fixture.opens(Extension::Jellyfin, &second).await,
        Some(fixture.alice)
    );
}

const SET_PASSWORD: &str = "mutation($input: SetPasswordInput!) {
    setPassword(input: $input) { user { username hasPassword } userErrors { field code } }
}";

fn password_input(current: Option<&str>, new: &str) -> Value {
    json!({ "input": { "currentPassword": current, "newPassword": new } })
}

fn refusals(response: async_graphql::Response) -> Value {
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    response.data.into_json().unwrap()["setPassword"]["userErrors"].clone()
}

#[tokio::test]
async fn a_password_changes_only_with_the_current_one() {
    let fixture = fixture().await;
    let weak = fixture
        .as_user(
            fixture.as_alice(),
            SET_PASSWORD,
            password_input(Some(PASSWORD), "short"),
        )
        .await;
    assert_eq!(
        refusals(weak),
        json!([{ "field": ["input", "newPassword"], "code": "WEAK_PASSWORD" }])
    );
    for wrong in [None, Some("wrong horse")] {
        let refused = fixture
            .as_user(
                fixture.as_alice(),
                SET_PASSWORD,
                password_input(wrong, "a new passphrase"),
            )
            .await;
        assert_eq!(
            refusals(refused),
            json!([{ "field": ["input", "currentPassword"], "code": "INVALID_CREDENTIALS" }])
        );
    }
    let changed = fixture
        .as_user(
            fixture.as_alice(),
            SET_PASSWORD,
            password_input(Some(PASSWORD), "a new passphrase"),
        )
        .await
        .data
        .into_json()
        .unwrap();
    assert_eq!(
        changed["setPassword"],
        json!({ "user": { "username": "alice", "hasPassword": true }, "userErrors": [] })
    );
    fixture.connect("JELLYFIN").await;
    assert_eq!(
        fixture.sign_in("JELLYFIN", PASSWORD, "d").await["userErrors"][0]["code"],
        "INVALID_CREDENTIALS"
    );
    assert_eq!(
        fixture.sign_in("JELLYFIN", "a new passphrase", "d").await["userErrors"],
        json!([])
    );
}

#[tokio::test]
async fn a_user_created_without_a_password_sets_one_and_nobody_sets_one_for_nobody() {
    let fixture = fixture().await;
    let carol = fixture
        .access
        .create_user(
            NewUser {
                username: "carol",
                email: None,
                role: ServerRole::User,
                active: true,
                password_hash: None,
            },
            0,
        )
        .await
        .unwrap();
    let caller = Principal {
        user: carol.id,
        name: "carol".to_owned(),
        ..bob()
    };
    let first = password_input(None, "a first passphrase");
    let response = fixture
        .as_user(caller, SET_PASSWORD, first.clone())
        .await
        .data
        .into_json()
        .unwrap();
    assert_eq!(
        response["setPassword"],
        json!({ "user": { "username": "carol", "hasPassword": true }, "userErrors": [] })
    );
    let nobody = fixture
        .schema
        .execute(
            async_graphql::Request::new(SET_PASSWORD)
                .data(Arc::clone(&fixture.sessions))
                .variables(async_graphql::Variables::from_json(first)),
        )
        .await;
    assert!(!nobody.errors.is_empty());
}

#[tokio::test]
async fn an_owner_sets_another_user_s_password_and_a_user_does_not() {
    let fixture = fixture().await;
    let input =
        json!({ "input": { "userId": fixture.alice_id(), "newPassword": "reset by an owner" } });
    let refused = fixture.as_user(bob(), SET_PASSWORD, input.clone()).await;
    assert!(!refused.errors.is_empty());
    let reset = fixture
        .as_user(klapz_core::testing::owner(), SET_PASSWORD, input)
        .await
        .data
        .into_json()
        .unwrap();
    assert_eq!(
        reset["setPassword"],
        json!({ "user": { "username": "alice", "hasPassword": true }, "userErrors": [] })
    );
    fixture.connect("JELLYFIN").await;
    assert_eq!(
        fixture.sign_in("JELLYFIN", "reset by an owner", "d").await["userErrors"],
        json!([])
    );
    let unknown = json!({ "input": { "userId": GlobalId::encode("User", 999), "newPassword": "whatever passphrase" } });
    assert_eq!(
        refusals(
            fixture
                .as_user(klapz_core::testing::owner(), SET_PASSWORD, unknown)
                .await
        ),
        json!([{ "field": ["input", "userId"], "code": "USER_NOT_FOUND" }])
    );
}
