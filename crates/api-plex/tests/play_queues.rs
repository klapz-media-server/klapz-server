//! `/playQueues`, replaying sequences captured on the reference (1.43.5) with queues made of its
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
    LibrarySection, Principal, RatingKey, SectionId, SectionKind, ServerCapabilities,
    ServerIdentity, UserId,
};
use serde_json::Value;
use tower::ServiceExt;

const MACHINE: &str = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
const FILM: i64 = 577_986;
const SHOW: i64 = 577_987;
const SEASON: i64 = 577_988;
const EPISODE_1: i64 = 578_173;
const EPISODE_2: i64 = 577_990;
const CLIENT: &str = "openplex-pq-capture";

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

fn item(id: i64, section: i64, kind: ItemKind, parent: Option<i64>, title: &str) -> MetadataItem {
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
        duration_ms: Some(2_467_160),
        rating: None,
        audience_rating: None,
        added_at: 1_610_180_026,
        updated_at: 1_794_332_579,
        refreshed_at: None,
    }
}

fn schema() -> ApiSchema {
    let mut second = item(EPISODE_2, 28, ItemKind::Episode, Some(SEASON), "Episode 2");
    second.position = Some(2);
    let items = vec![
        item(FILM, 27, ItemKind::Movie, None, "Soul"),
        item(SHOW, 28, ItemKind::Show, None, "Bull"),
        item(SEASON, 28, ItemKind::Season, Some(SHOW), "Season 5"),
        item(EPISODE_1, 28, ItemKind::Episode, Some(SEASON), "Episode 1"),
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
        access: std::sync::Arc::new(open_plex_core::testing::InMemoryAccess::default()),
        directory: std::sync::Arc::new(open_plex_core::testing::StubDirectory::default()),
        watch: std::sync::Arc::new(open_plex_core::testing::InMemoryWatch::default()),
        streams: std::sync::Arc::new(open_plex_core::testing::StubStreams::default()),
        collections: std::sync::Arc::new(open_plex_core::testing::InMemoryCollections::default()),
        session_locks: std::sync::Arc::default(),
        scans: std::sync::Arc::new(open_plex_core::testing::RecordedScans::default()),
    })
}

fn app_as(schema: &ApiSchema, caller: Principal) -> axum::Router {
    let identity = ServerIdentity::new(MACHINE, "0.1.0-dev");
    router(PlexState::new(identity, schema.clone())).layer(axum::Extension(caller))
}

fn owner(schema: &ApiSchema) -> axum::Router {
    app_as(schema, open_plex_core::testing::owner())
}

fn user(schema: &ApiSchema, id: i64) -> axum::Router {
    app_as(
        schema,
        Principal {
            name: format!("user{id}"),
            role: open_plex_core::ServerRole::User,
            libraries: LibraryAccess::Only(
                [
                    (SectionId::new(27), LibraryRole::Viewer),
                    (SectionId::new(28), LibraryRole::Viewer),
                ]
                .into_iter()
                .collect(),
            ),
            user: UserId::new(id),
            server_identifier: None,
        },
    )
}

async fn send_as(
    app: &axum::Router,
    method: Method,
    uri: &str,
    client: &str,
    accept_json: bool,
) -> (StatusCode, String) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("X-Plex-Client-Identifier", client);
    if accept_json {
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

async fn status(app: &axum::Router, method: Method, uri: &str) -> StatusCode {
    send_as(app, method, uri, CLIENT, true).await.0
}

/// The container of a `200`.
async fn container(app: &axum::Router, method: Method, uri: &str) -> Value {
    let (status, body) = send_as(app, method, uri, CLIENT, true).await;
    assert_eq!(status, StatusCode::OK, "{uri}: {body}");
    let body: Value = serde_json::from_str(&body).unwrap();
    body["MediaContainer"].clone()
}

fn uri_of(path: &str) -> String {
    let uri = format!("server://{MACHINE}/com.plexapp.plugins.library{path}");
    form_urlencoded::byte_serialize(uri.as_bytes()).collect()
}

fn of_item(key: i64) -> String {
    uri_of(&format!("/library/metadata/{key}"))
}

async fn create(app: &axum::Router, key: i64, extra: &str) -> Value {
    container(
        app,
        Method::POST,
        &format!("/playQueues?type=video&uri={}{extra}", of_item(key)),
    )
    .await
}

fn queue_id(container: &Value) -> i64 {
    container["playQueueID"].as_i64().unwrap()
}

/// `(playQueueItemID, ratingKey)` of each entry shown, in order.
fn entries(container: &Value) -> Vec<(i64, i64)> {
    container["Metadata"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    (
                        item["playQueueItemID"].as_i64().unwrap(),
                        item["ratingKey"].as_str().unwrap().parse().unwrap(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

fn rating_keys(container: &Value) -> Vec<i64> {
    entries(container).into_iter().map(|(_, key)| key).collect()
}

fn ids(container: &Value) -> Vec<i64> {
    entries(container).into_iter().map(|(id, _)| id).collect()
}

fn attributes(container: &Value) -> Vec<&str> {
    container
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect()
}

async fn add(app: &axum::Router, queue: i64, key: i64, next: bool) -> Value {
    let next = if next { "&next=1" } else { "" };
    container(
        app,
        Method::PUT,
        &format!("/playQueues/{queue}?uri={}{next}", of_item(key)),
    )
    .await
}

// Queue 413711 on the reference: Bull, then Soul at the end, Soul next, Episode 2 at the end.
#[tokio::test]
async fn additions_go_where_the_reference_puts_them() {
    let schema = schema();
    let app = owner(&schema);
    let created = create(&app, SHOW, "").await;
    let queue = queue_id(&created);
    let [first, _] = ids(&created)[..] else {
        panic!("two episodes")
    };

    let added = add(&app, queue, FILM, false).await;
    assert_eq!(rating_keys(&added), [EPISODE_1, FILM, EPISODE_2]);
    let soul = ids(&added)[1];
    assert_eq!(added["playQueueLastAddedItemID"], soul);
    assert_eq!(added["playQueueVersion"], 2);
    // Captured on the PUT: the area's end comes right after the queue's id.
    assert_eq!(
        attributes(&added),
        [
            "size",
            "identifier",
            "mediaTagPrefix",
            "mediaTagVersion",
            "playQueueID",
            "playQueueLastAddedItemID",
            "playQueueSelectedItemID",
            "playQueueSelectedItemOffset",
            "playQueueSelectedMetadataItemID",
            "playQueueShuffled",
            "playQueueSourceURI",
            "playQueueTotalCount",
            "playQueueVersion",
            "Metadata",
        ]
    );

    let next = add(&app, queue, FILM, true).await;
    assert_eq!(rating_keys(&next), [EPISODE_1, FILM, FILM, EPISODE_2]);
    assert_eq!(next["playQueueLastAddedItemID"], soul, "next leaves it");
    assert_eq!(next["playQueueVersion"], 3);

    let last = add(&app, queue, EPISODE_2, false).await;
    assert_eq!(
        rating_keys(&last),
        [EPISODE_1, FILM, FILM, EPISODE_2, EPISODE_2]
    );
    assert_eq!(last["playQueueLastAddedItemID"], ids(&last)[3]);
    assert_eq!(last["playQueueSelectedItemID"], first);
    assert_eq!(last["playQueueVersion"], 4);
    assert_eq!(last["playQueueTotalCount"], 5);
}

#[tokio::test]
async fn adding_a_series_queues_its_episodes_and_nothing_adds_nothing() {
    let schema = schema();
    let app = owner(&schema);
    let queue = queue_id(&create(&app, FILM, "").await);
    let added = add(&app, queue, SHOW, true).await;
    assert_eq!(rating_keys(&added), [FILM, EPISODE_1, EPISODE_2]);
    assert_eq!(added["playQueueLastAddedItemID"], ids(&added)[2]);

    for uri in [
        format!("/playQueues/{queue}?uri=nonsense"),
        format!("/playQueues/{queue}?uri={}", of_item(99_999_999)),
        format!("/playQueues/{queue}?playlistID=99999999"),
    ] {
        let unchanged = container(&app, Method::PUT, &uri).await;
        assert_eq!(unchanged["playQueueVersion"], 2, "{uri}");
        assert_eq!(unchanged["playQueueTotalCount"], 3, "{uri}");
    }
    assert_eq!(
        status(&app, Method::PUT, &format!("/playQueues/{queue}")).await,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn an_empty_queue_plays_what_is_added_first() {
    let schema = schema();
    let app = owner(&schema);
    let empty = container(&app, Method::POST, "/playQueues?type=video&uri=nonsense").await;
    assert!(empty.get("playQueueSelectedItemID").is_none());
    let queue = queue_id(&empty);
    assert_eq!(
        status(&app, Method::PUT, &format!("/playQueues/{queue}/shuffle")).await,
        StatusCode::NOT_FOUND
    );
    let added = add(&app, queue, SHOW, true).await;
    assert_eq!(added["playQueueSelectedItemID"], ids(&added)[0]);
    assert_eq!(added["playQueueLastAddedItemID"], ids(&added)[1]);
    assert_eq!(added["playQueueSourceURI"], "nonsense");
}

#[tokio::test]
async fn moves_follow_the_reference_s_reading_of_after() {
    let schema = schema();
    let app = owner(&schema);
    let queue = queue_id(&create(&app, SHOW, "").await);
    let added = add(&app, queue, FILM, false).await;
    let [e1, soul, e2] = ids(&added)[..] else {
        panic!("three entries")
    };
    let path = |entry: i64, after: &str| format!("/playQueues/{queue}/items/{entry}/move{after}");

    // Captured on queue 413740: `-1` is first, and so is no `after` at all.
    let first = container(&app, Method::PUT, &path(e2, "?after=-1")).await;
    assert_eq!(ids(&first), [e2, e1, soul]);
    assert_eq!(first["playQueueSelectedItemOffset"], 1);
    assert_eq!(first["playQueueVersion"], 3);
    // `after` naming no entry puts the entry second; moving the area's end shortens it.
    let second = container(&app, Method::PUT, &path(soul, "?after=0")).await;
    assert_eq!(ids(&second), [e2, soul, e1]);
    assert_eq!(second["playQueueLastAddedItemID"], e1);
    let after = container(&app, Method::PUT, &path(e2, &format!("?after={e1}"))).await;
    assert_eq!(ids(&after), [soul, e1, e2]);
    assert_eq!(after["playQueueLastAddedItemID"], e2, "moved after the end");
    let front = container(&app, Method::PUT, &path(e1, "")).await;
    assert_eq!(ids(&front), [e1, soul, e2]);

    assert_eq!(
        status(&app, Method::PUT, &path(e1, &format!("?after={e1}"))).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        status(&app, Method::PUT, &path(999, "")).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        status(&app, Method::PUT, &path(e1, "?after=abc")).await,
        StatusCode::BAD_REQUEST
    );
}

// Queues 413716 and 413717 on the reference.
#[tokio::test]
async fn a_removal_answers_with_the_container_as_it_was() {
    let schema = schema();
    let app = owner(&schema);
    let queue = queue_id(&create(&app, SHOW, "").await);
    add(&app, queue, FILM, false).await;
    let added = add(&app, queue, FILM, false).await;
    let [e1, a1, a2, e2] = ids(&added)[..] else {
        panic!("four entries")
    };
    let remove = |entry: i64| format!("/playQueues/{queue}/items/{entry}");

    let removed = container(&app, Method::DELETE, &remove(a2)).await;
    assert_eq!(ids(&removed), [e1, a1, e2]);
    assert_eq!(removed["playQueueTotalCount"], 4);
    assert_eq!(removed["playQueueVersion"], 3);
    assert_eq!(removed["playQueueLastAddedItemID"], a2);
    let read = container(&app, Method::GET, &format!("/playQueues/{queue}")).await;
    assert_eq!(read["playQueueLastAddedItemID"], a1, "the entry before it");
    assert_eq!(read["playQueueVersion"], 4);

    // The entry playing gone, the window around it is empty; the next entry plays.
    let selected = container(&app, Method::DELETE, &remove(e1)).await;
    assert_eq!(selected["size"], 0);
    assert!(selected.get("Metadata").is_none());
    assert_eq!(selected["playQueueSelectedItemID"], e1);
    assert_eq!(selected["playQueueSelectedItemOffset"], 0);
    assert!(selected.get("playQueueSelectedMetadataItemID").is_none());
    let read = container(&app, Method::GET, &format!("/playQueues/{queue}")).await;
    assert_eq!(read["playQueueSelectedItemID"], a1);

    assert_eq!(
        status(&app, Method::DELETE, &remove(e1)).await,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn clearing_keeps_the_entry_playing() {
    let schema = schema();
    let app = owner(&schema);
    let queue = queue_id(&create(&app, SHOW, "").await);
    let added = add(&app, queue, FILM, false).await;
    let cleared = container(&app, Method::DELETE, &format!("/playQueues/{queue}/items")).await;
    assert_eq!(ids(&cleared), [ids(&added)[0]]);
    assert_eq!(cleared["playQueueTotalCount"], 1);
    assert_eq!(cleared["playQueueVersion"], 3);
    assert!(cleared.get("playQueueLastAddedItemID").is_none());
    // Without its additions the queue can be shuffled again, rebuilt from its source.
    let shuffled = container(&app, Method::PUT, &format!("/playQueues/{queue}/shuffle")).await;
    assert_eq!(shuffled["playQueueTotalCount"], 2);
}

// Queue 413713 on the reference: the answer is the container before the reset, with nothing.
#[tokio::test]
async fn a_reset_answers_with_the_container_as_it_was() {
    let schema = schema();
    let app = owner(&schema);
    let created = container(
        &app,
        Method::POST,
        &format!(
            "/playQueues?type=video&uri={}&key=/library/metadata/{EPISODE_2}",
            uri_of("/library/sections/28/all?type=4")
        ),
    )
    .await;
    let queue = queue_id(&created);
    let [e1, e2] = ids(&created)[..] else {
        panic!("two episodes")
    };
    assert_eq!(created["playQueueSelectedItemID"], e2);

    let reset = container(&app, Method::PUT, &format!("/playQueues/{queue}/reset")).await;
    assert_eq!(reset["size"], 0);
    assert!(reset.get("Metadata").is_none());
    assert_eq!(reset["playQueueSelectedItemID"], e2);
    assert_eq!(reset["playQueueVersion"], 1);
    let read = container(&app, Method::GET, &format!("/playQueues/{queue}")).await;
    assert_eq!(read["playQueueSelectedItemID"], e1);
    assert_eq!(read["playQueueVersion"], 1);
}

#[tokio::test]
async fn shuffling_keeps_the_entry_playing_and_renews_the_others() {
    let schema = schema();
    let app = owner(&schema);
    let created = create(&app, SHOW, &format!("&key=/library/metadata/{EPISODE_2}")).await;
    let queue = queue_id(&created);
    let [e1, e2] = ids(&created)[..] else {
        panic!("two episodes")
    };
    let shuffled = container(&app, Method::PUT, &format!("/playQueues/{queue}/shuffle")).await;
    assert_eq!(shuffled["playQueueShuffled"], true);
    assert_eq!(ids(&shuffled)[0], e2);
    assert_eq!(shuffled["playQueueSelectedItemOffset"], 0);
    assert_ne!(ids(&shuffled)[1], e1, "a new entry");
    assert_eq!(shuffled["playQueueVersion"], 2);

    let restored = container(&app, Method::PUT, &format!("/playQueues/{queue}/unshuffle")).await;
    assert_eq!(restored["playQueueShuffled"], false);
    assert_eq!(rating_keys(&restored), [EPISODE_1, EPISODE_2]);
    assert_eq!(restored["playQueueSelectedItemID"], e2);
    assert_eq!(restored["playQueueSelectedItemOffset"], 1);
    assert_eq!(restored["playQueueVersion"], 3);

    add(&app, queue, FILM, false).await;
    for path in ["shuffle", "unshuffle"] {
        assert_eq!(
            status(&app, Method::PUT, &format!("/playQueues/{queue}/{path}")).await,
            StatusCode::NOT_FOUND,
            "{path} with an up next area"
        );
    }
}

#[tokio::test]
async fn a_player_keeps_one_queue_of_each_type() {
    let schema = schema();
    let app = owner(&schema);
    let create_for = |client: &'static str, kind: &'static str| {
        let app = app.clone();
        async move {
            let (status, body) = send_as(
                &app,
                Method::POST,
                &format!("/playQueues?type={kind}&uri={}", of_item(FILM)),
                client,
                true,
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            let body: Value = serde_json::from_str(&body).unwrap();
            queue_id(&body["MediaContainer"])
        }
    };
    let a1 = create_for("A", "video").await;
    let b1 = create_for("B", "video").await;
    let a2 = create_for("A", "video").await;
    let audio = create_for("A", "audio").await;
    let read = |queue: i64| format!("/playQueues/{queue}");
    assert_eq!(
        status(&app, Method::GET, &read(a1)).await,
        StatusCode::NOT_FOUND
    );
    for kept in [b1, a2, audio] {
        assert_eq!(status(&app, Method::GET, &read(kept)).await, StatusCode::OK);
    }
}

#[tokio::test]
async fn continuous_plays_an_episode_on_to_the_end_of_its_series() {
    let schema = schema();
    let app = owner(&schema);
    let first = create(&app, EPISODE_1, "&continuous=1").await;
    assert_eq!(rating_keys(&first), [EPISODE_1, EPISODE_2]);
    assert_eq!(
        first["playQueueSourceURI"],
        format!("library://x/item/%2Flibrary%2Fmetadata%2F{EPISODE_1}")
    );
    let last = create(&app, EPISODE_2, "&continuous=1").await;
    assert_eq!(rating_keys(&last), [EPISODE_2]);
    let alone = create(&app, EPISODE_1, "").await;
    assert_eq!(rating_keys(&alone), [EPISODE_1]);
    let film = create(&app, FILM, "&continuous=1").await;
    assert_eq!(rating_keys(&film), [FILM]);
}

// Queue 413729 on the reference: five entries, read around the last.
#[tokio::test]
async fn repeat_fills_the_window_from_the_other_end() {
    let schema = schema();
    let app = owner(&schema);
    let queue = queue_id(&create(&app, SHOW, "").await);
    for _ in 0..3 {
        add(&app, queue, FILM, false).await;
    }
    let all = container(&app, Method::GET, &format!("/playQueues/{queue}")).await;
    let order = ids(&all);
    let last = order[4];
    let around = |extra: &str| format!("/playQueues/{queue}?window=1&center={last}{extra}");
    let plain = container(&app, Method::GET, &around("")).await;
    assert_eq!(ids(&plain), [order[3], order[4]]);
    let wrapped = container(&app, Method::GET, &around("&repeat=1")).await;
    assert_eq!(ids(&wrapped), [order[3], order[4], order[0]]);
}

#[tokio::test]
async fn an_unknown_queue_is_not_found_everywhere() {
    let schema = schema();
    let app = owner(&schema);
    for (method, uri) in [
        (Method::GET, "/playQueues/99999999".to_owned()),
        (
            Method::PUT,
            format!("/playQueues/99999999?uri={}", of_item(FILM)),
        ),
        (Method::DELETE, "/playQueues/99999999/items".to_owned()),
        (Method::DELETE, "/playQueues/99999999/items/1".to_owned()),
        (Method::PUT, "/playQueues/99999999/items/1/move".to_owned()),
        (Method::PUT, "/playQueues/99999999/reset".to_owned()),
        (Method::PUT, "/playQueues/99999999/shuffle".to_owned()),
        (Method::PUT, "/playQueues/99999999/unshuffle".to_owned()),
    ] {
        assert_eq!(
            status(&app, method, &uri).await,
            StatusCode::NOT_FOUND,
            "{uri}"
        );
    }
}

#[tokio::test]
async fn another_viewer_s_queue_does_not_exist() {
    let schema = schema();
    let owner = user(&schema, 1);
    let created = create(&owner, SHOW, "").await;
    let queue = queue_id(&created);
    let entry = ids(&created)[0];
    let stranger = user(&schema, 2);
    for (method, uri) in [
        (Method::GET, format!("/playQueues/{queue}")),
        (
            Method::PUT,
            format!("/playQueues/{queue}?uri={}", of_item(FILM)),
        ),
        (Method::DELETE, format!("/playQueues/{queue}/items")),
        (Method::DELETE, format!("/playQueues/{queue}/items/{entry}")),
        (
            Method::PUT,
            format!("/playQueues/{queue}/items/{entry}/move"),
        ),
        (Method::PUT, format!("/playQueues/{queue}/reset")),
        (Method::PUT, format!("/playQueues/{queue}/shuffle")),
    ] {
        assert_eq!(
            status(&stranger, method, &uri).await,
            StatusCode::NOT_FOUND,
            "{uri}"
        );
    }
    let read = container(&owner, Method::GET, &format!("/playQueues/{queue}")).await;
    assert_eq!(read["playQueueVersion"], 1);
    assert_eq!(read["playQueueSelectedItemOffset"], 0);
}

#[tokio::test]
async fn xml_names_each_entry_after_its_type() {
    let schema = schema();
    let app = owner(&schema);
    let queue = queue_id(&create(&app, SHOW, "").await);
    let (status, body) = send_as(
        &app,
        Method::PUT,
        &format!("/playQueues/{queue}?uri={}", of_item(FILM)),
        CLIENT,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("playQueueLastAddedItemID=\""), "{body}");
    assert!(body.contains("<Video playQueueItemID=\""), "{body}");
    assert!(body.contains("playQueueShuffled=\"0\""), "{body}");
}
