//! The server's settings on the official API: owners read and change them, all of a change or
//! none of it, and nobody else learns that there are any.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use klapz_api::{build_schema, ApiContext, ApiSchema};
use klapz_core::access::LibraryAccess;
use klapz_core::testing::{InMemoryAccess, InMemoryLibrary, InMemoryMetadata, InMemorySettings};
use klapz_core::{Principal, ServerRole};
use serde_json::{json, Value};

fn schema(access: InMemoryAccess) -> ApiSchema {
    build_schema(ApiContext {
        capabilities: klapz_core::ServerCapabilities::new(
            klapz_core::ServerIdentity::new("0000000000000000000000000000000000000000", "0.1.0"),
            0,
        ),
        library: Arc::new(InMemoryLibrary::new(Vec::new())),
        metadata: Arc::new(InMemoryMetadata::new(Vec::new())),
        playback: Arc::new(klapz_core::testing::InMemoryPlayback::default()),
        play_queues: Arc::new(klapz_core::testing::InMemoryPlayQueues::default()),
        playlists: Arc::new(klapz_core::testing::InMemoryPlaylists::new(Vec::new())),
        collections: Arc::new(klapz_core::testing::InMemoryCollections::default()),
        access: Arc::new(access),
        directory: Arc::new(klapz_core::testing::StubDirectory::default()),
        watch: Arc::new(klapz_core::testing::InMemoryWatch::default()),
        streams: Arc::new(klapz_core::testing::StubStreams::default()),
        session_locks: Arc::default(),
        scans: Arc::new(klapz_core::testing::RecordedScans::default()),
        settings: Arc::new(InMemorySettings::default()),
        matching: None,
    })
}

fn caller(user: &klapz_core::access::User) -> Principal {
    Principal {
        name: user.username.clone(),
        role: user.role,
        libraries: LibraryAccess::All,
        user: user.id,
        server_identifier: None,
    }
}

async fn execute(
    schema: &ApiSchema,
    caller: Principal,
    query: &str,
    variables: Value,
) -> async_graphql::Response {
    let request = async_graphql::Request::new(query)
        .variables(async_graphql::Variables::from_json(variables))
        .data(caller);
    schema.execute(request).await
}

async fn run(schema: &ApiSchema, caller: Principal, query: &str, variables: Value) -> Value {
    let response = execute(schema, caller, query, variables).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    response.data.into_json().unwrap()
}

const SETTINGS: &str = r"
query Settings($namespace: String) {
  server {
    settings(namespace: $namespace) {
      nodes {
        key kind value defaultValue isDefault choices minimum maximum updatedAt
        updatedBy { username }
      }
    }
  }
}";

const UPDATE: &str = r"
mutation Update($input: UpdateServerSettingsInput!) {
  updateServerSettings(input: $input) {
    clientMutationId
    settings { key value isDefault updatedBy { username } }
    userErrors { field code }
  }
}";

fn preset(value: Option<&str>) -> Value {
    json!({ "key": "transcoder.h264.preset", "value": value })
}

#[tokio::test]
async fn an_owner_reads_every_setting_with_its_default_in_force() {
    let access = InMemoryAccess::default();
    let owner = access.add_user("owner", ServerRole::Owner).unwrap();
    let schema = schema(access);

    let data = run(&schema, caller(&owner), SETTINGS, json!({})).await;
    let keys: Vec<&str> = data["server"]["settings"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|setting| setting["key"].as_str().unwrap())
        .collect();
    assert_eq!(keys, ["general.public_url", "transcoder.h264.preset"]);

    let data = run(
        &schema,
        caller(&owner),
        SETTINGS,
        json!({ "namespace": "transcoder" }),
    )
    .await;
    assert_eq!(
        data["server"]["settings"]["nodes"],
        json!([{
            "key": "transcoder.h264.preset",
            "kind": "CHOICE",
            "value": "veryfast",
            "defaultValue": "veryfast",
            "isDefault": true,
            "choices": [
                "ultrafast", "superfast", "veryfast", "faster", "fast", "medium", "slow",
                "slower", "veryslow"
            ],
            "minimum": null,
            "maximum": null,
            "updatedAt": null,
            "updatedBy": null,
        }])
    );

    for (namespace, count) in [
        ("general", 1),
        ("transcoder", 1),
        ("transcoder.h264", 1),
        ("transcode", 0),
    ] {
        let data = run(
            &schema,
            caller(&owner),
            SETTINGS,
            json!({ "namespace": namespace }),
        )
        .await;
        assert_eq!(
            data["server"]["settings"]["nodes"]
                .as_array()
                .unwrap()
                .len(),
            count,
            "{namespace}"
        );
    }
}

#[tokio::test]
async fn nobody_but_an_owner_sees_or_changes_a_setting() {
    let access = InMemoryAccess::default();
    let user = access.add_user("viewer", ServerRole::User).unwrap();
    let schema = schema(access);

    let data = run(&schema, caller(&user), SETTINGS, json!({})).await;
    assert_eq!(data["server"]["settings"]["nodes"], json!([]));

    let response = execute(
        &schema,
        caller(&user),
        UPDATE,
        json!({ "input": { "settings": [preset(Some("slow"))] } }),
    )
    .await;
    assert_eq!(response.errors.len(), 1);
}

#[tokio::test]
async fn a_change_applies_whole_or_not_at_all() {
    let access = InMemoryAccess::default();
    let owner = access.add_user("owner", ServerRole::Owner).unwrap();
    let schema = schema(access);

    let refused = run(
        &schema,
        caller(&owner),
        UPDATE,
        json!({ "input": {
            "clientMutationId": "a",
            "settings": [
                preset(Some("slow")),
                { "key": "transcoder.h265.preset", "value": "slow" },
            ],
        } }),
    )
    .await;
    assert_eq!(
        refused["updateServerSettings"],
        json!({
            "clientMutationId": "a",
            "settings": [],
            "userErrors": [{ "field": ["settings", "1", "key"], "code": "UNKNOWN_SETTING" }],
        })
    );
    let invalid = run(
        &schema,
        caller(&owner),
        UPDATE,
        json!({ "input": { "settings": [preset(Some("placebo"))] } }),
    )
    .await;
    assert_eq!(
        invalid["updateServerSettings"]["userErrors"],
        json!([{ "field": ["settings", "0", "value"], "code": "INVALID_VALUE" }])
    );
    let unchanged = run(
        &schema,
        caller(&owner),
        SETTINGS,
        json!({ "namespace": "transcoder" }),
    )
    .await;
    assert_eq!(
        unchanged["server"]["settings"]["nodes"][0]["value"],
        "veryfast"
    );

    let changed = run(
        &schema,
        caller(&owner),
        UPDATE,
        json!({ "input": { "settings": [preset(Some(" slow "))] } }),
    )
    .await;
    assert_eq!(
        changed["updateServerSettings"]["settings"],
        json!([{
            "key": "transcoder.h264.preset",
            "value": "slow",
            "isDefault": false,
            "updatedBy": { "username": "owner" },
        }])
    );

    let address = run(
        &schema,
        caller(&owner),
        UPDATE,
        json!({ "input": { "settings": [
            { "key": "general.public_url", "value": "https://Media.Example.com/" },
        ] } }),
    )
    .await;
    assert_eq!(
        address["updateServerSettings"]["settings"][0]["value"],
        "https://media.example.com"
    );

    let reset = run(
        &schema,
        caller(&owner),
        UPDATE,
        json!({ "input": { "settings": [preset(None)] } }),
    )
    .await;
    assert_eq!(
        reset["updateServerSettings"]["settings"],
        json!([{
            "key": "transcoder.h264.preset",
            "value": "veryfast",
            "isDefault": true,
            "updatedBy": null,
        }])
    );
}
