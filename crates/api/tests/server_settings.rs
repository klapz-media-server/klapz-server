//! The server's settings on the official API, and the values a library holds in place of
//! some: owners read and change them, all of a change or none of it, and nobody else learns
//! that there are any.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use klapz_api::{build_schema, ApiContext, ApiSchema};
use klapz_core::access::LibraryAccess;
use klapz_core::testing::{
    InMemoryAccess, InMemoryLibrary, InMemoryMetadata, InMemorySettings, RecordedScans,
};
use klapz_core::{LibrarySection, Principal, SectionId, SectionKind, ServerRole};
use serde_json::{json, Value};

fn schema(access: InMemoryAccess) -> ApiSchema {
    schema_with(access, Vec::new(), Arc::default())
}

fn schema_with(
    access: InMemoryAccess,
    libraries: Vec<LibrarySection>,
    scans: Arc<RecordedScans>,
) -> ApiSchema {
    build_schema(ApiContext {
        capabilities: klapz_core::ServerCapabilities::new(
            klapz_core::ServerIdentity::new("0000000000000000000000000000000000000000", "0.1.0"),
            0,
        ),
        library: Arc::new(InMemoryLibrary::new(libraries)),
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
        scans,
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
        key kind value defaultValue isDefault choices minimum maximum optional updatedAt
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
    assert_eq!(
        keys,
        [
            "analysis.concurrency.deep",
            "analysis.concurrency.headers",
            "analysis.concurrency.preview_thumbnails",
            "analysis.concurrency.subtitles",
            "analysis.deep",
            "analysis.preview_thumbnails",
            "analysis.subtitles",
            "general.public_url",
            "transcoder.h264.crf",
            "transcoder.h264.preset"
        ]
    );

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
            "key": "transcoder.h264.crf",
            "kind": "INTEGER",
            "value": "",
            "defaultValue": "",
            "isDefault": true,
            "choices": null,
            "minimum": "0",
            "maximum": "51",
            "optional": true,
            "updatedAt": null,
            "updatedBy": null,
        }, {
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
            "optional": false,
            "updatedAt": null,
            "updatedBy": null,
        }])
    );

    for (namespace, count) in [
        ("analysis", 7),
        ("analysis.concurrency", 4),
        ("general", 1),
        ("transcoder", 2),
        ("transcoder.h264", 2),
        ("transcoder.h264.crf", 1),
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
        unchanged["server"]["settings"]["nodes"][1]["value"],
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

fn crf(value: Option<&str>) -> Value {
    json!({ "key": "transcoder.h264.crf", "value": value })
}

#[tokio::test]
async fn an_owner_chooses_a_rate_factor_within_x264s_range_or_none() {
    let access = InMemoryAccess::default();
    let owner = access.add_user("owner", ServerRole::Owner).unwrap();
    let schema = schema(access);
    let update = |setting: Value| {
        let schema = &schema;
        let owner = &owner;
        async move {
            run(
                schema,
                caller(owner),
                UPDATE,
                json!({ "input": { "settings": [setting] } }),
            )
            .await["updateServerSettings"]
                .clone()
        }
    };

    for refused in ["52", "-1", "23.5", "auto"] {
        assert_eq!(
            update(crf(Some(refused))).await["userErrors"],
            json!([{ "field": ["settings", "0", "value"], "code": "INVALID_VALUE" }]),
            "{refused}"
        );
    }

    let chosen = update(crf(Some(" 028 "))).await;
    assert_eq!(chosen["userErrors"], json!([]));
    assert_eq!(chosen["settings"][0]["value"], "28");
    assert_eq!(chosen["settings"][0]["isDefault"], false);

    // Chosen empty, every quality keeps its own factor again, and the choice is kept as made.
    let emptied = update(crf(Some(""))).await;
    assert_eq!(emptied["settings"][0]["value"], "");
    assert_eq!(emptied["settings"][0]["isDefault"], false);

    let reset = update(crf(None)).await;
    assert_eq!(reset["settings"][0]["value"], "");
    assert_eq!(reset["settings"][0]["isDefault"], true);
}

const LIBRARY_SETTINGS: &str = r"
query LibrarySettings($id: ID!) {
  node(id: $id) {
    ... on Library {
      settings {
        nodes { key kind value isInherited serverValue updatedBy { username } }
      }
    }
  }
}";

const UPDATE_LIBRARY: &str = r"
mutation UpdateLibrary($input: UpdateLibrarySettingsInput!) {
  updateLibrarySettings(input: $input) {
    library { title }
    settings { key value isInherited serverValue }
    userErrors { field code }
  }
}";

fn films() -> LibrarySection {
    LibrarySection {
        id: SectionId::new(1),
        kind: SectionKind::Movie,
        title: "Films".to_owned(),
        title_sort: None,
        language: klapz_core::language::LanguageTag::parse("fr-FR").unwrap(),
        uuid: "uuid".to_owned(),
        created_at: 0,
        updated_at: 0,
        scanned_at: None,
        content_changed_at: 0,
        hidden: false,
    }
}

fn library_id() -> String {
    klapz_api::GlobalId::encode("Library", 1)
}

#[tokio::test]
async fn a_library_follows_the_servers_analysis_until_an_owner_gives_it_its_own() {
    let access = InMemoryAccess::default();
    let owner = access.add_user("owner", ServerRole::Owner).unwrap();
    let viewer = access.add_user("viewer", ServerRole::User).unwrap();
    let scans = Arc::new(RecordedScans::default());
    let schema = schema_with(access, vec![films()], Arc::clone(&scans));
    let deep = |value: Option<&str>| json!({ "key": "analysis.deep", "value": value });

    run(
        &schema,
        caller(&owner),
        UPDATE,
        json!({ "input": { "settings": [deep(Some("false"))] } }),
    )
    .await;
    let listed = run(
        &schema,
        caller(&owner),
        LIBRARY_SETTINGS,
        json!({ "id": library_id() }),
    )
    .await;
    assert_eq!(
        listed["node"]["settings"]["nodes"],
        json!([
            { "key": "analysis.deep", "kind": "BOOLEAN", "value": "false", "isInherited": true,
              "serverValue": "false", "updatedBy": null },
            { "key": "analysis.preview_thumbnails", "kind": "BOOLEAN", "value": "true",
              "isInherited": true, "serverValue": "true", "updatedBy": null },
            { "key": "analysis.subtitles", "kind": "BOOLEAN", "value": "true",
              "isInherited": true, "serverValue": "true", "updatedBy": null },
        ])
    );

    let changed = run(
        &schema,
        caller(&owner),
        UPDATE_LIBRARY,
        json!({ "input": { "libraryId": library_id(), "settings": [deep(Some("true"))] } }),
    )
    .await;
    assert_eq!(
        changed["updateLibrarySettings"],
        json!({
            "library": { "title": "Films" },
            "settings": [{ "key": "analysis.deep", "value": "true", "isInherited": false,
                           "serverValue": "false" }],
            "userErrors": [],
        })
    );
    assert_eq!(
        scans
            .settings_changes
            .load(std::sync::atomic::Ordering::SeqCst),
        2,
        "the analysis is told of both changes"
    );

    let inherited = run(
        &schema,
        caller(&owner),
        UPDATE_LIBRARY,
        json!({ "input": { "libraryId": library_id(), "settings": [deep(None)] } }),
    )
    .await;
    assert_eq!(
        inherited["updateLibrarySettings"]["settings"][0]["isInherited"],
        true
    );

    let hidden = run(
        &schema,
        caller(&viewer),
        LIBRARY_SETTINGS,
        json!({ "id": library_id() }),
    )
    .await;
    assert!(
        hidden["node"].is_null() || hidden["node"]["settings"]["nodes"] == json!([]),
        "{hidden}"
    );
    let refused = execute(
        &schema,
        caller(&viewer),
        UPDATE_LIBRARY,
        json!({ "input": { "libraryId": library_id(), "settings": [deep(Some("true"))] } }),
    )
    .await;
    assert_eq!(refused.errors.len(), 1);
}

#[tokio::test]
async fn a_library_takes_no_value_of_the_servers_own_settings() {
    let access = InMemoryAccess::default();
    let owner = access.add_user("owner", ServerRole::Owner).unwrap();
    let schema = schema_with(access, vec![films()], Arc::default());

    let refused = run(
        &schema,
        caller(&owner),
        UPDATE_LIBRARY,
        json!({ "input": { "libraryId": library_id(), "settings": [preset(Some("slow"))] } }),
    )
    .await;
    assert_eq!(
        refused["updateLibrarySettings"]["userErrors"],
        json!([{ "field": ["settings", "0", "key"], "code": "SERVER_ONLY" }])
    );
    let unknown = run(
        &schema,
        caller(&owner),
        UPDATE_LIBRARY,
        json!({ "input": {
            "libraryId": klapz_api::GlobalId::encode("Library", 9),
            "settings": [{ "key": "analysis.deep", "value": "false" }],
        } }),
    )
    .await;
    assert_eq!(
        unknown["updateLibrarySettings"],
        json!({
            "library": null,
            "settings": [],
            "userErrors": [{ "field": ["libraryId"], "code": "LIBRARY_NOT_FOUND" }],
        })
    );
}
