//! Collections on the Plex surface, against shapes captured on the reference (1.43.5) with
//! collections made of its test libraries: Soul in library 27, Bull and the two episodes of
//! its season 5 in 28. Every collection made there was deleted afterwards.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use http_body_util::BodyExt;
use klapz_api::{build_schema, ApiContext, ApiSchema};
use klapz_api_plex::{router, PlexState};
use klapz_core::access::{LibraryAccess, LibraryRole};
use klapz_core::edit::CalendarDate;
use klapz_core::metadata::{ItemKind, MetadataItem};
use klapz_core::testing::{InMemoryCollections, InMemoryLibrary, InMemoryMetadata};
use klapz_core::{
    LibrarySection, Principal, RatingKey, SectionId, SectionKind, ServerCapabilities,
    ServerIdentity, UserId,
};
use serde_json::Value;
use tower::ServiceExt;

const MACHINE: &str = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
const FILM: i64 = 577_986;
const SHOW: i64 = 577_987;
const EPISODE_1: i64 = 578_173;
const EPISODE_2: i64 = 577_990;

fn section(id: i64, kind: SectionKind, title: &str) -> LibrarySection {
    LibrarySection {
        id: SectionId::new(id),
        kind,
        title: title.to_owned(),
        title_sort: None,
        language: klapz_core::language::LanguageTag::parse("fr-FR").unwrap(),
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
        item(SHOW, 28, ItemKind::Show, "Bull", None),
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
        capabilities: ServerCapabilities::new(ServerIdentity::new(MACHINE, "0.1.0-dev"), 0),
        library: std::sync::Arc::new(InMemoryLibrary::new(vec![
            section(27, SectionKind::Movie, "klapz tests"),
            section(28, SectionKind::Show, "klapz tests series"),
        ])),
        metadata: std::sync::Arc::new(metadata),
        playback: std::sync::Arc::new(klapz_core::testing::InMemoryPlayback::default()),
        play_queues: std::sync::Arc::new(klapz_core::testing::InMemoryPlayQueues::default()),
        playlists: std::sync::Arc::new(klapz_core::testing::InMemoryPlaylists::default()),
        collections: std::sync::Arc::new(collections),
        access: std::sync::Arc::new(klapz_core::testing::InMemoryAccess::default()),
        directory: std::sync::Arc::new(klapz_core::testing::StubDirectory::default()),
        watch: std::sync::Arc::new(klapz_core::testing::InMemoryWatch::default()),
        streams: std::sync::Arc::new(klapz_core::testing::StubStreams::default()),
        session_locks: std::sync::Arc::default(),
        settings: std::sync::Arc::new(klapz_core::testing::InMemorySettings::default()),
        scans: std::sync::Arc::new(klapz_core::testing::RecordedScans::default()),
        matching: None,
    })
}

fn app_as(schema: &ApiSchema, caller: Principal) -> axum::Router {
    router(PlexState::new(
        ServerIdentity::new(MACHINE, "0.1.0-dev"),
        schema.clone(),
    ))
    .layer(axum::Extension(caller))
}

fn owner(schema: &ApiSchema) -> axum::Router {
    app_as(schema, klapz_core::testing::owner())
}

/// A user given the series library as a viewer, and nothing else.
fn viewer(schema: &ApiSchema) -> axum::Router {
    app_as(
        schema,
        Principal {
            name: "user7".to_owned(),
            role: klapz_core::ServerRole::User,
            libraries: LibraryAccess::Only(
                std::iter::once((SectionId::new(28), LibraryRole::Viewer)).collect(),
            ),
            user: UserId::new(7),
            server_identifier: None,
        },
    )
}

async fn send(app: &axum::Router, method: Method, uri: &str, json: bool) -> (StatusCode, String) {
    let mut request = Request::builder().method(method).uri(uri);
    if json {
        request = request.header(header::ACCEPT, "application/json");
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned())
}

async fn json(app: &axum::Router, method: Method, uri: &str) -> Value {
    let (status, body) = send(app, method, uri, true).await;
    assert_eq!(status, StatusCode::OK, "{uri}: {body}");
    serde_json::from_str(&body).unwrap()
}

async fn status(app: &axum::Router, method: Method, uri: &str) -> StatusCode {
    send(app, method, uri, true).await.0
}

fn uri_of(path: &str) -> String {
    let uri = format!("server://{MACHINE}/com.plexapp.plugins.library{path}");
    form_urlencoded::byte_serialize(uri.as_bytes()).collect()
}

/// `POST /library/collections`, as Plex Web sends it, answering the collection.
async fn create(app: &axum::Router, section: i64, kind: u8, title: &str, items: &[i64]) -> Value {
    let title: String = form_urlencoded::byte_serialize(title.as_bytes()).collect();
    let mut uri =
        format!("/library/collections?type={kind}&title={title}&smart=0&sectionId={section}");
    if !items.is_empty() {
        let keys: Vec<String> = items.iter().map(ToString::to_string).collect();
        let path = format!("/library/metadata/{}", keys.join(","));
        uri = format!("{uri}&uri={}", uri_of(&path));
    }
    let body = json(app, Method::POST, &uri).await;
    body["MediaContainer"]["Metadata"][0].clone()
}

fn keys(metadata: &Value) -> Vec<&str> {
    metadata
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect()
}

fn rating_keys(container: &Value) -> Vec<String> {
    container["Metadata"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| item["ratingKey"].as_str().unwrap().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_created_collection_answers_as_the_reference_does() {
    let schema = schema();
    let app = owner(&schema);
    let created = create(&app, 27, 1, "OP Coll A", &[FILM]).await;
    // Captured on POST /library/collections: no library on the collection, in this order.
    assert_eq!(
        keys(&created),
        [
            "ratingKey",
            "key",
            "guid",
            "type",
            "title",
            "subtype",
            "summary",
            "index",
            "ratingCount",
            "thumb",
            "addedAt",
            "updatedAt",
            "childCount"
        ]
    );
    let key = created["ratingKey"].as_str().unwrap();
    assert_eq!(
        created["key"],
        format!("/library/collections/{key}/children")
    );
    assert_eq!(created["type"], "collection");
    assert_eq!(created["subtype"], "movie");
    assert_eq!(created["summary"], "");
    assert_eq!(created["ratingCount"], 1);
    assert_eq!(created["childCount"], 1);
    assert!(created["index"].is_number());
    assert_eq!(
        created["thumb"],
        format!(
            "/library/collections/{key}/composite/{}?width=400&height=600",
            created["updatedAt"]
        )
    );
}

#[tokio::test]
async fn a_lookup_and_a_listing_show_a_collection_with_its_library_where_the_reference_does() {
    let schema = schema();
    let app = owner(&schema);
    let created = create(&app, 28, 4, "OP Coll D", &[EPISODE_1, EPISODE_2]).await;
    let key = created["ratingKey"].as_str().unwrap().to_owned();

    let lookup = json(&app, Method::GET, &format!("/library/metadata/{key}")).await;
    let container = &lookup["MediaContainer"];
    assert_eq!(container["librarySectionID"], 28);
    assert_eq!(container["librarySectionTitle"], "klapz tests series");
    let found = &container["Metadata"][0];
    assert_eq!(found["librarySectionKey"], "/library/sections/28");
    assert_eq!(found["subtype"], "episode");
    assert_eq!(found["ratingCount"], 4);
    // Years as strings, unlike an item's.
    assert_eq!(found["maxYear"], "2021");
    assert_eq!(found["minYear"], "2021");
    assert_eq!(
        json(&app, Method::GET, &format!("/library/collections/{key}")).await,
        lookup
    );

    // A lookup mixing a collection and an item answers both, in the order asked.
    let mixed = json(
        &app,
        Method::GET,
        &format!("/library/metadata/{key},{SHOW}"),
    )
    .await;
    assert_eq!(
        rating_keys(&mixed["MediaContainer"]),
        [key.clone(), SHOW.to_string()]
    );

    let listing = json(&app, Method::GET, "/library/sections/28/collections").await;
    let container = &listing["MediaContainer"];
    assert_eq!(container["title1"], "klapz tests series");
    assert_eq!(container["viewGroup"], "show");
    let listed = &container["Metadata"][0];
    assert!(listed.get("librarySectionID").is_none());
    assert_eq!(
        json(&app, Method::GET, "/library/sections/28/all?type=18").await,
        listing
    );

    let (status, xml) = send(
        &app,
        Method::GET,
        &format!("/library/metadata/{key}"),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(xml.contains("<Directory ratingKey="), "{xml}");
    assert!(xml.contains("type=\"collection\""), "{xml}");
}

#[tokio::test]
async fn its_children_are_items_in_a_listing_form_naming_their_library() {
    let schema = schema();
    let app = owner(&schema);
    let created = create(&app, 28, 4, "OP Coll D", &[EPISODE_1, EPISODE_2]).await;
    let key = created["ratingKey"].as_str().unwrap().to_owned();
    let children = json(
        &app,
        Method::GET,
        &format!("/library/collections/{key}/children"),
    )
    .await;
    let container = &children["MediaContainer"];
    // The episode with no release date first, as on the reference.
    assert_eq!(
        rating_keys(container),
        [EPISODE_2.to_string(), EPISODE_1.to_string()]
    );
    assert_eq!(keys(container), ["size", "Metadata"]);
    let first = &container["Metadata"][0];
    assert_eq!(first["librarySectionID"], 28);
    assert_eq!(first["Collection"][0]["tag"], "OP Coll D");
    assert_eq!(
        json(
            &app,
            Method::GET,
            &format!("/library/collections/{key}/items")
        )
        .await,
        children
    );

    let (_, paged) = send(
        &app,
        Method::GET,
        &format!(
            "/library/collections/{key}/children?X-Plex-Container-Start=0&X-Plex-Container-Size=1"
        ),
        true,
    )
    .await;
    let paged: Value = serde_json::from_str(&paged).unwrap();
    assert_eq!(paged["MediaContainer"]["totalSize"], 2);
    assert_eq!(paged["MediaContainer"]["size"], 1);
}

#[tokio::test]
async fn adding_moving_and_removing_answer_as_measured() {
    let schema = schema();
    let app = owner(&schema);
    let created = create(&app, 28, 4, "OP Coll D", &[EPISODE_1]).await;
    let key = created["ratingKey"].as_str().unwrap().to_owned();
    let items = format!("/library/collections/{key}/items");

    // A uri of the wrong kind alone is a 400; with one that can join, a 200.
    assert_eq!(
        status(
            &app,
            Method::PUT,
            &format!(
                "{items}?uri={}",
                uri_of(&format!("/library/metadata/{SHOW}"))
            )
        )
        .await,
        StatusCode::BAD_REQUEST
    );
    let added = json(
        &app,
        Method::PUT,
        &format!(
            "{items}?uri={}",
            uri_of(&format!("/library/metadata/{SHOW},{EPISODE_2}"))
        ),
    )
    .await;
    let changed = &added["MediaContainer"]["Metadata"][0];
    assert_eq!(changed["childCount"], 2);
    assert_eq!(changed["librarySectionID"], 28);
    assert_eq!(
        status(&app, Method::PUT, &items).await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        status(&app, Method::PUT, &format!("{items}?uri=garbage")).await,
        StatusCode::BAD_REQUEST
    );

    // Moves: after itself, an outsider, or a word are 400s; after an outsider goes first.
    let moving = format!("{items}/{EPISODE_2}/move");
    for refused in [
        format!("{moving}?after={EPISODE_2}"),
        format!("{items}/{FILM}/move"),
        format!("{moving}?after=abc"),
    ] {
        assert_eq!(
            status(&app, Method::PUT, &refused).await,
            StatusCode::BAD_REQUEST,
            "{refused}"
        );
    }
    json(&app, Method::PUT, &format!("{moving}?after={FILM}")).await;
    assert_eq!(
        status(
            &app,
            Method::PUT,
            "/library/collections/999999999/items/1/move"
        )
        .await,
        StatusCode::NOT_FOUND
    );

    // The spec's PUT removes nothing and answers 404; DELETE removes, twice harmlessly.
    let one = format!("{items}/{EPISODE_2}");
    assert_eq!(status(&app, Method::PUT, &one).await, StatusCode::NOT_FOUND);
    let removed = json(&app, Method::DELETE, &one).await;
    assert_eq!(removed["MediaContainer"]["Metadata"][0]["childCount"], 1);
    json(&app, Method::DELETE, &one).await;
    assert_eq!(
        status(
            &app,
            Method::DELETE,
            "/library/collections/999999999/items/1"
        )
        .await,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn the_order_and_mode_are_set_through_prefs() {
    let schema = schema();
    let app = owner(&schema);
    let created = create(&app, 28, 4, "OP Coll D", &[EPISODE_1, EPISODE_2]).await;
    let key = created["ratingKey"].as_str().unwrap().to_owned();
    let prefs = format!("/library/metadata/{key}/prefs");
    let (status_code, body) = send(
        &app,
        Method::PUT,
        &format!("{prefs}?collectionSort=2&collectionMode=1"),
        true,
    )
    .await;
    assert_eq!(status_code, StatusCode::OK);
    assert!(body.is_empty(), "{body}");
    assert_eq!(
        status(&app, Method::PUT, &format!("{prefs}?bogus=1")).await,
        StatusCode::BAD_REQUEST
    );
    let lookup = json(&app, Method::GET, &format!("/library/metadata/{key}")).await;
    let found = &lookup["MediaContainer"]["Metadata"][0];
    assert_eq!(found["collectionSort"], "2");
    assert_eq!(found["collectionMode"], "1");

    // In custom order, a move changes what the children list.
    json(
        &app,
        Method::PUT,
        &format!("/library/collections/{key}/items/{EPISODE_1}/move?after={EPISODE_2}"),
    )
    .await;
    let children = json(
        &app,
        Method::GET,
        &format!("/library/collections/{key}/children"),
    )
    .await;
    assert_eq!(
        rating_keys(&children["MediaContainer"]),
        [EPISODE_2.to_string(), EPISODE_1.to_string()]
    );
    // An item's preferences are not served.
    assert_eq!(
        status(
            &app,
            Method::PUT,
            &format!("/library/metadata/{SHOW}/prefs?x=1")
        )
        .await,
        StatusCode::NOT_IMPLEMENTED
    );
}

#[tokio::test]
async fn an_item_names_its_collections_and_a_listing_filters_on_them() {
    let schema = schema();
    let app = owner(&schema);
    let created = create(&app, 27, 1, "OP Coll A", &[FILM]).await;
    let number = created["index"].as_i64().unwrap();

    // Captured on GET /library/metadata/577986: id, filter, tag, guid, in this order.
    let lookup = json(&app, Method::GET, &format!("/library/metadata/{FILM}")).await;
    let tag = &lookup["MediaContainer"]["Metadata"][0]["Collection"][0];
    assert_eq!(keys(tag), ["id", "filter", "tag", "guid"]);
    assert_eq!(tag["id"], number);
    assert_eq!(tag["filter"], format!("collection={number}"));
    assert_eq!(tag["guid"], created["guid"]);

    // A listing names the collection alone.
    let listing = json(&app, Method::GET, "/library/sections/27/all").await;
    assert_eq!(
        listing["MediaContainer"]["Metadata"][0]["Collection"],
        serde_json::json!([{ "tag": "OP Coll A" }])
    );
    for filter in [number.to_string(), "OP%20Coll%20A".to_owned()] {
        let filtered = json(
            &app,
            Method::GET,
            &format!("/library/sections/27/all?collection={filter}"),
        )
        .await;
        assert_eq!(rating_keys(&filtered["MediaContainer"]), [FILM.to_string()]);
    }
    let none = json(
        &app,
        Method::GET,
        "/library/sections/27/all?collection=999999999",
    )
    .await;
    assert_eq!(none["MediaContainer"]["size"], 0);

    let choices = json(&app, Method::GET, "/library/sections/27/collection").await;
    let container = &choices["MediaContainer"];
    assert_eq!(container["title2"], "By Collection");
    assert_eq!(
        container["Directory"],
        serde_json::json!([{
            "fastKey": format!("/library/sections/27/all?collection={number}"),
            "key": number.to_string(),
            "title": "OP Coll A",
        }])
    );
}

#[tokio::test]
async fn deleting_by_tag_empties_and_deleting_the_metadata_removes() {
    let schema = schema();
    let app = owner(&schema);
    let created = create(&app, 27, 1, "OP Coll A", &[FILM]).await;
    let key = created["ratingKey"].as_str().unwrap().to_owned();
    let number = created["index"].as_i64().unwrap();

    // The spec's "delete a collection" takes the tag number, and empties the collection of
    // that library alone; another library's answer is a 200 that changes nothing.
    assert_eq!(
        status(
            &app,
            Method::DELETE,
            &format!("/library/sections/28/collection/{number}")
        )
        .await,
        StatusCode::OK
    );
    let lookup = json(&app, Method::GET, &format!("/library/metadata/{key}")).await;
    assert_eq!(lookup["MediaContainer"]["Metadata"][0]["childCount"], 1);
    assert_eq!(
        status(
            &app,
            Method::DELETE,
            &format!("/library/sections/27/collection/{number}")
        )
        .await,
        StatusCode::OK
    );
    let lookup = json(&app, Method::GET, &format!("/library/metadata/{key}")).await;
    assert_eq!(lookup["MediaContainer"]["Metadata"][0]["childCount"], 0);
    assert_eq!(
        status(
            &app,
            Method::DELETE,
            "/library/sections/27/collection/999999999"
        )
        .await,
        StatusCode::NOT_FOUND
    );

    // Plex Web deletes the collection itself through its metadata.
    assert_eq!(
        status(&app, Method::DELETE, &format!("/library/metadata/{key}")).await,
        StatusCode::OK
    );
    assert_eq!(
        status(&app, Method::GET, &format!("/library/metadata/{key}")).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        status(&app, Method::DELETE, &format!("/library/metadata/{FILM}")).await,
        StatusCode::NOT_IMPLEMENTED
    );
}

#[tokio::test]
async fn creation_refuses_what_the_reference_refuses() {
    let schema = schema();
    let app = owner(&schema);
    for refused in [
        "/library/collections?type=1&title=OP%20X1&sectionId=999",
        "/library/collections?type=1&sectionId=27",
        "/library/collections?type=1&title=OP%20X4",
        "/library/collections?type=4&title=OP%20U&smart=1&sectionId=28",
    ] {
        assert_eq!(
            status(&app, Method::POST, refused).await,
            StatusCode::BAD_REQUEST,
            "{refused}"
        );
    }
    // No type takes the library's own; a uri naming nothing makes an empty collection.
    let created = json(
        &app,
        Method::POST,
        "/library/collections?title=OP%20X3&sectionId=27&uri=garbage",
    )
    .await;
    let collection = &created["MediaContainer"]["Metadata"][0];
    assert_eq!(collection["subtype"], "movie");
    assert_eq!(collection["childCount"], 0);
}

#[tokio::test]
async fn a_smart_collection_carries_its_content_and_lists_it() {
    let schema = schema();
    let app = owner(&schema);
    let created = json(
        &app,
        Method::POST,
        &format!(
            "/library/collections?type=4&title=OP%20Smart%20T&smart=1&sectionId=28&uri={}",
            uri_of("/library/sections/28/all?type=4&year=2021")
        ),
    )
    .await;
    let collection = &created["MediaContainer"]["Metadata"][0];
    assert_eq!(collection["ratingCount"], 104);
    assert_eq!(collection["smart"], "1");
    assert_eq!(collection["childCount"], 1);
    assert!(collection["content"]
        .as_str()
        .unwrap()
        .ends_with("/library/sections/28/all?type=4&year=2021"));
    let key = collection["ratingKey"].as_str().unwrap();
    let children = json(
        &app,
        Method::GET,
        &format!("/library/collections/{key}/children"),
    )
    .await;
    assert_eq!(
        rating_keys(&children["MediaContainer"]),
        [EPISODE_1.to_string()]
    );
    assert_eq!(
        status(
            &app,
            Method::PUT,
            &format!(
                "/library/collections/{key}/items?uri={}",
                uri_of(&format!("/library/metadata/{EPISODE_2}"))
            )
        )
        .await,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn a_viewer_reads_collections_and_changes_none() {
    let schema = schema();
    let created = create(&owner(&schema), 28, 2, "OP Coll C", &[SHOW]).await;
    let key = created["ratingKey"].as_str().unwrap().to_owned();
    let app = viewer(&schema);
    json(&app, Method::GET, &format!("/library/metadata/{key}")).await;
    assert_eq!(
        status(&app, Method::DELETE, &format!("/library/metadata/{key}")).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        status(
            &app,
            Method::POST,
            "/library/collections?type=2&title=Mine&sectionId=28"
        )
        .await,
        StatusCode::BAD_REQUEST
    );
    // The film library is not given: its collections are nobody's here.
    assert_eq!(
        status(&app, Method::GET, "/library/sections/27/collections").await,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn a_type_18_edit_renames_and_locks() {
    let schema = schema();
    let app = owner(&schema);
    let created = create(&app, 27, 1, "OP Lock", &[FILM]).await;
    let key = created["ratingKey"].as_str().unwrap().to_owned();
    let (status_code, _) = send(
        &app,
        Method::PUT,
        &format!(
            "/library/sections/27/all?type=18&id={key}&title.value=OP%20Lock2&title.locked=1&titleSort.value=aaa&titleSort.locked=1&summary.locked=1&thumb.locked=1"
        ),
        true,
    )
    .await;
    assert_eq!(status_code, StatusCode::OK);
    let lookup = json(&app, Method::GET, &format!("/library/metadata/{key}")).await;
    let found = &lookup["MediaContainer"]["Metadata"][0];
    assert_eq!(found["title"], "OP Lock2");
    assert_eq!(found["titleSort"], "aaa");
    // Captured: the locks, as an item lists its own.
    assert_eq!(
        found["Field"],
        serde_json::json!([
            { "locked": true, "name": "title" },
            { "locked": true, "name": "titleSort" },
            { "locked": true, "name": "summary" },
            { "locked": true, "name": "thumb" },
        ])
    );
    // A field the reference keeps on a collection and this server does not.
    assert_eq!(
        status(
            &app,
            Method::PUT,
            &format!("/library/sections/27/all?type=18&id={key}&year.value=1999")
        )
        .await,
        StatusCode::NOT_IMPLEMENTED
    );
}

#[tokio::test]
async fn the_search_shelf_lists_a_collection_as_the_reference_does() {
    let schema = schema();
    let app = owner(&schema);
    let created = create(&app, 27, 1, "OP Coll A", &[FILM]).await;
    let number = created["index"].as_i64().unwrap();
    let body = json(&app, Method::GET, "/hubs/search?query=OP%20Coll&limit=10").await;
    let shelf = body["MediaContainer"]["Hub"]
        .as_array()
        .unwrap()
        .iter()
        .find(|hub| hub["hubIdentifier"] == "collection")
        .unwrap()
        .clone();
    let entry = &shelf["Directory"][0];
    // Captured on GET /hubs/search?query=OP%20Coll: the entry for OP Coll A, in this order.
    assert_eq!(
        keys(entry),
        [
            "key",
            "librarySectionID",
            "librarySectionKey",
            "librarySectionTitle",
            "librarySectionType",
            "reason",
            "reasonID",
            "reasonTitle",
            "score",
            "type",
            "id",
            "filter",
            "tag",
            "tagType",
            "count",
            "guid"
        ]
    );
    assert_eq!(
        entry["key"],
        format!("/library/sections/27/all?collection={number}")
    );
    assert_eq!(entry["score"], "0.52000");
    assert_eq!(entry["type"], "tag");
    assert_eq!(entry["tagType"], 2);
    assert_eq!(entry["count"], 1);
    assert_eq!(entry["guid"], created["guid"]);
}
