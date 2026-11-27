//! Playlists on the official API: who owns them, what each reader sees in them, and what a
//! smart one lists.
//!
//! Items are the reference's test libraries: Soul in library 27, two episodes of Bull and the
//! series itself in 28.

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
const SHOW: i64 = 577_987;
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

fn item(id: i64, section: i64, kind: ItemKind, title: &str, duration_ms: i64) -> MetadataItem {
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
        duration_ms: Some(duration_ms),
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
        item(FILM, 27, ItemKind::Movie, "Soul", 6_031_568),
        item(SHOW, 28, ItemKind::Show, "Bull", 0),
        item(EPISODE_1, 28, ItemKind::Episode, "Episode 1", 2_467_160),
        item(EPISODE_2, 28, ItemKind::Episode, "Episode 2", 2_467_480),
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
        collections: Arc::new(klapz_core::testing::InMemoryCollections::default()),
        access: Arc::new(klapz_core::testing::InMemoryAccess::default()),
        directory: Arc::new(klapz_core::testing::StubDirectory::default()),
        watch: Arc::new(klapz_core::testing::InMemoryWatch::default()),
        streams: Arc::new(klapz_core::testing::StubStreams::default()),
        session_locks: std::sync::Arc::default(),
        settings: std::sync::Arc::new(klapz_core::testing::InMemorySettings::default()),
        scans: std::sync::Arc::new(klapz_core::testing::RecordedScans::default()),
        matching: None,
    })
}

fn owner() -> Principal {
    klapz_core::testing::owner()
}

/// A user who may read only the series library.
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

const CREATE: &str = r"
mutation Create($input: CreatePlaylistInput!) {
  createPlaylist(input: $input) {
    clientMutationId
    playlist { id title isSmart itemCount durationMs viewCount }
    userErrors { field code }
  }
}
";

const READ: &str = r"
query Read($id: ID!) {
  node(id: $id) {
    ... on Playlist {
      title
      summary
      lockedFields
      itemCount
      durationMs
      items(first: 10) { edges { node { id } } }
      entries(first: 10) { edges { node { id item { id } } } }
    }
  }
}
";

async fn create(schema: &ApiSchema, caller: Principal, title: &str, items: &[i64]) -> String {
    let ids: Vec<String> = items.iter().copied().map(item_id).collect();
    let data = run(
        schema,
        caller,
        CREATE,
        json!({ "input": { "title": title, "kind": "VIDEO", "itemIds": ids } }),
    )
    .await;
    data["createPlaylist"]["playlist"]["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn item_ids(read: &Value) -> Vec<String> {
    read["node"]["items"]["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|edge| edge["node"]["id"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn a_new_playlist_holds_its_items_once_and_sums_their_durations() {
    let schema = schema();
    let data = run(
        &schema,
        owner(),
        CREATE,
        json!({ "input": {
            "clientMutationId": "c1",
            "title": "OP Dumb",
            "kind": "VIDEO",
            "itemIds": [item_id(FILM), item_id(FILM), item_id(EPISODE_1)],
        } }),
    )
    .await;
    let created = &data["createPlaylist"];
    assert_eq!(created["clientMutationId"], "c1");
    assert_eq!(created["userErrors"], json!([]));
    assert_eq!(created["playlist"]["itemCount"], 2);
    assert_eq!(created["playlist"]["durationMs"], 6_031_568 + 2_467_160);
    assert_eq!(created["playlist"]["viewCount"], 1);
    assert_eq!(created["playlist"]["isSmart"], false);
}

#[tokio::test]
async fn a_series_is_not_an_item_a_playlist_holds() {
    let data = run(
        &schema(),
        owner(),
        CREATE,
        json!({ "input": { "title": "OP", "kind": "VIDEO", "itemIds": [item_id(SHOW)] } }),
    )
    .await;
    assert_eq!(data["createPlaylist"]["playlist"], Value::Null);
    assert_eq!(
        data["createPlaylist"]["userErrors"],
        json!([{ "field": ["input", "itemIds"], "code": "NOT_PLAYABLE" }])
    );
}

#[tokio::test]
async fn a_playlist_is_its_owners_alone() {
    let schema = schema();
    let id = create(&schema, owner(), "Owner's", &[EPISODE_1]).await;
    let seen_by_user = run(&schema, user(7), READ, json!({ "id": id })).await;
    assert_eq!(seen_by_user["node"], Value::Null);

    let rename = r#"
mutation Rename($id: ID!) {
  updatePlaylist(input: { id: $id, title: "Mine" }) { playlist { id } userErrors { code } }
}
"#;
    let refused = run(&schema, user(7), rename, json!({ "id": id })).await;
    assert_eq!(
        refused["updatePlaylist"]["userErrors"],
        json!([{ "code": "PLAYLIST_NOT_FOUND" }])
    );

    let listing = r"{ playlists(first: 10) { totalCount edges { node { title } } } }";
    let theirs = run(&schema, user(7), listing, json!({})).await;
    assert_eq!(theirs["playlists"]["totalCount"], 0);
    let mine = run(&schema, owner(), listing, json!({})).await;
    assert_eq!(mine["playlists"]["totalCount"], 1);
}

#[tokio::test]
async fn items_of_a_library_the_owner_cannot_read_are_hidden_and_cannot_be_added() {
    let schema = schema();
    let refused = run(
        &schema,
        user(7),
        CREATE,
        json!({ "input": { "title": "Mine", "kind": "VIDEO", "itemIds": [item_id(FILM)] } }),
    )
    .await;
    assert_eq!(
        refused["createPlaylist"]["userErrors"],
        json!([{ "field": ["input", "itemIds"], "code": "ITEM_NOT_FOUND" }])
    );

    // The owner's playlist read by the owner sees both libraries.
    let id = create(&schema, owner(), "Mixed", &[FILM, EPISODE_1]).await;
    let read = run(&schema, owner(), READ, json!({ "id": id })).await;
    assert_eq!(item_ids(&read), [item_id(FILM), item_id(EPISODE_1)]);
}

#[tokio::test]
async fn entries_move_and_leave() {
    let schema = schema();
    let id = create(&schema, owner(), "OP", &[FILM, EPISODE_1, EPISODE_2]).await;
    let read = run(&schema, owner(), READ, json!({ "id": id })).await;
    let entries: Vec<String> = read["node"]["entries"]["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|edge| edge["node"]["id"].as_str().unwrap().to_owned())
        .collect();

    let move_entry = r"
mutation Move($entry: ID!, $after: ID) {
  movePlaylistEntry(input: { entryId: $entry, afterEntryId: $after }) {
    playlist { id }
    userErrors { field code }
  }
}
";
    run(
        &schema,
        owner(),
        move_entry,
        json!({ "entry": entries[2], "after": entries[0] }),
    )
    .await;
    let read = run(&schema, owner(), READ, json!({ "id": id })).await;
    assert_eq!(
        item_ids(&read),
        [item_id(FILM), item_id(EPISODE_2), item_id(EPISODE_1)]
    );
    run(&schema, owner(), move_entry, json!({ "entry": entries[1] })).await;
    let read = run(&schema, owner(), READ, json!({ "id": id })).await;
    assert_eq!(
        item_ids(&read),
        [item_id(EPISODE_1), item_id(FILM), item_id(EPISODE_2)]
    );

    let remove = r"
mutation Remove($entry: ID!) {
  removeFromPlaylist(input: { entryId: $entry }) { playlist { itemCount } userErrors { code } }
}
";
    let removed = run(&schema, owner(), remove, json!({ "entry": entries[0] })).await;
    assert_eq!(removed["removeFromPlaylist"]["playlist"]["itemCount"], 2);
    let again = run(&schema, owner(), remove, json!({ "entry": entries[0] })).await;
    assert_eq!(
        again["removeFromPlaylist"]["userErrors"],
        json!([{ "code": "ENTRY_NOT_FOUND" }])
    );
}

#[tokio::test]
async fn adding_counts_only_what_was_not_there() {
    let schema = schema();
    let id = create(&schema, owner(), "OP", &[FILM]).await;
    let add = r"
mutation Add($id: ID!, $items: [ID!]!) {
  addToPlaylist(input: { playlistId: $id, itemIds: $items }) {
    addedCount
    playlist { itemCount }
  }
}
";
    let added = run(
        &schema,
        owner(),
        add,
        json!({ "id": id, "items": [item_id(EPISODE_2), item_id(FILM)] }),
    )
    .await;
    assert_eq!(added["addToPlaylist"]["addedCount"], 1);
    assert_eq!(added["addToPlaylist"]["playlist"]["itemCount"], 2);
}

const CREATE_SMART: &str = r#"
mutation Smart($library: ID!) {
  createPlaylist(input: {
    title: "OP Smart"
    kind: VIDEO
    rule: { libraryId: $library, filter: { kind: EPISODE }, orderBy: TITLE, direction: DESCENDING }
  }) {
    playlist {
      id
      isSmart
      itemCount
      durationMs
      rule { library { id } orderBy direction }
      items(first: 10) { edges { node { id } } }
      entries(first: 10) { edges { node { id } } }
    }
    userErrors { field code }
  }
}
"#;

#[tokio::test]
async fn a_smart_playlist_lists_its_rule() {
    let schema = schema();
    let library = GlobalId::encode("Library", 28);
    let data = run(
        &schema,
        owner(),
        CREATE_SMART,
        json!({ "library": library }),
    )
    .await;
    let playlist = &data["createPlaylist"]["playlist"];
    assert_eq!(playlist["isSmart"], true);
    assert_eq!(playlist["itemCount"], 2);
    assert_eq!(playlist["durationMs"], Value::Null);
    assert_eq!(playlist["rule"]["library"]["id"], library);
    assert_eq!(playlist["rule"]["direction"], "DESCENDING");
    let items: Vec<&str> = playlist["items"]["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|edge| edge["node"]["id"].as_str().unwrap())
        .collect();
    assert_eq!(items, [item_id(EPISODE_2), item_id(EPISODE_1)]);
    assert_eq!(playlist["entries"]["edges"], json!([]));

    let id = playlist["id"].as_str().unwrap();
    let clear = r"
mutation Clear($id: ID!) { clearPlaylist(input: { id: $id }) { userErrors { code } } }
";
    let refused = run(&schema, owner(), clear, json!({ "id": id })).await;
    assert_eq!(
        refused["clearPlaylist"]["userErrors"],
        json!([{ "code": "SMART_PLAYLIST" }])
    );
}

#[tokio::test]
async fn a_rule_over_a_library_the_caller_cannot_read_is_refused() {
    let data = run(
        &schema(),
        user(7),
        CREATE_SMART,
        json!({ "library": GlobalId::encode("Library", 27) }),
    )
    .await;
    assert_eq!(
        data["createPlaylist"]["userErrors"],
        json!([{ "field": ["input", "rule"], "code": "INVALID_RULE" }])
    );
}

#[tokio::test]
async fn a_request_from_nobody_has_no_playlists() {
    let schema = schema();
    create(&schema, owner(), "Owner's", &[FILM]).await;
    let listing = schema
        .execute(r"{ playlists(first: 10) { totalCount edges { node { id } } } }")
        .await
        .data
        .into_json()
        .unwrap();
    assert_eq!(listing["playlists"]["totalCount"], 0);
}

#[tokio::test]
async fn deleting_answers_the_id_and_then_not_found() {
    let schema = schema();
    let id = create(&schema, owner(), "OP", &[FILM]).await;
    let delete = r"
mutation Delete($id: ID!) {
  deletePlaylist(input: { id: $id }) { deletedPlaylistId userErrors { code } }
}
";
    let deleted = run(&schema, owner(), delete, json!({ "id": id })).await;
    assert_eq!(deleted["deletePlaylist"]["deletedPlaylistId"], id);
    let again = run(&schema, owner(), delete, json!({ "id": id })).await;
    assert_eq!(
        again["deletePlaylist"]["userErrors"],
        json!([{ "code": "PLAYLIST_NOT_FOUND" }])
    );
}
