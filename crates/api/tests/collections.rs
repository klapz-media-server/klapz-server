//! Collections on the official API: what each reader sees, who may change them, the orders
//! their items come in, and what an item says of its collections.
//!
//! Items are the reference's test libraries: Soul in library 27, two episodes of Bull and the
//! series itself in 28.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::HashMap;
use std::sync::Arc;

use open_plex_api::{build_schema, ApiContext, ApiSchema, GlobalId};
use open_plex_core::access::{LibraryAccess, LibraryRole};
use open_plex_core::edit::CalendarDate;
use open_plex_core::metadata::{ItemKind, MetadataItem};
use open_plex_core::testing::{InMemoryCollections, InMemoryLibrary, InMemoryMetadata};
use open_plex_core::{LibrarySection, Principal, RatingKey, SectionId, SectionKind, UserId};
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

fn item(
    id: i64,
    section: i64,
    kind: ItemKind,
    title: &str,
    released: Option<&str>,
) -> MetadataItem {
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
        year: released.map(|date| date[..4].parse().unwrap()),
        released_on: released.map(|date| CalendarDate::parse(date).unwrap().midnight()),
        duration_ms: None,
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
        item(FILM, 27, ItemKind::Movie, "Soul", None),
        item(SHOW, 28, ItemKind::Show, "Bull", Some("2016-09-20")),
        item(
            EPISODE_1,
            28,
            ItemKind::Episode,
            "Episode 1",
            Some("2021-04-30"),
        ),
        item(EPISODE_2, 28, ItemKind::Episode, "Episode 2", None),
    ];
    let collections = InMemoryCollections::new(items.clone());
    let metadata = InMemoryMetadata::new(items).with_collections(&collections);
    build_schema(ApiContext {
        capabilities: open_plex_core::ServerCapabilities::new(
            open_plex_core::ServerIdentity::new(
                "0000000000000000000000000000000000000000",
                "0.1.0",
            ),
            0,
        ),
        library: Arc::new(InMemoryLibrary::new(vec![
            section(27, SectionKind::Movie),
            section(28, SectionKind::Show),
        ])),
        metadata: Arc::new(metadata),
        playback: Arc::new(open_plex_core::testing::InMemoryPlayback::default()),
        play_queues: Arc::new(open_plex_core::testing::InMemoryPlayQueues::default()),
        playlists: Arc::new(open_plex_core::testing::InMemoryPlaylists::default()),
        collections: Arc::new(collections),
        access: Arc::new(open_plex_core::testing::InMemoryAccess::default()),
        directory: Arc::new(open_plex_core::testing::StubDirectory::default()),
        watch: Arc::new(open_plex_core::testing::InMemoryWatch::default()),
        streams: Arc::new(open_plex_core::testing::StubStreams::default()),
        session_locks: Arc::default(),
        scans: std::sync::Arc::new(open_plex_core::testing::RecordedScans::default()),
    })
}

fn owner() -> Principal {
    open_plex_core::testing::owner()
}

/// A user given the series library in this role, and nothing else.
fn user(role: LibraryRole) -> Principal {
    Principal {
        name: "user 7".to_owned(),
        role: open_plex_core::ServerRole::User,
        libraries: LibraryAccess::Only(HashMap::from([(SectionId::new(28), role)])),
        user: UserId::new(7),
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

fn library_id(key: i64) -> String {
    GlobalId::encode("Library", key)
}

const CREATE: &str = r"
mutation Create($input: CreateCollectionInput!) {
  createCollection(input: $input) {
    clientMutationId
    collection { id tagNumber title titleSort kind isSmart itemCount minYear maxYear }
    userErrors { field code }
  }
}
";

const READ: &str = r"
query Read($id: ID!) {
  node(id: $id) {
    ... on Collection {
      title
      order
      mode
      lockedFields
      itemCount
      updatedAt
      library { id }
      items(first: 10) { edges { node { id } } }
    }
  }
}
";

async fn create(
    schema: &ApiSchema,
    caller: Principal,
    title: &str,
    kind: &str,
    items: &[i64],
) -> Value {
    let ids: Vec<String> = items.iter().copied().map(item_id).collect();
    let data = run(
        schema,
        caller,
        CREATE,
        json!({ "input": {
            "libraryId": library_id(28),
            "title": title,
            "kind": kind,
            "itemIds": ids,
        } }),
    )
    .await;
    data["createCollection"].clone()
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
async fn a_new_collection_keeps_the_items_of_its_kind_and_library_once() {
    let schema = schema();
    // Measured on the reference: Soul, of another library, and the series, of another kind,
    // are left out of an episode collection without a refusal.
    let created = create(
        &schema,
        owner(),
        "OP Coll D",
        "EPISODE",
        &[EPISODE_1, EPISODE_2, EPISODE_1, SHOW, FILM],
    )
    .await;
    assert_eq!(created["userErrors"], json!([]));
    let collection = &created["collection"];
    assert_eq!(collection["itemCount"], 2);
    assert_eq!(collection["kind"], "EPISODE");
    assert_eq!(collection["isSmart"], false);
    assert_eq!(collection["minYear"], 2021);
    assert_eq!(collection["maxYear"], 2021);
    assert_eq!(
        GlobalId::decode(collection["id"].as_str().unwrap())
            .unwrap()
            .key,
        EPISODE_1 + 1,
        "numbered past the items"
    );
}

#[tokio::test]
async fn its_items_come_by_release_date_then_by_title_or_as_arranged() {
    let schema = schema();
    let created = create(
        &schema,
        owner(),
        "OP Coll D",
        "EPISODE",
        &[EPISODE_1, EPISODE_2],
    )
    .await;
    let id = created["collection"]["id"].as_str().unwrap().to_owned();
    // Released first, the episode with no date leading: what the reference answered.
    let read = run(&schema, owner(), READ, json!({ "id": id })).await;
    assert_eq!(item_ids(&read), [item_id(EPISODE_2), item_id(EPISODE_1)]);

    let set_order = r"
mutation Order($input: UpdateCollectionInput!) {
  updateCollection(input: $input) { collection { order } userErrors { code } }
}";
    run(
        &schema,
        owner(),
        set_order,
        json!({ "input": { "id": id, "order": "TITLE" } }),
    )
    .await;
    let read = run(&schema, owner(), READ, json!({ "id": id })).await;
    assert_eq!(item_ids(&read), [item_id(EPISODE_1), item_id(EPISODE_2)]);

    run(
        &schema,
        owner(),
        set_order,
        json!({ "input": { "id": id, "order": "CUSTOM" } }),
    )
    .await;
    let moved = run(
        &schema,
        owner(),
        r"
mutation Move($input: MoveCollectionItemInput!) {
  moveCollectionItem(input: $input) { collection { order } userErrors { field code } }
}",
        json!({ "input": {
            "collectionId": id,
            "itemId": item_id(EPISODE_1),
            "afterItemId": item_id(EPISODE_2),
        } }),
    )
    .await;
    assert_eq!(moved["moveCollectionItem"]["userErrors"], json!([]));
    let read = run(&schema, owner(), READ, json!({ "id": id })).await;
    assert_eq!(read["node"]["order"], "CUSTOM");
    assert_eq!(item_ids(&read), [item_id(EPISODE_2), item_id(EPISODE_1)]);
}

#[tokio::test]
async fn adding_refuses_only_what_none_of_can_join() {
    let schema = schema();
    let created = create(&schema, owner(), "OP Coll C", "SHOW", &[]).await;
    let id = created["collection"]["id"].as_str().unwrap().to_owned();
    let add = r"
mutation Add($input: AddToCollectionInput!) {
  addToCollection(input: $input) { addedCount collection { itemCount } userErrors { field code } }
}";
    let refused = run(
        &schema,
        owner(),
        add,
        json!({ "input": { "collectionId": id, "itemIds": [item_id(EPISODE_1)] } }),
    )
    .await;
    assert_eq!(
        refused["addToCollection"]["userErrors"],
        json!([{ "field": ["input", "itemIds"], "code": "NOT_ADDABLE" }])
    );
    let added = run(
        &schema,
        owner(),
        add,
        json!({ "input": { "collectionId": id, "itemIds": [item_id(EPISODE_1), item_id(SHOW)] } }),
    )
    .await;
    assert_eq!(added["addToCollection"]["addedCount"], 1);
    let again = run(
        &schema,
        owner(),
        add,
        json!({ "input": { "collectionId": id, "itemIds": [item_id(SHOW)] } }),
    )
    .await;
    assert_eq!(again["addToCollection"]["addedCount"], 0);
    assert_eq!(again["addToCollection"]["collection"]["itemCount"], 1);
}

#[tokio::test]
async fn an_item_names_its_collections_and_a_listing_filters_on_them() {
    let schema = schema();
    let created = create(&schema, owner(), "OP Coll C", "SHOW", &[SHOW]).await;
    let number = created["collection"]["tagNumber"]
        .as_str()
        .unwrap()
        .to_owned();
    let data = run(
        &schema,
        owner(),
        r#"
query Items($show: ID!, $library: ID!, $number: String!) {
  node(id: $show) { ... on Show { collections(first: 5) { edges { node { title } } } } }
  library: node(id: $library) {
    ... on Library {
      items(first: 5, filter: { collections: [{ tagNumbers: [$number] }] }) {
        edges { node { id } }
      }
      none: items(first: 5, filter: { collections: [{ titles: ["nothing"] }] }) {
        edges { node { id } }
      }
    }
  }
}"#,
        json!({ "show": item_id(SHOW), "library": library_id(28), "number": number }),
    )
    .await;
    assert_eq!(
        data["node"]["collections"]["edges"],
        json!([{ "node": { "title": "OP Coll C" } }])
    );
    assert_eq!(
        data["library"]["items"]["edges"],
        json!([{ "node": { "id": item_id(SHOW) } }])
    );
    assert_eq!(data["library"]["none"]["edges"], json!([]));
}

#[tokio::test]
async fn a_smart_collection_lists_its_rule_and_takes_no_item() {
    let schema = schema();
    let data = run(
        &schema,
        owner(),
        r"
mutation Smart($input: CreateCollectionInput!) {
  createCollection(input: $input) {
    collection { id isSmart itemCount minYear rule { orderBy direction } items(first: 5) { edges { node { id } } } }
    userErrors { code }
  }
}",
        json!({ "input": {
            "libraryId": library_id(28),
            "title": "OP Smart T",
            "kind": "EPISODE",
            "rule": { "filter": { "kind": "EPISODE", "yearFrom": 2021 } },
            "sourceUri": "server://x/com.plexapp.plugins.library/library/sections/28/all?type=4&year=2021",
        } }),
    )
    .await;
    let collection = &data["createCollection"]["collection"];
    assert_eq!(collection["isSmart"], true);
    assert_eq!(collection["itemCount"], 1);
    assert_eq!(collection["minYear"], Value::Null);
    assert_eq!(
        collection["items"]["edges"],
        json!([{ "node": { "id": item_id(EPISODE_1) } }])
    );
    let id = collection["id"].as_str().unwrap();
    let add = run(
        &schema,
        owner(),
        r"
mutation Add($input: AddToCollectionInput!) {
  addToCollection(input: $input) { userErrors { code } }
}",
        json!({ "input": { "collectionId": id, "itemIds": [item_id(EPISODE_2)] } }),
    )
    .await;
    assert_eq!(
        add["addToCollection"]["userErrors"],
        json!([{ "code": "SMART_COLLECTION" }])
    );
}

#[tokio::test]
async fn a_viewer_reads_but_changes_nothing_and_an_outsider_sees_nothing() {
    let schema = schema();
    let created = create(&schema, owner(), "OP Coll D", "EPISODE", &[EPISODE_1]).await;
    let id = created["collection"]["id"].as_str().unwrap().to_owned();

    let read = run(
        &schema,
        user(LibraryRole::Viewer),
        READ,
        json!({ "id": id }),
    )
    .await;
    assert_eq!(read["node"]["title"], "OP Coll D");
    let refused = create(&schema, user(LibraryRole::Viewer), "Mine", "SHOW", &[]).await;
    assert_eq!(refused["userErrors"][0]["code"], "LIBRARY_NOT_FOUND");
    let renamed = run(
        &schema,
        user(LibraryRole::Viewer),
        r"
mutation Rename($input: UpdateCollectionInput!) {
  updateCollection(input: $input) { collection { title } userErrors { code } }
}",
        json!({ "input": { "id": id, "title": "Taken" } }),
    )
    .await;
    assert_eq!(
        renamed["updateCollection"]["userErrors"],
        json!([{ "code": "COLLECTION_NOT_FOUND" }])
    );

    // An admin of the library changes it as an owner does.
    let admin = create(
        &schema,
        user(LibraryRole::Admin),
        "Admin's",
        "SHOW",
        &[SHOW],
    )
    .await;
    assert_eq!(admin["userErrors"], json!([]));

    // Given only the film library, a user meets nothing of the series library's.
    let outsider = Principal {
        libraries: LibraryAccess::Only(HashMap::from([(SectionId::new(27), LibraryRole::Admin)])),
        ..user(LibraryRole::Viewer)
    };
    let read = run(&schema, outsider.clone(), READ, json!({ "id": id })).await;
    assert_eq!(read["node"], Value::Null);
    let listed = run(
        &schema,
        outsider,
        r"query { collections(first: 10) { totalCount edges { node { title } } } }",
        json!({}),
    )
    .await;
    assert_eq!(listed["collections"]["totalCount"], 0);
}

#[tokio::test]
async fn edits_lock_fields_and_leave_the_date_and_removing_dates_it() {
    let schema = schema();
    let created = create(&schema, owner(), "OP Coll D", "EPISODE", &[EPISODE_1]).await;
    let id = created["collection"]["id"].as_str().unwrap().to_owned();
    let before = run(&schema, owner(), READ, json!({ "id": id })).await;
    let edited = run(
        &schema,
        owner(),
        r"
mutation Edit($input: UpdateCollectionInput!) {
  updateCollection(input: $input) {
    collection { title titleSort summary mode lockedFields updatedAt }
    userErrors { code }
  }
}",
        json!({ "input": {
            "id": id,
            "title": "The Saga",
            "summary": "A summary",
            "mode": "INSTEAD_OF_ITEMS",
            "lock": ["TITLE", "POSTER"],
        } }),
    )
    .await;
    let collection = &edited["updateCollection"]["collection"];
    assert_eq!(collection["titleSort"], "Saga");
    assert_eq!(collection["mode"], "INSTEAD_OF_ITEMS");
    assert_eq!(collection["lockedFields"], json!(["TITLE", "POSTER"]));
    assert_eq!(collection["updatedAt"], before["node"]["updatedAt"]);

    let removed = run(
        &schema,
        owner(),
        r"
mutation Remove($input: RemoveFromCollectionInput!) {
  removeFromCollection(input: $input) { removed collection { itemCount } userErrors { code } }
}",
        json!({ "input": { "collectionId": id, "itemId": item_id(EPISODE_1) } }),
    )
    .await;
    assert_eq!(removed["removeFromCollection"]["removed"], true);
    assert_eq!(
        removed["removeFromCollection"]["collection"]["itemCount"],
        0
    );

    let deleted = run(
        &schema,
        owner(),
        r"
mutation Delete($input: DeleteCollectionInput!) {
  deleteCollection(input: $input) { deletedCollectionId userErrors { code } }
}",
        json!({ "input": { "id": id } }),
    )
    .await;
    assert_eq!(deleted["deleteCollection"]["deletedCollectionId"], id);
    let read = run(&schema, owner(), READ, json!({ "id": id })).await;
    assert_eq!(read["node"], Value::Null);
}
