//! Backdrop colors on items and `/services/ultrablur/*`, against the reference (1.43.5).
//!
//! The colors are those the reference stores and publishes for *Retour vers le futur III*
//! (item 21812); the `/colors` bodies are its answer for an image, byte for byte, but for the
//! colors themselves.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use klapz_api::{build_schema, ApiContext, ApiSchema, GlobalId};
use klapz_api_plex::{router, PlexState};
use klapz_core::backdrop::{CornerColors, Rgb};
use klapz_core::metadata::{ItemKind, MetadataItem};
use klapz_core::testing::{InMemoryLibrary, InMemoryMetadata};
use klapz_core::{
    LibrarySection, RatingKey, SectionId, SectionKind, ServerCapabilities, ServerIdentity,
};
use serde_json::{json, Value};
use tower::ServiceExt;

const MACHINE: &str = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
const FILM: i64 = 21_812;
const UNANALYSED: i64 = 21_813;

fn film(id: i64) -> MetadataItem {
    MetadataItem {
        id: RatingKey::new(id),
        section_id: SectionId::new(4),
        parent_id: None,
        kind: ItemKind::Movie,
        guid: format!("plex://movie/{id}"),
        title: Some("Retour vers le futur III".to_owned()),
        title_sort: None,
        original_title: None,
        summary: None,
        tagline: None,
        studio: None,
        content_rating: None,
        content_rating_age: None,
        position: None,
        year: Some(1990),
        released_on: None,
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
    let metadata =
        InMemoryMetadata::new(vec![film(FILM), film(UNANALYSED)]).with_corner_colors(vec![(
            RatingKey::new(FILM),
            CornerColors {
                top_left: Rgb::new(0x1a, 0x2c, 0x53),
                top_right: Rgb::new(0x81, 0x50, 0x2b),
                bottom_right: Rgb::new(0x58, 0x42, 0x1c),
                bottom_left: Rgb::new(0x8f, 0x46, 0x1d),
            },
        )]);
    build_schema(ApiContext {
        capabilities: ServerCapabilities::new(ServerIdentity::new(MACHINE, "0.1.0-dev"), 0),
        library: std::sync::Arc::new(InMemoryLibrary::new(vec![LibrarySection {
            id: SectionId::new(4),
            kind: SectionKind::Movie,
            title: "VF - Films".to_owned(),
            title_sort: None,
            language: klapz_core::language::LanguageTag::parse("fr-FR").unwrap(),
            uuid: "2cd48489-54c0-41e9-ae62-f3dcb0f3c249".to_owned(),
            created_at: 0,
            updated_at: 0,
            scanned_at: None,
            content_changed_at: 0,
            hidden: false,
        }])),
        metadata: std::sync::Arc::new(metadata),
        playback: std::sync::Arc::new(klapz_core::testing::InMemoryPlayback::default()),
        play_queues: std::sync::Arc::new(klapz_core::testing::InMemoryPlayQueues::default()),
        playlists: std::sync::Arc::new(klapz_core::testing::InMemoryPlaylists::default()),
        collections: std::sync::Arc::new(klapz_core::testing::InMemoryCollections::default()),
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

struct Answer {
    status: StatusCode,
    content_type: String,
    body: Vec<u8>,
}

impl Answer {
    fn text(&self) -> String {
        String::from_utf8(self.body.clone()).unwrap()
    }

    fn json(&self) -> Value {
        assert_eq!(self.status, StatusCode::OK, "{}", self.text());
        serde_json::from_slice(&self.body).unwrap()
    }
}

async fn get(uri: &str, json: bool) -> Answer {
    let app = router(PlexState::new(
        ServerIdentity::new(MACHINE, "0.1.0-dev"),
        schema(),
    ))
    .layer(axum::Extension(klapz_core::testing::owner()));
    let mut request = Request::builder().uri(uri);
    if json {
        request = request.header(header::ACCEPT, "application/json");
    }
    let response = app
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    Answer {
        status,
        content_type,
        body: body.to_vec(),
    }
}

fn stored() -> Value {
    json!({ "topLeft": "1a2c53", "topRight": "81502b", "bottomRight": "58421c", "bottomLeft": "8f461d" })
}

#[tokio::test]
async fn an_item_publishes_its_stored_colors_after_its_media() {
    let lookup = get(&format!("/library/metadata/{FILM},{UNANALYSED}"), true)
        .await
        .json();
    let items = lookup["MediaContainer"]["Metadata"].as_array().unwrap();
    assert_eq!(items[0]["UltraBlurColors"], stored());
    // An object, its keys in the reference's order.
    let keys: Vec<&String> = items[0]["UltraBlurColors"]
        .as_object()
        .unwrap()
        .keys()
        .collect();
    assert_eq!(keys, ["topLeft", "topRight", "bottomRight", "bottomLeft"]);
    assert!(items[1].get("UltraBlurColors").is_none());

    let listing = get("/library/sections/4/all", true).await.json();
    assert_eq!(
        listing["MediaContainer"]["Metadata"][0]["UltraBlurColors"],
        stored()
    );

    let xml = get(&format!("/library/metadata/{FILM}"), false)
        .await
        .text();
    assert!(
        xml.contains(
            r#"<UltraBlurColors topLeft="1a2c53" topRight="81502b" bottomRight="58421c" bottomLeft="8f461d" />"#
        ),
        "{xml}"
    );
}

#[tokio::test]
async fn an_items_art_answers_its_stored_colors_in_the_references_shape() {
    let uri =
        format!("/services/ultrablur/colors?url=%2Flibrary%2Fmetadata%2F{FILM}%2Fart%2F1697263694");
    let answer = get(&uri, true).await;
    assert_eq!(
        answer.text(),
        r#"{"MediaContainer":{"size":1,"UltraBlurColors":[{"topLeft":"1a2c53","topRight":"81502b","bottomRight":"58421c","bottomLeft":"8f461d"}]}}"#
    );
    let xml = get(&uri, false).await;
    assert_eq!(xml.content_type, "text/xml;charset=utf-8");
    assert_eq!(
        xml.text(),
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<MediaContainer size=\"1\">\n<UltraBlurColors topLeft=\"1a2c53\" topRight=\"81502b\" bottomRight=\"58421c\" bottomLeft=\"8f461d\" />\n</MediaContainer>\n"
    );
}

#[tokio::test]
async fn colors_need_an_address_and_something_readable_behind_it() {
    assert_eq!(
        get("/services/ultrablur/colors", true).await.status,
        StatusCode::BAD_REQUEST
    );
    for url in [
        // No stored colors, a poster, a resource: nothing this server can read.
        format!("%2Flibrary%2Fmetadata%2F{UNANALYSED}%2Fart%2F1"),
        format!("%2Flibrary%2Fmetadata%2F{FILM}%2Fthumb%2F1697263694"),
        "%2F%3A%2Fresources%2Fmovie-fanart.jpg".to_owned(),
        String::new(),
        "garbage".to_owned(),
    ] {
        let answer = get(&format!("/services/ultrablur/colors?url={url}"), true).await;
        assert_eq!(answer.status, StatusCode::NOT_FOUND, "{url}");
        assert_eq!(answer.content_type, "text/html", "{url}");
    }
}

#[tokio::test]
async fn an_image_is_fetched_from_plex_hosts_only() {
    // The reference reads its own loopback when asked; this answers before any request.
    for url in [
        "http%3A%2F%2F127.0.0.1%3A32400%2F%3A%2Fresources%2Fmovie-fanart.jpg",
        "http%3A%2F%2Fmetadata-static.plex.tv%2Fp.jpg",
        "https%3A%2F%2F169.254.169.254%2Flatest%2Fmeta-data",
    ] {
        let colors = get(&format!("/services/ultrablur/colors?url={url}"), true).await;
        assert_eq!(colors.status, StatusCode::FORBIDDEN, "{url}");
        let image = get(&format!("/services/ultrablur/image?url={url}"), true).await;
        assert_eq!(image.status, StatusCode::FORBIDDEN, "{url}");
    }
}

const COLORS: &str = "topLeft=3f280a&topRight=6b4713&bottomRight=0f2a43&bottomLeft=1c425d";

#[tokio::test]
async fn draws_a_png_of_the_size_asked() {
    let answer = get(
        &format!("/services/ultrablur/image?{COLORS}&width=97&height=61&noise=0"),
        true,
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(answer.content_type, "image/png");
    // Its pixels are the transcoder's test, against the reference's own image.
    assert_eq!(&answer.body[..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(image_size(&answer.body), (97, 61));

    let from_art = get(
        &format!(
            "/services/ultrablur/image?url=%2Flibrary%2Fmetadata%2F{FILM}%2Fart&width=64&height=48"
        ),
        true,
    )
    .await;
    assert_eq!(from_art.status, StatusCode::OK);
    assert_eq!(image_size(&from_art.body), (64, 48));
}

/// A PNG's size, from its header chunk.
fn image_size(png: &[u8]) -> (u32, u32) {
    let side = |at: usize| u32::from_be_bytes(png[at..at + 4].try_into().unwrap());
    (side(16), side(20))
}

#[tokio::test]
async fn refuses_an_image_as_the_reference_does() {
    for query in [
        format!("{COLORS}&width=3841&height=2160"),
        format!("{COLORS}&width=-5&height=240"),
        format!("{COLORS}&noise=2"),
        "topLeft=3f280a&topRight=6b4713&bottomRight=0f2a43".to_owned(),
    ] {
        let answer = get(&format!("/services/ultrablur/image?{query}"), false).await;
        assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{query}");
    }
    // Zero makes the reference's drawing fail; ours too.
    let zero = get(
        &format!("/services/ultrablur/image?{COLORS}&width=0&height=0"),
        false,
    )
    .await;
    assert_eq!(zero.status, StatusCode::INTERNAL_SERVER_ERROR);
    let unread = get(
        &format!("/services/ultrablur/image?url=%2Flibrary%2Fmetadata%2F{UNANALYSED}%2Fart"),
        false,
    )
    .await;
    assert_eq!(unread.status, StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn the_official_api_publishes_the_colors_as_css() {
    let query = format!(
        r#"{{ node(id: "{}") {{ ... on Movie {{ cornerColors {{ topLeft topRight bottomRight bottomLeft }} }} }} }}"#,
        GlobalId::encode("Item", FILM)
    );
    let response = schema()
        .execute(async_graphql::Request::new(query).data(klapz_core::testing::owner()))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        serde_json::to_value(response.data).unwrap()["node"]["cornerColors"],
        json!({ "topLeft": "#1a2c53", "topRight": "#81502b", "bottomRight": "#58421c", "bottomLeft": "#8f461d" })
    );
}
