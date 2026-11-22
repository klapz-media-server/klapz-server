//! Conformance of `/hubs/search/voice` and `/library/sections/{id}/autocomplete`.
//!
//! The expectations are the reference's answers (`tests/fixtures/`), captured and reduced to
//! what these endpoints decide: the shelves and their attributes, the entries, their
//! distances, their order and their ranges, with no file path left. The catalog is built
//! from the same captures, so that the two servers rank the same titles.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use klapz_api::{build_schema, ApiContext};
use klapz_api_plex::{router, PlexState};
use klapz_core::metadata::{ItemKind, ItemTag, MetadataItem, Person, Tag, TagKind};
use klapz_core::testing::{InMemoryLibrary, InMemoryMetadata};
use klapz_core::{
    LibrarySection, PersonId, RatingKey, SectionId, SectionKind, ServerCapabilities,
    ServerIdentity, TagId,
};
use serde_json::Value;
use tower::ServiceExt;

const VOICE_BREAKING_BAD: &str = include_str!("fixtures/voice_breaking_bad.json");
const VOICE_DUNE: &str = include_str!("fixtures/voice_dune.json");
const VOICE_THE_OFFICE: &str = include_str!("fixtures/voice_the_office_shows.json");
const TITLE_DUN: &str = include_str!("fixtures/autocomplete_title_dun.json");
const ACTOR_HANKS: &str = include_str!("fixtures/autocomplete_actor_hanks.json");

fn section(id: i64, kind: SectionKind, title: &str) -> LibrarySection {
    LibrarySection {
        id: SectionId::new(id),
        kind,
        title: title.to_owned(),
        title_sort: None,
        language: "fr-FR".to_owned(),
        uuid: format!("uuid-{id}"),
        created_at: 1_477_835_115,
        updated_at: 1_792_916_300,
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
    title_sort: Option<&str>,
) -> MetadataItem {
    MetadataItem {
        id: RatingKey::new(id),
        section_id: SectionId::new(section),
        parent_id: None,
        kind,
        guid: format!("plex://item/{id}"),
        title: Some(title.to_owned()),
        title_sort: title_sort.map(ToOwned::to_owned),
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
        added_at: 1_500_000_000,
        updated_at: 1_600_000_000,
        refreshed_at: None,
        chapter_source: None,
    }
}

/// The items every fixture lists, as the reference describes them.
fn captured_items() -> Vec<MetadataItem> {
    let mut items = Vec::new();
    for fixture in [VOICE_BREAKING_BAD, VOICE_DUNE, VOICE_THE_OFFICE, TITLE_DUN] {
        let container = &serde_json::from_str::<Value>(fixture).unwrap()["MediaContainer"];
        let listed = container["Hub"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|hub| hub["Metadata"].as_array().cloned().unwrap_or_default())
            .chain(
                container["Metadata"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default(),
            );
        for entry in listed {
            let id: i64 = entry["ratingKey"].as_str().unwrap().parse().unwrap();
            if items.iter().any(|item: &MetadataItem| item.id.get() == id) {
                continue;
            }
            let kind = match entry["type"].as_str().unwrap() {
                "movie" => ItemKind::Movie,
                _ => ItemKind::Show,
            };
            let section = entry["librarySectionID"].as_i64().unwrap_or(4);
            items.push(item(
                id,
                section,
                kind,
                entry["title"].as_str().unwrap(),
                entry["titleSort"].as_str(),
            ));
        }
    }
    items
}

/// The actors the completion fixture lists, each on a film of the library, and one who is
/// no completion of `hanks`.
fn captured_actors() -> Vec<ItemTag> {
    let container = &serde_json::from_str::<Value>(ACTOR_HANKS).unwrap()["MediaContainer"];
    let mut actors: Vec<ItemTag> = container["Directory"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            let id = entry["id"].as_i64().unwrap();
            let name = entry["tag"].as_str().unwrap().to_owned();
            ItemTag {
                item_id: RatingKey::new(470_473),
                tag: Tag {
                    id: TagId::new(id),
                    kind: TagKind::Actor,
                    name: name.clone(),
                    person: Some(Person {
                        id: PersonId::new(id),
                        name,
                        provider_key: entry["tagKey"].as_str().map(ToOwned::to_owned),
                        thumb_url: entry["thumb"].as_str().map(ToOwned::to_owned),
                    }),
                },
                role: None,
            }
        })
        .collect();
    actors.push(ItemTag {
        item_id: RatingKey::new(470_473),
        tag: Tag {
            id: TagId::new(478_655),
            kind: TagKind::Actor,
            name: "Tom Hardy".to_owned(),
            person: None,
        },
        role: None,
    });
    actors
}

fn app() -> axum::Router {
    let identity = ServerIdentity::new("deadbeefdeadbeefdeadbeefdeadbeefdeadbeef", "0.1.0-dev");
    let library = InMemoryLibrary::new(vec![
        section(4, SectionKind::Movie, "VF - Films"),
        section(5, SectionKind::Show, "VF - Series"),
        section(6, SectionKind::Movie, "VO - Movies"),
        section(7, SectionKind::Show, "VO - TV Shows"),
    ]);
    let metadata = InMemoryMetadata::new(captured_items()).with_tags(captured_actors());
    let schema = build_schema(ApiContext {
        capabilities: ServerCapabilities::new(identity.clone(), 0),
        library: std::sync::Arc::new(library),
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
        scans: std::sync::Arc::new(klapz_core::testing::RecordedScans::default()),
        matching: None,
    });
    router(PlexState::new(identity, schema)).layer(axum::Extension(klapz_core::testing::owner()))
}

async fn get(uri: &str, accept: &str) -> (StatusCode, String) {
    let request = Request::builder()
        .uri(uri)
        .header(header::ACCEPT, accept)
        .body(Body::empty())
        .unwrap();
    let response = app().oneshot(request).await.unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned())
}

async fn json(uri: &str) -> Value {
    let (status, body) = get(uri, "application/json").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_str(&body).unwrap()
}

/// An answer reduced as the fixtures are: the binding's own attributes, and of each entry
/// the ones that identify and place it.
fn reduced(mut answer: Value) -> Value {
    const ITEM: [&str; 10] = [
        "distance",
        "title.queryRange",
        "ratingKey",
        "key",
        "type",
        "title",
        "titleSort",
        "librarySectionTitle",
        "librarySectionID",
        "librarySectionKey",
    ];
    const TAG: [&str; 6] = ["id", "filter", "tag", "tagType", "tagKey", "thumb"];
    fn keep(entry: &Value, keys: &[&str]) -> Value {
        Value::Object(
            keys.iter()
                .filter_map(|key| Some(((*key).to_owned(), entry.get(*key)?.clone())))
                .collect(),
        )
    }
    fn reduce(container: &mut Value) {
        let Some(object) = container.as_object_mut() else {
            return;
        };
        object.remove("mediaTagVersion");
        for (key, value) in object.iter_mut() {
            let Some(entries) = value.as_array_mut() else {
                continue;
            };
            for entry in entries.iter_mut() {
                match key.as_str() {
                    "Hub" => reduce(entry),
                    "Metadata" => *entry = keep(entry, &ITEM),
                    "Directory" => *entry = keep(entry, &TAG),
                    _ => {}
                }
            }
        }
    }
    reduce(&mut answer["MediaContainer"]);
    answer
}

/// Matches at the same distance in one order: the reference's follows its scan, ours the
/// folded title then the id. Applied to both sides alike.
fn ties_in_id_order(mut answer: Value) -> Value {
    for hub in answer["MediaContainer"]["Hub"]
        .as_array_mut()
        .into_iter()
        .flatten()
    {
        let items = hub["Metadata"].as_array_mut().unwrap();
        items.sort_by_key(|item| {
            (
                item["distance"].as_u64().unwrap(),
                item["ratingKey"].as_str().unwrap().parse::<i64>().unwrap(),
            )
        });
        let keys: Vec<&str> = items
            .iter()
            .map(|item| item["ratingKey"].as_str().unwrap())
            .collect();
        hub["hubKey"] = Value::String(format!("/library/metadata/{}", keys.join(",")));
    }
    answer
}

fn fixture(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

#[tokio::test]
async fn voice_shelves_match_the_reference() {
    for (uri, captured) in [
        (
            "/hubs/search/voice?query=breaking%20bad",
            VOICE_BREAKING_BAD,
        ),
        ("/hubs/search/voice?query=dune", VOICE_DUNE),
        (
            "/hubs/search/voice?query=the%20office&type=2&limit=1",
            VOICE_THE_OFFICE,
        ),
    ] {
        assert_eq!(
            ties_in_id_order(reduced(json(uri).await)),
            ties_in_id_order(fixture(captured)),
            "{uri}"
        );
    }
}

#[tokio::test]
async fn a_voice_shelf_is_named_after_its_kind_in_xml() {
    let (status, body) = get("/hubs/search/voice?query=breaking%20bad", "*/*").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains(r#"<Hub distance="0" hubKey="/library/metadata/22773,24569" title="show" type="show" hubIdentifier="results.search.2" context="hub.results.search" size="2" more="0" style="shelf">"#), "{body}");
    assert!(
        body.contains(r#"<Directory distance="0" ratingKey="22773""#),
        "{body}"
    );
    assert!(
        body.contains(r#"<Video distance="3" ratingKey="306682""#),
        "{body}"
    );
}

#[tokio::test]
async fn the_limit_counts_across_shelves() {
    let answer = json("/hubs/search/voice?query=breaking%20bad&limit=2").await;
    let hubs = answer["MediaContainer"]["Hub"].as_array().unwrap();
    assert_eq!(hubs.len(), 1);
    assert_eq!(hubs[0]["size"], 2);
}

#[tokio::test]
async fn voice_search_answers_nothing_for_nothing_asked() {
    for uri in [
        "/hubs/search/voice?query=dune&limit=0",
        "/hubs/search/voice?query=dune&type=8",
        "/hubs/search/voice?query=dune&type=99",
    ] {
        let (status, body) = get(uri, "*/*").await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(
            body.contains(r#"<MediaContainer size="0">"#),
            "{uri}: {body}"
        );
    }
}

#[tokio::test]
async fn voice_search_refuses_what_the_reference_refuses() {
    for uri in [
        "/hubs/search/voice",
        "/hubs/search/voice?query=",
        "/hubs/search/voice?query=dune&limit=abc",
        "/hubs/search/voice?query=dune&limit=-1",
        "/hubs/search/voice?query=dune&type=abc",
        "/hubs/search/voice?query=dune&type=1,2",
    ] {
        assert_eq!(get(uri, "*/*").await.0, StatusCode::BAD_REQUEST, "{uri}");
    }
}

#[tokio::test]
async fn title_completions_match_the_reference() {
    let mut answer = reduced(json("/library/sections/4/autocomplete?type=1&title.query=dun").await);
    // Always windowed here, where the reference states the window only when asked for one.
    let container = answer["MediaContainer"].as_object_mut().unwrap();
    assert_eq!(container.remove("totalSize"), Some(Value::from(9)));
    assert_eq!(container.remove("offset"), Some(Value::from(0)));
    assert_eq!(answer, fixture(TITLE_DUN));
}

#[tokio::test]
async fn tag_completions_match_the_reference() {
    let mut answer =
        reduced(json("/library/sections/4/autocomplete?type=1&actor.query=hanks").await);
    let container = answer["MediaContainer"].as_object_mut().unwrap();
    assert_eq!(container.remove("totalSize"), Some(Value::from(9)));
    assert_eq!(container.remove("offset"), Some(Value::from(0)));
    assert_eq!(answer, fixture(ACTOR_HANKS));
}

#[tokio::test]
async fn completion_refuses_what_the_reference_refuses() {
    for uri in [
        "/library/sections/4/autocomplete",
        "/library/sections/4/autocomplete?type=1",
        "/library/sections/4/autocomplete?title.query=dun",
        "/library/sections/4/autocomplete?type=abc&title.query=dun",
        "/library/sections/4/autocomplete?type=1&title.query=",
        "/library/sections/4/autocomplete?type=1&title.query=dun&genre.query=act",
        "/library/sections/4/autocomplete?type=1&foo.query=a",
    ] {
        assert_eq!(get(uri, "*/*").await.0, StatusCode::BAD_REQUEST, "{uri}");
    }
    assert_eq!(
        get(
            "/library/sections/999/autocomplete?type=1&title.query=d",
            "*/*"
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    // A field the reference completes and this server does not yet.
    assert_eq!(
        get(
            "/library/sections/4/autocomplete?type=1&studio.query=a",
            "*/*"
        )
        .await
        .0,
        StatusCode::NOT_IMPLEMENTED
    );
}

#[tokio::test]
async fn a_completion_pages_and_keeps_the_other_filters() {
    let (status, body) = get(
        "/library/sections/4/autocomplete?type=1&title.query=dun&X-Plex-Container-Start=3&X-Plex-Container-Size=2",
        "*/*",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(r#"size="2" totalSize="9" offset="3""#),
        "{body}"
    );
    assert!(
        body.contains(r#"<Video title.queryRange="0,2" ratingKey="470473""#),
        "{body}"
    );
    // A type the library does not hold completes nothing.
    let answer = json("/library/sections/4/autocomplete?type=2&title.query=dun").await;
    assert_eq!(answer["MediaContainer"]["size"], 0);
}
