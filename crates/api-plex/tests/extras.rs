//! An item's extras, against responses captured on the reference (1.43.5):
//! `tests/fixtures/extras`. Two kinds of source: the trailers the reference's metadata provider
//! supplied to a film of the real library, captured as the reference looks each one up because
//! it lists none of them to an account without a subscription; and local extras of every kind
//! beside a film and a series of its test libraries, the probe's paths scrubbed. The stored
//! extras are rebuilt from the captures, so the comparison is of the shape, field for field.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use http_body_util::BodyExt;
use klapz_api::{build_schema, ApiContext, ApiSchema};
use klapz_api_plex::{router, PlexState};
use klapz_core::edit::CalendarDate;
use klapz_core::extra::{Extra, ExtraKind};
use klapz_core::metadata::{
    AudioAnalysis, ColorDescription, FileLayout, ItemKind, MediaFile, MediaStream, MediaVersion,
    MetadataItem, ScanType, StreamKind, TrackTraits, VideoAnalysis,
};
use klapz_core::testing::{InMemoryLibrary, InMemoryMetadata};
use klapz_core::{
    ExtraId, LibrarySection, MediaId, PartId, RatingKey, SectionId, SectionKind,
    ServerCapabilities, ServerIdentity, StreamId,
};
use serde_json::{Map, Value};
use tower::ServiceExt;

const MACHINE: &str = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
/// Les Croods, whose two provider trailers were captured.
const CROODS: i64 = 576_947;
/// The test library's probe film and series, with local extras.
const PROBE_FILM: i64 = 578_594;
const PROBE_SHOW: i64 = 578_605;
/// A film with a featurette and no trailer.
const NO_TRAILER: i64 = 900_001;
/// A film without extras.
const BARE: i64 = 900_002;

fn fixture(name: &str) -> Value {
    let path = format!(
        "{}/tests/fixtures/extras/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
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

fn item(id: i64, section: i64, kind: ItemKind, guid: &str, title: &str) -> MetadataItem {
    MetadataItem {
        id: RatingKey::new(id),
        section_id: SectionId::new(section),
        parent_id: None,
        kind,
        guid: guid.to_owned(),
        title: Some(title.to_owned()),
        title_sort: Some(title.to_owned()),
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
        added_at: 1_794_839_063,
        updated_at: 1_794_839_065,
        refreshed_at: None,
        chapter_source: None,
    }
}

fn kind_of(subtype: &str) -> ExtraKind {
    match subtype {
        "trailer" => ExtraKind::Trailer,
        "deletedScene" => ExtraKind::DeletedScene,
        "interview" => ExtraKind::Interview,
        "behindTheScenes" => ExtraKind::BehindTheScenes,
        "sceneOrSample" => ExtraKind::Scene,
        "featurette" => ExtraKind::Featurette,
        "short" => ExtraKind::Short,
        "other" => ExtraKind::Other,
        other => panic!("no kind captured as {other}"),
    }
}

fn int(value: &Value, key: &str) -> Option<i64> {
    value.get(key).and_then(Value::as_i64)
}

fn int32(value: &Value, key: &str) -> Option<i32> {
    int(value, key).map(|number| i32::try_from(number).unwrap())
}

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

/// What storage would hold for the extras a capture lists.
#[derive(Default)]
struct Stored {
    extras: Vec<Extra>,
    versions: Vec<(ExtraId, MediaVersion)>,
    files: Vec<MediaFile>,
    streams: Vec<MediaStream>,
}

impl Stored {
    fn add(&mut self, owner: i64, entry: &Value) {
        let id: i64 = text(entry, "ratingKey").unwrap().parse().unwrap();
        self.extras.push(Extra {
            id: ExtraId::new(id),
            item: RatingKey::new(owner),
            kind: kind_of(entry["subtype"].as_str().unwrap()),
            position: int32(entry, "index"),
            guid: text(entry, "guid").unwrap(),
            title: text(entry, "title"),
            title_sort: text(entry, "titleSort").or_else(|| text(entry, "title")),
            summary: text(entry, "summary").filter(|summary| !summary.is_empty()),
            year: int32(entry, "year"),
            released_on: text(entry, "originallyAvailableAt")
                .and_then(|date| CalendarDate::parse(&date))
                .map(CalendarDate::midnight),
            duration_ms: int(entry, "duration"),
            added_at: int(entry, "addedAt"),
            updated_at: int(entry, "updatedAt"),
        });
        for media in entry["Media"].as_array().unwrap() {
            self.add_version(owner, id, media);
        }
    }

    fn add_version(&mut self, owner: i64, extra: i64, media: &Value) {
        let version = int(media, "id").unwrap();
        let part = &media["Part"][0];
        let streamed = media.get("premium").is_some();
        let tracks = part["Stream"].as_array().unwrap();
        let video = tracks
            .iter()
            .find(|track| int(track, "streamType") == Some(1));
        self.versions.push((
            ExtraId::new(extra),
            MediaVersion {
                id: MediaId::new(version),
                item_id: RatingKey::new(owner),
                width: int32(media, "width"),
                height: int32(media, "height"),
                // Measured, as stored; the surface snaps it to the reference's 1.78 and 2.35.
                aspect_ratio: media["aspectRatio"].as_f64().map(|ratio| {
                    if ratio > 2.0 {
                        2.352_941
                    } else {
                        1.777_778
                    }
                }),
                duration_ms: int(media, "duration"),
                bitrate: int32(media, "bitrate").map(|kilobits| kilobits * 1000),
                container: text(media, "container"),
                video_codec: text(media, "videoCodec"),
                audio_codec: text(media, "audioCodec"),
                audio_channels: int32(media, "audioChannels"),
                frame_rate: video.and_then(|track| track["frameRate"].as_f64()),
                interlaced: false,
                optimized_for_streaming: media.get("optimizedForStreaming").is_some(),
            },
        ));
        let part_id = PartId::new(int(part, "id").unwrap());
        self.files.push(MediaFile {
            id: part_id,
            version_id: MediaId::new(version),
            position: 0,
            path: text(part, "file").unwrap_or_default(),
            size_bytes: int(part, "size"),
            duration_ms: int(part, "duration"),
            layout: FileLayout {
                optimized_for_streaming: part["optimizedForStreaming"].as_bool(),
                ..FileLayout::default()
            },
            stream_url: streamed.then(|| text(part, "key").unwrap()),
            content_hash: None,
            open_subtitles_hash: None,
            stated: klapz_core::metadata::StatedFile::default(),
            analysis: klapz_core::metadata::FileAnalysis::Complete,
        });
        for track in tracks {
            self.streams.push(stream(part_id, track));
        }
    }
}

fn stream(file: PartId, track: &Value) -> MediaStream {
    let kind = match int(track, "streamType") {
        Some(1) => StreamKind::Video,
        _ => StreamKind::Audio,
    };
    MediaStream {
        id: StreamId::new(int(track, "id").unwrap()),
        file_id: file,
        kind,
        position: int32(track, "index").unwrap(),
        codec: text(track, "codec"),
        language: text(track, "languageTag"),
        channels: int32(track, "channels"),
        bitrate: int32(track, "bitrate").map(|kilobits| kilobits * 1000),
        is_default: false,
        is_forced: false,
        external_path: None,
        title: None,
        stream_identifier: None,
        profile: text(track, "profile"),
        bit_depth: int32(track, "bitDepth"),
        traits: TrackTraits::default(),
        video: (kind == StreamKind::Video).then(|| {
            Box::new(VideoAnalysis {
                width: int32(track, "width"),
                height: int32(track, "height"),
                coded_width: int32(track, "codedWidth"),
                coded_height: int32(track, "codedHeight"),
                frame_rate: track["frameRate"].as_f64(),
                level: int32(track, "level"),
                ref_frames: int32(track, "refFrames"),
                scan_type: text(track, "scanType").map(|_| ScanType::Progressive),
                chroma_subsampling: text(track, "chromaSubsampling"),
                chroma_location: text(track, "chromaLocation"),
                color: ColorDescription {
                    range: text(track, "colorRange"),
                    ..ColorDescription::default()
                },
                has_scaling_matrix: track["hasScalingMatrix"].as_bool(),
                ..VideoAnalysis::default()
            })
        }),
        audio: (kind == StreamKind::Audio).then(|| AudioAnalysis {
            sampling_rate: int32(track, "samplingRate"),
            channel_layout: text(track, "audioChannelLayout"),
            loudness: None,
        }),
        subtitle: None,
        required_bandwidths: Vec::new(),
        stated: klapz_core::metadata::StatedTrack::default(),
    }
}

fn schema() -> ApiSchema {
    let items = vec![
        item(
            CROODS,
            4,
            ItemKind::Movie,
            "plex://movie/5d776889374a5b001fec87a9",
            "Les Croods",
        ),
        item(
            PROBE_FILM,
            27,
            ItemKind::Movie,
            "tv.plex.agents.none://578594",
            "Extras Probe (2019)",
        ),
        item(
            PROBE_SHOW,
            28,
            ItemKind::Show,
            "tv.plex.agents.none://578605",
            "Probe Show",
        ),
        item(
            NO_TRAILER,
            27,
            ItemKind::Movie,
            "plex://movie/1",
            "Featured",
        ),
        item(BARE, 27, ItemKind::Movie, "plex://movie/2", "Bare"),
    ];
    let mut stored = Stored::default();
    for clip in ["clip_576949", "clip_576950"] {
        stored.add(CROODS, &fixture(clip)["MediaContainer"]["Metadata"][0]);
    }
    for entry in fixture("extras_578594")["MediaContainer"]["Metadata"]
        .as_array()
        .unwrap()
    {
        stored.add(PROBE_FILM, entry);
    }
    for entry in fixture("extras_578605")["MediaContainer"]["Metadata"]
        .as_array()
        .unwrap()
    {
        stored.add(PROBE_SHOW, entry);
    }
    // The probe series' featurette, given to a film of its own with no trailer.
    let featurette = &fixture("extras_578605")["MediaContainer"]["Metadata"][1];
    let mut lone = featurette.clone();
    lone["ratingKey"] = Value::from("900010");
    lone["Media"] = Value::Array(Vec::new());
    stored.add(NO_TRAILER, &lone);

    let metadata = InMemoryMetadata::new(items).with_extras(
        stored.extras,
        stored.versions,
        stored.files,
        stored.streams,
    );
    build_schema(ApiContext {
        capabilities: ServerCapabilities::new(ServerIdentity::new(MACHINE, "0.1.0-dev"), 0),
        library: std::sync::Arc::new(InMemoryLibrary::new(vec![
            section(4, SectionKind::Movie, "VF - Films"),
            section(27, SectionKind::Movie, "klapz tests"),
            section(28, SectionKind::Show, "klapz tests series"),
        ])),
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
    })
}

struct Answer {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    body: String,
}

async fn send(method: Method, uri: &str, json: bool) -> Answer {
    let app = router(PlexState::new(
        ServerIdentity::new(MACHINE, "0.1.0-dev"),
        schema(),
    ))
    .layer(axum::Extension(klapz_core::testing::owner()));
    // The captures name languages in English, which the reference does on request only: with
    // no language it names each in itself (`Français`), where we fall back to English.
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("X-Plex-Language", "en");
    if json {
        request = request.header(header::ACCEPT, "application/json");
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

async fn json(uri: &str) -> Value {
    let answer = send(Method::GET, uri, true).await;
    assert_eq!(answer.status, StatusCode::OK, "{uri}: {}", answer.body);
    serde_json::from_str(&answer.body).unwrap()
}

/// An entry as we publish it: artwork is left out on extras as on items.
fn without_artwork(entry: &Value) -> Value {
    let mut entry = entry.as_object().unwrap().clone();
    // `retain` rather than `remove`, which swaps the last key into the removed one's place.
    entry.retain(|key, _| !["thumb", "art", "Image"].contains(&key.as_str()));
    Value::Object(entry)
}

/// The container's attributes but the media flags' version, which we do not track.
fn container(body: &Value) -> Map<String, Value> {
    let mut attributes = body["MediaContainer"].as_object().unwrap().clone();
    attributes.remove("Metadata");
    attributes.remove("mediaTagVersion");
    attributes
}

fn entries(body: &Value) -> Vec<Value> {
    body["MediaContainer"]["Metadata"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(without_artwork)
        .collect()
}

#[tokio::test]
async fn a_providers_trailers_are_listed_as_the_reference_looks_each_up() {
    let body = json(&format!("/library/metadata/{CROODS}/extras")).await;
    let expected: Vec<Value> = ["clip_576949", "clip_576950"]
        .iter()
        .map(|clip| without_artwork(&fixture(clip)["MediaContainer"]["Metadata"][0]))
        .collect();
    assert_eq!(entries(&body), expected);
    let attributes = container(&body);
    assert_eq!(
        serde_json::to_value(&attributes).unwrap(),
        serde_json::json!({
            "size": 2,
            "identifier": "com.plexapp.plugins.library",
            "mediaTagPrefix": "/system/bundle/media/flags/",
        })
    );
    // In the reference's order, attribute for attribute.
    let keys: Vec<&String> = body["MediaContainer"]["Metadata"][0]
        .as_object()
        .unwrap()
        .keys()
        .collect();
    let reference: Vec<&String> = expected[0].as_object().unwrap().keys().collect();
    assert_eq!(keys, reference);
}

#[tokio::test]
async fn local_extras_of_every_kind_are_listed_by_position_as_captured() {
    let body = json(&format!("/library/metadata/{PROBE_FILM}/extras")).await;
    let captured = fixture("extras_578594");
    assert_eq!(entries(&body), entries(&captured));
    assert_eq!(container(&body), container(&captured));

    let show = json(&format!("/library/metadata/{PROBE_SHOW}/extras")).await;
    assert_eq!(entries(&show), entries(&fixture("extras_578605")));
}

#[tokio::test]
async fn the_kind_filter_and_the_sort_change_nothing() {
    let all = json(&format!("/library/metadata/{PROBE_FILM}/extras")).await;
    for query in ["extraType=5", "extraType=1,2", "type=12", "sort=title"] {
        let filtered = json(&format!("/library/metadata/{PROBE_FILM}/extras?{query}")).await;
        assert_eq!(filtered, all, "{query}");
    }
}

#[tokio::test]
async fn both_container_parameters_window_the_list() {
    let answer = send(
        Method::GET,
        &format!(
            "/library/metadata/{PROBE_FILM}/extras?X-Plex-Container-Start=2&X-Plex-Container-Size=3"
        ),
        true,
    )
    .await;
    let body: Value = serde_json::from_str(&answer.body).unwrap();
    let container = body["MediaContainer"].as_object().unwrap();
    assert_eq!(
        container.keys().take(3).collect::<Vec<_>>(),
        ["size", "totalSize", "offset"]
    );
    assert_eq!(body["MediaContainer"]["size"], 3);
    assert_eq!(body["MediaContainer"]["totalSize"], 10);
    assert_eq!(body["MediaContainer"]["offset"], 2);
    let keys: Vec<&str> = body["MediaContainer"]["Metadata"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["ratingKey"].as_str().unwrap())
        .collect();
    assert_eq!(keys, ["578597", "578598", "578599"]);
    assert_eq!(answer.headers["X-Plex-Container-Start"], "2");
    assert_eq!(answer.headers["X-Plex-Container-Total-Size"], "10");

    // Either alone is ignored.
    let answer = send(
        Method::GET,
        &format!("/library/metadata/{PROBE_FILM}/extras?X-Plex-Container-Start=2"),
        true,
    )
    .await;
    let body: Value = serde_json::from_str(&answer.body).unwrap();
    assert_eq!(body["MediaContainer"]["size"], 10);
    assert!(body["MediaContainer"].get("totalSize").is_none());
    assert!(!answer.headers.contains_key("X-Plex-Container-Start"));
}

#[tokio::test]
async fn an_item_without_extras_lists_none() {
    let body = json(&format!("/library/metadata/{BARE}/extras")).await;
    assert_eq!(body["MediaContainer"]["size"], 0);
    assert!(body["MediaContainer"].get("Metadata").is_none());
}

#[tokio::test]
async fn what_is_no_item_answers_as_the_reference_does() {
    for (uri, status) in [
        ("/library/metadata/999999999/extras", StatusCode::NOT_FOUND),
        ("/library/metadata/abc/extras", StatusCode::NOT_FOUND),
        (
            "/library/metadata/578594,578605/extras",
            StatusCode::NOT_FOUND,
        ),
        // An extra's own extras.
        ("/library/metadata/578595/extras", StatusCode::BAD_REQUEST),
    ] {
        assert_eq!(send(Method::GET, uri, true).await.status, status, "{uri}");
    }
}

#[tokio::test]
async fn adding_an_extra_is_refused_after_the_references_own_checks() {
    for (uri, status) in [
        ("/library/metadata/999999999/extras", StatusCode::NOT_FOUND),
        ("/library/metadata/578594/extras", StatusCode::BAD_REQUEST),
        (
            "/library/metadata/578594/extras?title=Probe",
            StatusCode::BAD_REQUEST,
        ),
        (
            "/library/metadata/578595/extras?url=x&title=y",
            StatusCode::BAD_REQUEST,
        ),
        (
            "/library/metadata/578594/extras?url=file%3A%2F%2F%2Fetc%2Fpasswd&title=",
            StatusCode::FORBIDDEN,
        ),
        (
            "/library/metadata/578594/extras?url=http%3A%2F%2Fexample.invalid%2Fx.mp4&title=Probe&extraType=1",
            StatusCode::FORBIDDEN,
        ),
    ] {
        assert_eq!(send(Method::POST, uri, true).await.status, status, "{uri}");
    }
    // Nothing was added.
    let body = json(&format!("/library/metadata/{PROBE_FILM}/extras")).await;
    assert_eq!(body["MediaContainer"]["size"], 10);
}

#[tokio::test]
async fn a_lookup_carries_its_extras_last_on_request_only() {
    let body = json(&format!("/library/metadata/{PROBE_FILM}?includeExtras=1")).await;
    let item = body["MediaContainer"]["Metadata"][0].as_object().unwrap();
    assert_eq!(item.keys().next_back().map(String::as_str), Some("Extras"));
    let captured = fixture("lookup_578594_include_extras");
    let reference = &captured["MediaContainer"]["Metadata"][0]["Extras"];
    let ours = &item["Extras"];
    assert_eq!(ours["size"], reference["size"]);
    let strip = |extras: &Value| -> Vec<Value> {
        extras["Metadata"]
            .as_array()
            .unwrap()
            .iter()
            .map(without_artwork)
            .collect()
    };
    assert_eq!(strip(ours), strip(reference));
    assert_eq!(
        item["primaryExtraKey"],
        captured["MediaContainer"]["Metadata"][0]["primaryExtraKey"]
    );

    for query in ["", "?includeExtras=0", "?includeExtras=true"] {
        let body = json(&format!("/library/metadata/{PROBE_FILM}{query}")).await;
        assert!(
            body["MediaContainer"]["Metadata"][0]
                .get("Extras")
                .is_none(),
            "{query}"
        );
    }

    // Every item of a lookup, an empty one included.
    let body = json(&format!(
        "/library/metadata/{PROBE_FILM},{BARE}?includeExtras=1"
    ))
    .await;
    assert_eq!(body["MediaContainer"]["Metadata"][0]["Extras"]["size"], 10);
    assert_eq!(
        body["MediaContainer"]["Metadata"][1]["Extras"],
        serde_json::json!({ "size": 0 })
    );
}

#[tokio::test]
async fn an_empty_extras_element_keeps_the_long_form() {
    let answer = send(
        Method::GET,
        &format!("/library/metadata/{BARE}?includeExtras=1"),
        false,
    )
    .await;
    assert!(
        answer.body.contains("<Extras size=\"0\">\n</Extras>\n"),
        "{}",
        answer.body
    );
    let answer = send(
        Method::GET,
        &format!("/library/metadata/{CROODS}/extras"),
        false,
    )
    .await;
    assert!(answer.body.contains("<Video ratingKey=\"576949\""));
    assert!(answer
        .body
        .contains("optimizedForStreaming=\"1\" protocol=\"mp4\" premium=\"1\">"));
}

#[tokio::test]
async fn the_primary_extra_is_the_first_trailer_wherever_films_and_series_are_listed() {
    let films = json("/library/sections/27/all").await;
    let key_of = |body: &Value, rating_key: i64| {
        body["MediaContainer"]["Metadata"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["ratingKey"].as_str() == Some(rating_key.to_string().as_str()))
            .unwrap()
            .get("primaryExtraKey")
            .cloned()
    };
    assert_eq!(
        key_of(&films, PROBE_FILM),
        Some(Value::from("/library/metadata/578595"))
    );
    assert_eq!(key_of(&films, NO_TRAILER), None);
    assert_eq!(key_of(&films, BARE), None);

    let shows = json("/library/sections/28/all").await;
    assert_eq!(
        key_of(&shows, PROBE_SHOW),
        Some(Value::from("/library/metadata/578611"))
    );
    let lookup = json(&format!("/library/metadata/{CROODS}")).await;
    assert_eq!(
        lookup["MediaContainer"]["Metadata"][0]["primaryExtraKey"],
        "/library/metadata/576949"
    );
}
