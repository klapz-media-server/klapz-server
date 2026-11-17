//! `GET /library/sections/{id}/categories` against responses captured on the reference
//! (1.43.5): `tests/fixtures/categories`. The library is the reference's test library 28,
//! whose one show was given five genres for the capture. It has no art, so the reference
//! answers its fallback image, as this server always does.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use open_plex_api::{build_schema, ApiContext, ApiSchema};
use open_plex_api_plex::{router, PlexState};
use open_plex_core::metadata::{ItemKind, ItemTag, MetadataItem, Tag, TagKind};
use open_plex_core::testing::{InMemoryLibrary, InMemoryMetadata};
use open_plex_core::{
    LibrarySection, RatingKey, SectionId, SectionKind, ServerCapabilities, ServerIdentity, TagId,
};
use serde_json::Value;
use tower::ServiceExt;

const MACHINE: &str = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";

/// The reference's media flags version, which this server does not track.
const REFERENCE_MEDIA_TAG_VERSION: &str = "1791210760";

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/categories/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(path).unwrap()
}

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

fn show(id: i64, section: i64, title: &str) -> MetadataItem {
    MetadataItem {
        id: RatingKey::new(id),
        section_id: SectionId::new(section),
        parent_id: None,
        kind: ItemKind::Show,
        guid: format!("plex://show/{id}"),
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
        duration_ms: None,
        rating: None,
        audience_rating: None,
        added_at: 0,
        updated_at: 0,
        refreshed_at: None,
    }
}

fn genre(item: i64, id: i64, name: &str) -> ItemTag {
    ItemTag {
        item_id: RatingKey::new(item),
        tag: Tag {
            id: TagId::new(id),
            kind: TagKind::Genre,
            name: name.to_owned(),
            person: None,
        },
        role: None,
    }
}

/// Library 28 as captured, and library 29, a series library whose show has no genre.
fn schema() -> ApiSchema {
    let metadata = InMemoryMetadata::new(vec![show(577_987, 28, "Bull"), show(9, 29, "Bare")])
        .with_tags(vec![
            genre(577_987, 38, "Action"),
            genre(577_987, 123, "Fantasy"),
            genre(577_987, 988_939, "Kung Fu"),
            genre(577_987, 58, "Comedy"),
            genre(577_987, 988_940, "Zorglub"),
        ]);
    build_schema(ApiContext {
        capabilities: ServerCapabilities::new(ServerIdentity::new(MACHINE, "0.1.0-dev"), 0),
        library: std::sync::Arc::new(InMemoryLibrary::new(vec![
            section(28, SectionKind::Show, "open-plex tests series"),
            section(29, SectionKind::Show, "open-plex tests series"),
        ])),
        metadata: std::sync::Arc::new(metadata),
        playback: std::sync::Arc::new(open_plex_core::testing::InMemoryPlayback::default()),
        play_queues: std::sync::Arc::new(open_plex_core::testing::InMemoryPlayQueues::default()),
        playlists: std::sync::Arc::new(open_plex_core::testing::InMemoryPlaylists::default()),
        collections: std::sync::Arc::new(open_plex_core::testing::InMemoryCollections::default()),
        access: std::sync::Arc::new(open_plex_core::testing::InMemoryAccess::default()),
        directory: std::sync::Arc::new(open_plex_core::testing::StubDirectory::default()),
        watch: std::sync::Arc::new(open_plex_core::testing::InMemoryWatch::default()),
        streams: std::sync::Arc::new(open_plex_core::testing::StubStreams::default()),
        session_locks: std::sync::Arc::default(),
        scans: std::sync::Arc::new(open_plex_core::testing::RecordedScans::default()),
    })
}

struct Answer {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    body: String,
}

async fn get(uri: &str, accept: Option<&str>) -> Answer {
    let app = router(PlexState::new(
        ServerIdentity::new(MACHINE, "0.1.0-dev"),
        schema(),
    ))
    .layer(axum::Extension(open_plex_core::testing::owner()));
    let mut request = Request::builder().uri(uri);
    if let Some(accept) = accept {
        request = request.header(header::ACCEPT, accept);
    }
    let response = app
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    Answer {
        status,
        headers,
        body: String::from_utf8(body.to_vec()).unwrap(),
    }
}

/// Ours, with the reference's media flags version put in.
async fn json(uri: &str) -> Value {
    let answer = get(uri, Some("application/json")).await;
    assert_eq!(answer.status, StatusCode::OK, "{uri}: {}", answer.body);
    let mut body: Value = serde_json::from_str(&answer.body).unwrap();
    body["MediaContainer"]["mediaTagVersion"] =
        Value::from(REFERENCE_MEDIA_TAG_VERSION.parse::<i64>().unwrap());
    body
}

fn reference(name: &str) -> Value {
    serde_json::from_str(&fixture(name)).unwrap()
}

#[tokio::test]
async fn lists_the_genres_as_the_reference_does() {
    assert_eq!(
        json("/library/sections/28/categories").await,
        reference("28.json")
    );
}

#[tokio::test]
async fn writes_the_same_xml_as_the_reference() {
    let answer = get("/library/sections/28/categories", None).await;
    assert_eq!(answer.status, StatusCode::OK);
    let ours = answer.body.replace(
        "mediaTagVersion=\"0\"",
        &format!("mediaTagVersion=\"{REFERENCE_MEDIA_TAG_VERSION}\""),
    );
    assert_eq!(ours, fixture("28.xml"));
}

#[tokio::test]
async fn a_window_keeps_the_colors_of_the_whole_list() {
    let uri = "/library/sections/28/categories?X-Plex-Container-Start=1&X-Plex-Container-Size=2";
    assert_eq!(json(uri).await, reference("28_paged.json"));
    let answer = get(uri, None).await;
    assert_eq!(answer.headers["X-Plex-Container-Start"], "1");
    assert_eq!(answer.headers["X-Plex-Container-Total-Size"], "5");
}

#[tokio::test]
async fn ignores_parameters_as_the_reference_does() {
    // `type`, a filter and a sort change nothing there.
    assert_eq!(
        json("/library/sections/28/categories?type=4&genre=38&sort=titleSort:desc").await,
        reference("28.json")
    );
}

#[tokio::test]
async fn a_library_without_genres_lists_nothing() {
    let mut expected = reference("28_empty.json");
    // The empty library captured is 28 before the capture's genres were added.
    expected["MediaContainer"]["Meta"]["Type"][0]["key"] =
        Value::from("/library/sections/29/categories");
    assert_eq!(json("/library/sections/29/categories").await, expected);
}

#[tokio::test]
async fn an_unknown_library_is_absent() {
    for uri in [
        "/library/sections/999/categories",
        "/library/sections/abc/categories",
    ] {
        assert_eq!(get(uri, None).await.status, StatusCode::NOT_FOUND, "{uri}");
    }
}

#[tokio::test]
async fn each_category_opens_a_listing_that_answers() {
    let answer = get(
        "/library/sections/28/all?genre=988939",
        Some("application/json"),
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
    let body: Value = serde_json::from_str(&answer.body).unwrap();
    assert_eq!(body["MediaContainer"]["Metadata"][0]["ratingKey"], "577987");
}
