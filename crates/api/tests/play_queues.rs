//! Play queue edits on the official API: Relay payloads, refusals and ownership.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::HashMap;
use std::sync::Arc;

use klapz_api::{build_schema, ApiContext, ApiSchema, GlobalId};
use klapz_core::access::{LibraryAccess, LibraryRole};
use klapz_core::metadata::{ItemKind, MetadataItem};
use klapz_core::testing::{InMemoryLibrary, InMemoryMetadata, InMemoryPlaylists};
use klapz_core::{LibrarySection, Principal, RatingKey, SectionId, SectionKind, UserId};
use serde_json::{json, Value};

const FILM: i64 = 577_986;
const EPISODE_1: i64 = 578_173;
const EPISODE_2: i64 = 577_990;

fn section(id: i64, kind: SectionKind) -> LibrarySection {
    LibrarySection {
        id: SectionId::new(id),
        kind,
        title: format!("library {id}"),
        title_sort: None,
        language: "fr-FR".to_owned(),
        uuid: format!("uuid-{id}"),
        created_at: 0,
        updated_at: 0,
        scanned_at: None,
        content_changed_at: 0,
        hidden: false,
    }
}

fn item(id: i64, section: i64, kind: ItemKind, title: &str) -> MetadataItem {
    MetadataItem {
        id: RatingKey::new(id),
        section_id: SectionId::new(section),
        parent_id: None,
        kind,
        guid: format!("tv.plex.agents.none://{id}"),
        title: Some(title.to_owned()),
        title_sort: None,
        original_title: None,
        summary: None,
        tagline: None,
        studio: None,
        content_rating: None,
        content_rating_age: None,
        position: None,
        year: None,
        released_on: None,
        duration_ms: Some(1_000),
        rating: None,
        audience_rating: None,
        added_at: 0,
        updated_at: 0,
        refreshed_at: None,
        chapter_source: None,
    }
}

fn schema() -> ApiSchema {
    let items = vec![
        item(FILM, 27, ItemKind::Movie, "Soul"),
        item(EPISODE_1, 28, ItemKind::Episode, "Episode 1"),
        item(EPISODE_2, 28, ItemKind::Episode, "Episode 2"),
    ];
    build_schema(ApiContext {
        capabilities: klapz_core::ServerCapabilities::new(
            klapz_core::ServerIdentity::new("0000000000000000000000000000000000000000", "0.1.0"),
            0,
        ),
        library: Arc::new(InMemoryLibrary::new(vec![
            section(27, SectionKind::Movie),
            section(28, SectionKind::Show),
        ])),
        metadata: Arc::new(InMemoryMetadata::new(items.clone())),
        playback: Arc::new(klapz_core::testing::InMemoryPlayback::default()),
        play_queues: Arc::new(klapz_core::testing::InMemoryPlayQueues::default()),
        playlists: Arc::new(InMemoryPlaylists::new(items)),
        access: Arc::new(klapz_core::testing::InMemoryAccess::default()),
        directory: Arc::new(klapz_core::testing::StubDirectory::default()),
        watch: Arc::new(klapz_core::testing::InMemoryWatch::default()),
        streams: Arc::new(klapz_core::testing::StubStreams::default()),
        collections: std::sync::Arc::new(klapz_core::testing::InMemoryCollections::default()),
        session_locks: std::sync::Arc::default(),
        settings: std::sync::Arc::new(klapz_core::testing::InMemorySettings::default()),
        scans: std::sync::Arc::new(klapz_core::testing::RecordedScans::default()),
        matching: None,
    })
}

/// A user given the series library alone.
fn user(id: i64) -> Principal {
    Principal {
        name: format!("user {id}"),
        role: klapz_core::ServerRole::User,
        libraries: LibraryAccess::Only(HashMap::from([(SectionId::new(28), LibraryRole::Viewer)])),
        user: UserId::new(id),
        server_identifier: None,
    }
}

async fn run(schema: &ApiSchema, caller: Principal, query: &str, variables: Value) -> Value {
    let request = async_graphql::Request::new(query)
        .variables(async_graphql::Variables::from_json(variables))
        .data(caller);
    let response = schema.execute(request).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    response.data.into_json().unwrap()
}

fn item_id(key: i64) -> String {
    GlobalId::encode("Item", key)
}

const QUEUE_FIELDS: &str = r"
  id version shuffled totalCount kind clientIdentifier
  selectedEntry { id position item { id } }
  lastAddedEntry { id position }
  entries(first: 10) { edges { node { id item { id } } } }
";

async fn create(schema: &ApiSchema, caller: Principal, items: &[i64]) -> Value {
    create_for(schema, caller, items, "player").await
}

/// A queue for one player: its previous queue is replaced.
async fn create_for(schema: &ApiSchema, caller: Principal, items: &[i64], client: &str) -> Value {
    let query = format!(
        "mutation Create($input: CreatePlayQueueInput!) {{ createPlayQueue(input: $input) {{ {QUEUE_FIELDS} }} }}"
    );
    let ids: Vec<String> = items.iter().map(|key| item_id(*key)).collect();
    let data = run(
        schema,
        caller,
        &query,
        json!({ "input": {
            "sourceUri": "library://x/directory/test",
            "itemIds": ids,
            "selectedPosition": 0,
            "shuffle": false,
            "clientIdentifier": client,
        }}),
    )
    .await;
    data["createPlayQueue"].clone()
}

async fn mutate(
    schema: &ApiSchema,
    caller: Principal,
    field: &str,
    input_type: &str,
    input: Value,
) -> Value {
    let query = format!(
        "mutation Edit($input: {input_type}!) {{ {field}(input: $input) {{ clientMutationId playQueue {{ {QUEUE_FIELDS} }} userErrors {{ field code }} }} }}"
    );
    run(schema, caller, &query, json!({ "input": input })).await[field].clone()
}

fn order(queue: &Value) -> Vec<i64> {
    queue["entries"]["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|edge| {
            GlobalId::decode(edge["node"]["item"]["id"].as_str().unwrap())
                .unwrap()
                .key
        })
        .collect()
}

fn entry_ids(queue: &Value) -> Vec<String> {
    queue["entries"]["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|edge| edge["node"]["id"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn a_new_queue_names_its_player_and_kind() {
    let schema = schema();
    let queue = create(
        &schema,
        klapz_core::testing::owner(),
        &[EPISODE_1, EPISODE_2],
    )
    .await;
    assert_eq!(queue["kind"], "VIDEO");
    assert_eq!(queue["clientIdentifier"], "player");
    assert_eq!(queue["lastAddedEntry"], Value::Null);
    assert_eq!(queue["totalCount"], 2);
}

#[tokio::test]
async fn additions_answer_with_the_queue_and_echo_the_mutation_id() {
    let schema = schema();
    let owner = || klapz_core::testing::owner();
    let queue = create(&schema, owner(), &[EPISODE_1, EPISODE_2]).await;
    let payload = mutate(
        &schema,
        owner(),
        "addToPlayQueue",
        "AddToPlayQueueInput",
        json!({
            "clientMutationId": "m1",
            "playQueueId": queue["id"],
            "itemIds": [item_id(FILM)],
        }),
    )
    .await;
    assert_eq!(payload["clientMutationId"], "m1");
    assert_eq!(payload["userErrors"], json!([]));
    let edited = &payload["playQueue"];
    assert_eq!(order(edited), [EPISODE_1, FILM, EPISODE_2]);
    assert_eq!(edited["version"], 2);
    assert_eq!(edited["lastAddedEntry"]["position"], 1);

    let refused = mutate(
        &schema,
        owner(),
        "shufflePlayQueue",
        "PlayQueueInput",
        json!({ "playQueueId": queue["id"] }),
    )
    .await;
    assert_eq!(refused["playQueue"], Value::Null);
    assert_eq!(
        refused["userErrors"],
        json!([{ "field": ["input", "playQueueId"], "code": "UP_NEXT" }])
    );
}

#[tokio::test]
async fn a_move_after_an_entry_of_another_queue_is_refused() {
    let schema = schema();
    let owner = || klapz_core::testing::owner();
    let first = create(&schema, owner(), &[EPISODE_1, EPISODE_2]).await;
    let second = create_for(&schema, owner(), &[FILM, EPISODE_1], "another player").await;
    let refused = mutate(
        &schema,
        owner(),
        "movePlayQueueEntry",
        "MovePlayQueueEntryInput",
        json!({
            "entryId": entry_ids(&first)[1],
            "afterEntryId": entry_ids(&second)[0],
        }),
    )
    .await;
    assert_eq!(
        refused["userErrors"],
        json!([{ "field": ["input", "afterEntryId"], "code": "AFTER_NOT_FOUND" }])
    );
    let moved = mutate(
        &schema,
        owner(),
        "movePlayQueueEntry",
        "MovePlayQueueEntryInput",
        json!({ "entryId": entry_ids(&first)[1] }),
    )
    .await;
    assert_eq!(order(&moved["playQueue"]), [EPISODE_2, EPISODE_1]);
    assert_eq!(moved["playQueue"]["selectedEntry"]["position"], 1);
}

#[tokio::test]
async fn an_item_the_caller_cannot_read_is_not_queued() {
    let schema = schema();
    let queue = create(&schema, user(1), &[EPISODE_1]).await;
    let refused = mutate(
        &schema,
        user(1),
        "addToPlayQueue",
        "AddToPlayQueueInput",
        json!({ "playQueueId": queue["id"], "itemIds": [item_id(FILM)], "next": true }),
    )
    .await;
    assert_eq!(
        refused["userErrors"],
        json!([{ "field": ["input", "itemIds"], "code": "ITEM_NOT_FOUND" }])
    );
}

#[tokio::test]
async fn another_viewer_s_queue_and_entries_answer_as_nothing() {
    let schema = schema();
    let queue = create(&schema, user(1), &[EPISODE_1, EPISODE_2]).await;
    let entry = entry_ids(&queue)[0].clone();
    for (field, input_type, input, at) in [
        (
            "resetPlayQueue",
            "PlayQueueInput",
            json!({ "playQueueId": queue["id"] }),
            ("playQueueId", "PLAY_QUEUE_NOT_FOUND"),
        ),
        (
            "clearPlayQueue",
            "PlayQueueInput",
            json!({ "playQueueId": queue["id"] }),
            ("playQueueId", "PLAY_QUEUE_NOT_FOUND"),
        ),
        (
            "removeFromPlayQueue",
            "RemoveFromPlayQueueInput",
            json!({ "entryId": entry }),
            ("entryId", "ENTRY_NOT_FOUND"),
        ),
        (
            "movePlayQueueEntry",
            "MovePlayQueueEntryInput",
            json!({ "entryId": entry }),
            ("entryId", "ENTRY_NOT_FOUND"),
        ),
    ] {
        let refused = mutate(&schema, user(2), field, input_type, input).await;
        assert_eq!(
            refused["userErrors"],
            json!([{ "field": ["input", at.0], "code": at.1 }]),
            "{field}"
        );
    }
    let node = run(
        &schema,
        user(2),
        "query($id: ID!) { node(id: $id) { id } }",
        json!({ "id": queue["id"] }),
    )
    .await;
    assert_eq!(node["node"], Value::Null);
}

#[tokio::test]
async fn removing_clearing_and_resetting_follow_the_entry_playing() {
    let schema = schema();
    let owner = || klapz_core::testing::owner();
    let queue = create(&schema, owner(), &[EPISODE_1, EPISODE_2, FILM]).await;
    let ids = entry_ids(&queue);
    let removed = mutate(
        &schema,
        owner(),
        "removeFromPlayQueue",
        "RemoveFromPlayQueueInput",
        json!({ "entryId": ids[0] }),
    )
    .await;
    assert_eq!(removed["playQueue"]["selectedEntry"]["id"], ids[1].as_str());
    let reset = mutate(
        &schema,
        owner(),
        "resetPlayQueue",
        "PlayQueueInput",
        json!({ "playQueueId": queue["id"] }),
    )
    .await;
    assert_eq!(reset["playQueue"]["version"], 2, "a reset raises nothing");
    let cleared = mutate(
        &schema,
        owner(),
        "clearPlayQueue",
        "PlayQueueInput",
        json!({ "playQueueId": queue["id"] }),
    )
    .await;
    assert_eq!(order(&cleared["playQueue"]), [EPISODE_2]);
    let unshuffled = mutate(
        &schema,
        owner(),
        "unshufflePlayQueue",
        "PlayQueueInput",
        json!({ "playQueueId": queue["id"] }),
    )
    .await;
    assert_eq!(
        order(&unshuffled["playQueue"]),
        [EPISODE_1, EPISODE_2, FILM]
    );
    assert_eq!(
        unshuffled["playQueue"]["selectedEntry"]["id"],
        ids[1].as_str()
    );
}
