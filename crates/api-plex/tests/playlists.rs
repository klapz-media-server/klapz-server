//! `/playlists`, against shapes captured on the reference (1.43.5) with playlists made of its
//! test libraries: Soul in library 27, the two episodes of Bull's season 5 in 28.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use http_body_util::BodyExt;
use open_plex_api::{build_schema, ApiContext, ApiSchema};
use open_plex_api_plex::{router, PlexState};
use open_plex_core::access::{LibraryAccess, LibraryRole};
use open_plex_core::metadata::{ItemKind, MetadataItem};
use open_plex_core::testing::{InMemoryLibrary, InMemoryMetadata, InMemoryPlaylists};
use open_plex_core::{
    Accounts, LibrarySection, Principal, RatingKey, Scope, SectionId, SectionKind,
    ServerCapabilities, ServerIdentity, UserId,
};
use serde_json::Value;
use tower::ServiceExt;

const MACHINE: &str = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
const FILM: i64 = 577_986;
const SHOW: i64 = 577_987;
const SEASON: i64 = 577_988;
const EPISODE_1: i64 = 578_173;
const EPISODE_2: i64 = 577_990;

fn section(id: i64, kind: SectionKind, title: &str) -> LibrarySection {
    LibrarySection {
        id: SectionId::new(id),
        kind,
        title: title.to_owned(),
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
    parent: Option<i64>,
    title: &str,
    duration_ms: Option<i64>,
) -> MetadataItem {
    MetadataItem {
        id: RatingKey::new(id),
        section_id: SectionId::new(section),
        parent_id: parent.map(RatingKey::new),
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
        position: Some(1),
        year: None,
        released_on: None,
        duration_ms,
        rating: None,
        audience_rating: None,
        added_at: 1_610_180_026,
        updated_at: 1_794_332_579,
    }
}

fn schema() -> ApiSchema {
    let mut second = item(
        EPISODE_2,
        28,
        ItemKind::Episode,
        Some(SEASON),
        "Episode 2",
        Some(2_467_480),
    );
    second.position = Some(2);
    let items = vec![
        item(FILM, 27, ItemKind::Movie, None, "Soul", Some(6_031_568)),
        item(SHOW, 28, ItemKind::Show, None, "Bull", None),
        item(SEASON, 28, ItemKind::Season, Some(SHOW), "Season 5", None),
        item(
            EPISODE_1,
            28,
            ItemKind::Episode,
            Some(SEASON),
            "Episode 1",
            Some(2_467_160),
        ),
        second,
    ];
    let identity = ServerIdentity::new(MACHINE, "0.1.0-dev");
    build_schema(ApiContext {
        capabilities: ServerCapabilities::new(identity, 0),
        library: std::sync::Arc::new(InMemoryLibrary::new(vec![
            section(27, SectionKind::Movie, "open-plex tests"),
            section(28, SectionKind::Show, "open-plex tests series"),
        ])),
        metadata: std::sync::Arc::new(InMemoryMetadata::new(items.clone())),
        playback: std::sync::Arc::new(open_plex_core::testing::InMemoryPlayback::default()),
        play_queues: std::sync::Arc::new(open_plex_core::testing::InMemoryPlayQueues::default()),
        playlists: std::sync::Arc::new(InMemoryPlaylists::new(items)),
        collections: std::sync::Arc::new(open_plex_core::testing::InMemoryCollections::default()),
        access: std::sync::Arc::new(open_plex_core::testing::InMemoryAccess::default()),
        directory: std::sync::Arc::new(open_plex_core::testing::StubDirectory::default()),
        watch: std::sync::Arc::new(open_plex_core::testing::InMemoryWatch::default()),
        streams: std::sync::Arc::new(open_plex_core::testing::StubStreams::default()),
        session_locks: std::sync::Arc::default(),
    })
}

fn app_as(schema: &ApiSchema, caller: Principal) -> axum::Router {
    let identity = ServerIdentity::new(MACHINE, "0.1.0-dev");
    router(PlexState::new(
        identity,
        schema.clone(),
        Accounts::new(Vec::new()),
    ))
    .layer(axum::Extension(caller))
}

fn operator(schema: &ApiSchema) -> axum::Router {
    app_as(schema, Principal::operator("owner"))
}

/// A user given the series library alone.
fn user(schema: &ApiSchema, id: i64) -> axum::Router {
    app_as(
        schema,
        Principal {
            name: format!("user{id}"),
            scope: Scope::User,
            libraries: LibraryAccess::Only(
                std::iter::once((SectionId::new(28), LibraryRole::Viewer)).collect(),
            ),
            user: Some(UserId::new(id)),
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

fn uri_of(path: &str) -> String {
    let uri = format!("server://{MACHINE}/com.plexapp.plugins.library{path}");
    form_urlencoded::byte_serialize(uri.as_bytes()).collect()
}

async fn create(app: &axum::Router, title: &str, path: &str, smart: bool) -> Value {
    let smart = u8::from(smart);
    let title: String = form_urlencoded::byte_serialize(title.as_bytes()).collect();
    let body = json(
        app,
        Method::POST,
        &format!(
            "/playlists?type=video&title={title}&smart={smart}&uri={}",
            uri_of(path)
        ),
    )
    .await;
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
async fn a_created_playlist_answers_as_the_reference_does() {
    let schema = schema();
    let app = operator(&schema);
    let created = create(
        &app,
        "OP Dumb",
        &format!("/library/metadata/{FILM},{EPISODE_1}"),
        false,
    )
    .await;
    // Captured on POST /playlists: no play count, no rule, in this order.
    assert_eq!(
        keys(&created),
        [
            "ratingKey",
            "key",
            "guid",
            "type",
            "title",
            "summary",
            "smart",
            "playlistType",
            "composite",
            "duration",
            "leafCount",
            "addedAt",
            "updatedAt"
        ]
    );
    let key = created["ratingKey"].as_str().unwrap();
    assert_eq!(created["key"], format!("/playlists/{key}/items"));
    assert_eq!(created["type"], "playlist");
    assert_eq!(created["smart"], false);
    assert_eq!(created["playlistType"], "video");
    assert_eq!(created["leafCount"], 2);
    // 6,031,568 ms and 2,467,160 ms: whole seconds, as the reference counts them.
    assert_eq!(created["duration"], 8_498_000);
    assert!(created["guid"]
        .as_str()
        .unwrap()
        .starts_with("com.plexapp.agents.none://"));
    assert_eq!(
        created["composite"],
        format!("/playlists/{key}/composite/{}", created["updatedAt"])
    );
}

#[tokio::test]
async fn the_listing_adds_the_play_count_and_puts_smart_playlists_first() {
    let schema = schema();
    let app = operator(&schema);
    create(&app, "AA Nothing", "/library/metadata/999999", false).await;
    create(&app, "ZZ Smart", "/library/sections/28/all?type=4", true).await;
    create(&app, "OP Lib", "/library/sections/28/all?type=4", false).await;

    let listing = json(&app, Method::GET, "/playlists").await;
    let container = &listing["MediaContainer"];
    let titles: Vec<&str> = container["Metadata"]
        .as_array()
        .unwrap()
        .iter()
        .map(|playlist| playlist["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["ZZ Smart", "AA Nothing", "OP Lib"]);
    let smart = &container["Metadata"][0];
    assert_eq!(
        keys(smart),
        [
            "ratingKey",
            "key",
            "guid",
            "type",
            "title",
            "summary",
            "smart",
            "playlistType",
            "composite",
            "icon",
            "viewCount",
            "lastViewedAt",
            "leafCount",
            "addedAt",
            "updatedAt"
        ]
    );
    assert_eq!(smart["icon"], "playlist://image.smart");
    assert_eq!(smart["viewCount"], 1);
    // An empty playlist has no image and no duration.
    let empty = &container["Metadata"][1];
    assert!(empty.get("composite").is_none(), "{empty}");
    assert!(empty.get("duration").is_none(), "{empty}");
    assert_eq!(empty["leafCount"], 0);

    let paged = json(
        &app,
        Method::GET,
        "/playlists?X-Plex-Container-Start=1&X-Plex-Container-Size=1",
    )
    .await;
    assert_eq!(paged["MediaContainer"]["size"], 1);
    assert_eq!(paged["MediaContainer"]["totalSize"], 3);
    assert_eq!(paged["MediaContainer"]["offset"], 1);

    let smart_only = json(&app, Method::GET, "/playlists?smart=1").await;
    assert_eq!(smart_only["MediaContainer"]["size"], 1);
    let audio = json(&app, Method::GET, "/playlists?playlistType=audio").await;
    assert_eq!(audio["MediaContainer"], serde_json::json!({ "size": 0 }));
    let (status, _) = send(&app, Method::GET, "/playlists?sort=duration:desc", true).await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
}

#[tokio::test]
async fn items_carry_their_library_and_their_entry() {
    let schema = schema();
    let app = operator(&schema);
    let created = create(
        &app,
        "OP Dumb",
        &format!("/library/metadata/{FILM},{EPISODE_1}"),
        false,
    )
    .await;
    let key = created["ratingKey"].as_str().unwrap();
    let body = json(&app, Method::GET, &format!("/playlists/{key}/items")).await;
    let container = &body["MediaContainer"];
    let attributes: Vec<&str> = keys(container)
        .into_iter()
        .filter(|name| *name != "Metadata")
        .collect();
    assert_eq!(
        attributes,
        [
            "size",
            "composite",
            "duration",
            "leafCount",
            "playlistType",
            "ratingKey",
            "smart",
            "title"
        ]
    );
    // Seconds here, where the playlist's own attribute is in milliseconds.
    assert_eq!(container["duration"], 8_498);
    assert_eq!(
        rating_keys(container),
        [FILM.to_string(), EPISODE_1.to_string()]
    );
    let film = &container["Metadata"][0];
    assert_eq!(film["librarySectionID"], 27);
    assert_eq!(film["librarySectionKey"], "/library/sections/27");
    let film_keys = keys(film);
    let entry = film_keys
        .iter()
        .position(|name| *name == "playlistItemID")
        .unwrap();
    assert_eq!(film_keys[entry + 1], "duration");

    let (status, xml) = send(&app, Method::GET, &format!("/playlists/{key}/items"), false).await;
    assert_eq!(status, StatusCode::OK);
    assert!(xml.contains("<Video ratingKey=\"577986\""), "{xml}");
    assert!(xml.contains("smart=\"0\""), "{xml}");
}

#[tokio::test]
async fn adding_drops_repeats_and_counts_what_was_asked() {
    let schema = schema();
    let app = operator(&schema);
    let created = create(&app, "OP", &format!("/library/metadata/{FILM}"), false).await;
    let key = created["ratingKey"].as_str().unwrap();
    // The series stands for its two episodes.
    let added = json(
        &app,
        Method::PUT,
        &format!(
            "/playlists/{key}/items?uri={}",
            uri_of(&format!("/library/metadata/{SHOW}"))
        ),
    )
    .await;
    assert_eq!(added["MediaContainer"]["leafCountAdded"], 2);
    assert_eq!(added["MediaContainer"]["leafCountRequested"], 2);
    let again = json(
        &app,
        Method::PUT,
        &format!(
            "/playlists/{key}/items?uri={}",
            uri_of(&format!("/library/metadata/{FILM}"))
        ),
    )
    .await;
    assert_eq!(again["MediaContainer"]["leafCountAdded"], 0);
    assert_eq!(again["MediaContainer"]["leafCountRequested"], 1);
    assert_eq!(again["MediaContainer"]["Metadata"][0]["leafCount"], 3);
}

async fn entries(app: &axum::Router, key: &str) -> Vec<(i64, String)> {
    let body = json(app, Method::GET, &format!("/playlists/{key}/items")).await;
    body["MediaContainer"]["Metadata"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            (
                item["playlistItemID"].as_i64().unwrap(),
                item["ratingKey"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

#[tokio::test]
async fn entries_move_after_another_or_first_and_leave() {
    let schema = schema();
    let app = operator(&schema);
    let created = create(
        &app,
        "OP",
        &format!("/library/metadata/{FILM},{EPISODE_1},{EPISODE_2}"),
        false,
    )
    .await;
    let key = created["ratingKey"].as_str().unwrap();
    let ids: Vec<i64> = entries(&app, key).await.iter().map(|(id, _)| *id).collect();

    json(
        &app,
        Method::PUT,
        &format!("/playlists/{key}/items/{}/move?after={}", ids[2], ids[0]),
    )
    .await;
    let order: Vec<String> = entries(&app, key).await.into_iter().map(|e| e.1).collect();
    assert_eq!(
        order,
        [
            FILM.to_string(),
            EPISODE_2.to_string(),
            EPISODE_1.to_string()
        ]
    );
    json(
        &app,
        Method::PUT,
        &format!("/playlists/{key}/items/{}/move", ids[1]),
    )
    .await;
    let order: Vec<String> = entries(&app, key).await.into_iter().map(|e| e.1).collect();
    assert_eq!(order[0], EPISODE_1.to_string());

    let (status, _) = send(
        &app,
        Method::PUT,
        &format!("/playlists/{key}/items/999999/move"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let removed = json(
        &app,
        Method::DELETE,
        &format!("/playlists/{key}/items/{}", ids[0]),
    )
    .await;
    assert_eq!(removed["MediaContainer"]["Metadata"][0]["leafCount"], 2);
    let (status, _) = send(
        &app,
        Method::DELETE,
        &format!("/playlists/{key}/items/{}", ids[0]),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let cleared = json(&app, Method::DELETE, &format!("/playlists/{key}/items")).await;
    let playlist = &cleared["MediaContainer"]["Metadata"][0];
    assert_eq!(playlist["leafCount"], 0);
    assert!(playlist.get("composite").is_none());
    let empty = json(&app, Method::GET, &format!("/playlists/{key}/items")).await;
    // The container keeps its image even empty, as captured.
    assert!(empty["MediaContainer"].get("composite").is_some());
    assert!(empty["MediaContainer"].get("Metadata").is_none());
}

#[tokio::test]
async fn an_edit_renames_and_locks_and_answers_an_empty_200() {
    let schema = schema();
    let app = operator(&schema);
    let created = create(&app, "OP Dumb", "/library/metadata/999999", false).await;
    let key = created["ratingKey"].as_str().unwrap();
    let (status, body) = send(
        &app,
        Method::PUT,
        &format!("/playlists/{key}?title=OP%20Renamed&summary=Hello"),
        true,
    )
    .await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, ""));
    send(
        &app,
        Method::PUT,
        &format!("/playlists/{key}?title.value=OP%20Renamed2&title.locked=1"),
        true,
    )
    .await;
    let read = json(&app, Method::GET, &format!("/playlists/{key}")).await;
    let playlist = &read["MediaContainer"]["Metadata"][0];
    assert_eq!(playlist["title"], "OP Renamed2");
    assert_eq!(playlist["summary"], "Hello");
    assert_eq!(
        playlist["Field"],
        serde_json::json!([{ "locked": true, "name": "title" }])
    );
}

#[tokio::test]
async fn a_smart_playlist_lists_its_rule_and_refuses_entry_changes() {
    let schema = schema();
    let app = operator(&schema);
    let created = create(
        &app,
        "OP Smart",
        "/library/sections/28/all?type=4&sort=titleSort:desc",
        true,
    )
    .await;
    let key = created["ratingKey"].as_str().unwrap();
    assert_eq!(created["smart"], true);
    assert_eq!(created["leafCount"], 2);

    let lookup = json(&app, Method::GET, &format!("/playlists/{key}")).await;
    let playlist = &lookup["MediaContainer"]["Metadata"][0];
    assert_eq!(keys(playlist)[0], "content");
    assert_eq!(
        playlist["content"],
        "library://x/directory/%2Flibrary%2Fsections%2F28%2Fall%3Ftype%3D4%26sort%3DtitleSort%3Adesc"
    );

    let items = json(&app, Method::GET, &format!("/playlists/{key}/items")).await;
    assert_eq!(
        rating_keys(&items["MediaContainer"]),
        [EPISODE_2.to_string(), EPISODE_1.to_string()]
    );
    assert!(items["MediaContainer"]["Metadata"][0]
        .get("playlistItemID")
        .is_none());

    for (method, path) in [
        (Method::DELETE, format!("/playlists/{key}/items")),
        (Method::PUT, format!("/playlists/{key}/items/1/move")),
        (
            Method::PUT,
            format!(
                "/playlists/{key}/items?uri={}",
                uri_of(&format!("/library/metadata/{FILM}"))
            ),
        ),
    ] {
        let (status, _) = send(&app, method, &path, true).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path}");
    }

    // A new library listing replaces the rule.
    json(
        &app,
        Method::PUT,
        &format!(
            "/playlists/{key}/items?uri={}",
            uri_of("/library/sections/28/all?type=4&sort=titleSort")
        ),
    )
    .await;
    let items = json(&app, Method::GET, &format!("/playlists/{key}/items")).await;
    assert_eq!(
        rating_keys(&items["MediaContainer"]),
        [EPISODE_1.to_string(), EPISODE_2.to_string()]
    );

    let generators = json(&app, Method::GET, &format!("/playlists/{key}/generators")).await;
    assert_eq!(generators["MediaContainer"]["size"], 1);
    assert_eq!(
        generators["MediaContainer"]["PlayQueueGenerator"][0]["uri"],
        "library://x/directory/%2Flibrary%2Fsections%2F28%2Fall%3Ftype%3D4%26sort%3DtitleSort"
    );
}

#[tokio::test]
async fn a_smart_playlist_with_a_filter_it_cannot_honor_is_refused() {
    let schema = schema();
    let app = operator(&schema);
    let (status, _) = send(
        &app,
        Method::POST,
        &format!(
            "/playlists?type=video&title=x&smart=1&uri={}",
            uri_of("/library/sections/28/all?type=4&label=12")
        ),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    let listing = json(&app, Method::GET, "/playlists").await;
    assert_eq!(listing["MediaContainer"]["size"], 0);
}

#[tokio::test]
async fn generators_are_one_per_entry_and_none_exists_by_id() {
    let schema = schema();
    let app = operator(&schema);
    let created = create(
        &app,
        "OP",
        &format!("/library/metadata/{FILM},{EPISODE_1}"),
        false,
    )
    .await;
    let key = created["ratingKey"].as_str().unwrap();
    let body = json(&app, Method::GET, &format!("/playlists/{key}/generators")).await;
    let generators = body["MediaContainer"]["PlayQueueGenerator"]
        .as_array()
        .unwrap();
    assert_eq!(generators.len(), 2);
    assert_eq!(generators[0]["metadataItemID"], FILM);
    assert_eq!(generators[0]["playlistID"].to_string(), key);
    assert_eq!(generators[0]["type"], -1);
    let ids: Vec<i64> = entries(&app, key).await.iter().map(|(id, _)| *id).collect();
    for path in [
        format!("/playlists/{key}/items/{}", ids[0]),
        format!("/playlists/{key}/items/{}/items", ids[0]),
    ] {
        let (status, _) = send(&app, Method::GET, &path, true).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
    }
    let (status, _) = send(
        &app,
        Method::PUT,
        &format!("/playlists/{key}/items/{}/{FILM}/reprocess", ids[0]),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn what_the_reference_refuses_is_refused() {
    let schema = schema();
    let app = operator(&schema);
    for uri in [
        "/playlists".to_owned(),
        format!(
            "/playlists?type=video&smart=0&uri={}",
            uri_of("/library/metadata/577986")
        ),
        format!(
            "/playlists?title=x&smart=0&uri={}",
            uri_of("/library/metadata/577986")
        ),
        "/playlists?type=video&title=x&smart=0".to_owned(),
        // A video item does not go into an audio playlist; nothing is created.
        format!(
            "/playlists?type=audio&title=x&smart=0&uri={}",
            uri_of("/library/metadata/577986")
        ),
    ] {
        let (status, _) = send(&app, Method::POST, &uri, true).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
    }
    assert_eq!(
        json(&app, Method::GET, "/playlists").await["MediaContainer"]["size"],
        0
    );
    for (method, uri) in [
        (Method::GET, "/playlists/999999"),
        (Method::GET, "/playlists/abc"),
        (Method::GET, "/playlists/999999/items"),
        (Method::GET, "/playlists/999999/generators"),
        (Method::DELETE, "/playlists/999999"),
        (Method::DELETE, "/playlists/999999/items"),
        (Method::PUT, "/playlists/999999?title=x"),
    ] {
        let (status, _) = send(&app, method, uri, true).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
    }
}

#[tokio::test]
async fn an_upload_names_a_path_and_is_refused() {
    let schema = schema();
    let app = operator(&schema);
    let (status, _) = send(
        &app,
        Method::POST,
        "/playlists/upload?path=C%3A%5Cplaylists",
        true,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(&app, Method::POST, "/playlists/upload", true).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn deleting_answers_no_content_and_then_not_found() {
    let schema = schema();
    let app = operator(&schema);
    let created = create(&app, "OP", &format!("/library/metadata/{FILM}"), false).await;
    let key = created["ratingKey"].as_str().unwrap();
    let (status, _) = send(&app, Method::DELETE, &format!("/playlists/{key}"), true).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = send(&app, Method::GET, &format!("/playlists/{key}"), true).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_user_sees_neither_the_operators_playlists_nor_items_it_was_not_given() {
    let schema = schema();
    let app = operator(&schema);
    let created = create(
        &app,
        "Operator's",
        &format!("/library/metadata/{FILM},{EPISODE_1}"),
        false,
    )
    .await;
    let key = created["ratingKey"].as_str().unwrap();
    let viewer = user(&schema, 7);
    for (method, uri) in [
        (Method::GET, format!("/playlists/{key}")),
        (Method::GET, format!("/playlists/{key}/items")),
        (Method::DELETE, format!("/playlists/{key}")),
    ] {
        let (status, _) = send(&viewer, method, &uri, true).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
    }
    let listing = json(&viewer, Method::GET, "/playlists").await;
    assert_eq!(listing["MediaContainer"]["size"], 0);

    // Its own playlist, asked to hold a film of a library it was not given, holds the episode.
    let own = create(
        &viewer,
        "Mine",
        &format!("/library/metadata/{FILM},{EPISODE_1}"),
        false,
    )
    .await;
    assert_eq!(own["leafCount"], 1);
}

#[tokio::test]
async fn a_play_queue_plays_a_playlist_and_counts_the_play() {
    let schema = schema();
    let app = operator(&schema);
    let created = create(
        &app,
        "OP Lib",
        &format!("/library/metadata/{EPISODE_1},{EPISODE_2}"),
        false,
    )
    .await;
    let key = created["ratingKey"].as_str().unwrap();
    for uri in [
        format!("/playQueues?type=video&playlistID={key}"),
        format!(
            "/playQueues?type=video&uri={}",
            uri_of(&format!("/playlists/{key}/items"))
        ),
    ] {
        let queue = json(&app, Method::POST, &uri).await;
        let container = &queue["MediaContainer"];
        assert_eq!(container["playQueuePlaylistID"].to_string(), key);
        assert_eq!(container["playQueuePlaylistTitle"], "OP Lib");
        assert_eq!(container["playQueueTotalCount"], 2);
        assert!(container.get("playQueueSourceURI").is_none());
    }
    let listing = json(&app, Method::GET, "/playlists").await;
    assert_eq!(listing["MediaContainer"]["Metadata"][0]["viewCount"], 3);
    let (status, _) = send(
        &app,
        Method::POST,
        "/playQueues?type=video&playlistID=999999",
        true,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
