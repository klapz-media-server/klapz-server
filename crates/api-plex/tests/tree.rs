//! An item's tree, and the sonic neighbors music would have, against the reference (1.43.5).
//!
//! The trees are the test libraries' film and series as captured (`tests/fixtures/tree`, the
//! folder of their files scrubbed), and the stored library is rebuilt from those captures:
//! the comparison is of the shape, byte for byte in both formats. The statuses were measured
//! on the reference's film and series libraries; it holds no music.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use klapz_api::{build_schema, ApiContext, ApiSchema};
use klapz_api_plex::{router, PlexState};
use klapz_core::edit::CalendarDate;
use klapz_core::metadata::{
    AudioAnalysis, ColorDescription, FileLayout, ItemKind, MediaFile, MediaStream, MediaVersion,
    MetadataItem, ScanType, StreamKind, SubtitleAnalysis, TrackTraits, VideoAnalysis,
};
use klapz_core::testing::{InMemoryLibrary, InMemoryMetadata, InMemoryWatch};
use klapz_core::watch::{WatchRepository, WatchState};
use klapz_core::{
    LibrarySection, MediaId, PartId, RatingKey, SectionId, SectionKind, ServerCapabilities,
    ServerIdentity, StreamId,
};
use serde_json::Value;
use tower::ServiceExt;

const MACHINE: &str = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
const SOUL: i64 = 577_986;
const BULL: i64 = 577_987;
const SEASON: i64 = 577_988;
const EPISODE: i64 = 578_173;
/// What the reference's flags are versioned at, which we do not track.
const REFERENCE_TAG_VERSION: &str = "1791210760";

fn fixture(name: &str) -> String {
    let path = format!("{}/tests/fixtures/tree/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(path).unwrap()
}

fn fixture_json(name: &str) -> Value {
    serde_json::from_str(&fixture(&format!("{name}.json"))).unwrap()
}

fn section(id: i64, kind: SectionKind) -> LibrarySection {
    LibrarySection {
        id: SectionId::new(id),
        kind,
        title: format!("Library {id}"),
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

/// What the captures hold, back in storage's terms.
#[derive(Default)]
struct Stored {
    items: Vec<MetadataItem>,
    versions: Vec<MediaVersion>,
    files: Vec<MediaFile>,
    streams: Vec<MediaStream>,
}

impl Stored {
    fn add(&mut self, entry: &Value, section: i64, kind: ItemKind, parent: Option<i64>) {
        let id = entry["id"].as_i64().unwrap();
        let text = |key: &str| {
            entry[key]
                .as_str()
                .filter(|text| !text.is_empty())
                .map(ToOwned::to_owned)
        };
        let refreshed = entry["refreshedAt"].as_i64().unwrap();
        self.items.push(MetadataItem {
            id: RatingKey::new(id),
            section_id: SectionId::new(section),
            parent_id: parent.map(RatingKey::new),
            kind,
            guid: text("guid").unwrap(),
            title: text("title"),
            title_sort: None,
            original_title: None,
            summary: None,
            tagline: None,
            studio: None,
            content_rating: None,
            content_rating_age: None,
            position: entry["index"]
                .as_i64()
                .map(|index| i32::try_from(index).unwrap()),
            year: None,
            released_on: text("originallyAvailableAt")
                .map(|date| CalendarDate::parse(&date).unwrap().midnight()),
            duration_ms: None,
            rating: None,
            audience_rating: None,
            added_at: entry["addedAt"].as_i64().unwrap(),
            updated_at: 0,
            refreshed_at: (refreshed >= 0).then_some(refreshed),
            chapter_source: None,
        });
        for version in entry["MediaItem"].as_array().into_iter().flatten() {
            self.add_version(id, version);
        }
        for child in entry["MetadataItem"].as_array().into_iter().flatten() {
            let kind = if kind == ItemKind::Show {
                ItemKind::Season
            } else {
                ItemKind::Episode
            };
            self.add(child, section, kind, Some(id));
        }
    }

    fn add_version(&mut self, item: i64, version: &Value) {
        let id = version["id"].as_i64().unwrap();
        self.versions.push(MediaVersion {
            id: MediaId::new(id),
            item_id: RatingKey::new(item),
            width: None,
            height: None,
            aspect_ratio: None,
            duration_ms: None,
            bitrate: None,
            container: None,
            video_codec: None,
            audio_codec: None,
            audio_channels: None,
            frame_rate: None,
            interlaced: false,
            optimized_for_streaming: false,
        });
        for (position, part) in version["MediaPart"].as_array().unwrap().iter().enumerate() {
            let file = part["id"].as_i64().unwrap();
            self.files.push(MediaFile {
                id: PartId::new(file),
                version_id: MediaId::new(id),
                position: i32::try_from(position).unwrap(),
                path: part["file"].as_str().unwrap().to_owned(),
                size_bytes: part["size"].as_i64(),
                duration_ms: part["duration"].as_i64(),
                layout: FileLayout::default(),
                stream_url: None,
                content_hash: part["hash"].as_str().map(ToOwned::to_owned),
                open_subtitles_hash: part["openSubtitleHash"].as_str().map(ToOwned::to_owned),
                stated: klapz_core::metadata::StatedFile::default(),
                analysis: klapz_core::metadata::FileAnalysis::Complete,
            });
            for track in part["MediaStream"].as_array().unwrap() {
                self.streams.push(track_of(file, track));
            }
        }
    }
}

/// A captured track, its analysis keys read back into the typed fields they were imported
/// into.
fn track_of(file: i64, track: &Value) -> MediaStream {
    let text = |key: &str| track[key].as_str().map(ToOwned::to_owned);
    let int = |key: &str| text(key).map(|value| value.parse::<i32>().unwrap());
    let flag = |key: &str| text(key).map(|value| value == "1");
    let kind = match track["type"].as_i64().unwrap() {
        1 => StreamKind::Video,
        2 => StreamKind::Audio,
        _ => StreamKind::Subtitle,
    };
    MediaStream {
        id: StreamId::new(track["id"].as_i64().unwrap()),
        file_id: PartId::new(file),
        kind,
        position: i32::try_from(track["index"].as_i64().unwrap()).unwrap(),
        codec: text("codec"),
        language: text("language"),
        channels: track["channels"]
            .as_i64()
            .map(|value| i32::try_from(value).unwrap()),
        bitrate: None,
        is_default: false,
        is_forced: false,
        external_path: None,
        title: text("title"),
        stream_identifier: None,
        profile: text("profile"),
        bit_depth: int("bitDepth"),
        traits: TrackTraits::default(),
        video: (kind == StreamKind::Video).then(|| {
            Box::new(VideoAnalysis {
                width: int("width"),
                height: int("height"),
                coded_width: int("codedWidth"),
                coded_height: int("codedHeight"),
                frame_rate: text("frameRate").map(|rate| rate.parse().unwrap()),
                level: int("level"),
                ref_frames: int("refFrames"),
                scan_type: text("scanType").map(|scan| match scan.as_str() {
                    "interlaced" => ScanType::Interlaced,
                    _ => ScanType::Progressive,
                }),
                chroma_subsampling: text("chromaSubsampling"),
                chroma_location: text("chromaLocation"),
                color: ColorDescription {
                    range: text("colorRange"),
                    space: text("colorSpace"),
                    primaries: text("colorPrimaries"),
                    transfer: text("colorTrc"),
                },
                has_scaling_matrix: flag("hasScalingMatrix"),
                ..VideoAnalysis::default()
            })
        }),
        audio: (kind == StreamKind::Audio).then(|| AudioAnalysis {
            sampling_rate: int("samplingRate"),
            channel_layout: text("audioChannelLayout"),
            loudness: None,
        }),
        subtitle: (kind == StreamKind::Subtitle).then(SubtitleAnalysis::default),
        required_bandwidths: text("requiredBandwidths")
            .map(|list| {
                list.split(',')
                    .map(|value| value.parse().unwrap())
                    .collect()
            })
            .unwrap_or_default(),
        stated: klapz_core::metadata::StatedTrack::default(),
    }
}

/// The captures were made by an account that had played Bull: the reference writes a series'
/// preferences only for an account holding a state of it.
async fn schema(played_bull: bool) -> ApiSchema {
    let mut stored = Stored::default();
    let root = |name: &str| fixture_json(name)["MediaContainer"]["MetadataItem"][0].clone();
    stored.add(&root("movie"), 27, ItemKind::Movie, None);
    stored.add(&root("show"), 28, ItemKind::Show, None);

    let watch = InMemoryWatch::over(stored.items.clone());
    let played: &[(String, WatchState)] = &[(
        format!("tv.plex.agents.none://{BULL}"),
        WatchState {
            view_count: 1,
            offset_ms: None,
            last_viewed_at: Some(1_794_835_553),
        },
    )];
    watch
        .save(
            klapz_core::testing::owner().user,
            if played_bull { played } else { &[] },
        )
        .await
        .unwrap();
    let metadata = InMemoryMetadata::new(stored.items).with_media(
        stored.versions,
        stored.files,
        stored.streams,
    );
    build_schema(ApiContext {
        capabilities: ServerCapabilities::new(ServerIdentity::new(MACHINE, "0.1.0-dev"), 0),
        library: std::sync::Arc::new(InMemoryLibrary::new(vec![
            section(27, SectionKind::Movie),
            section(28, SectionKind::Show),
        ])),
        metadata: std::sync::Arc::new(metadata),
        playback: std::sync::Arc::new(klapz_core::testing::InMemoryPlayback::default()),
        play_queues: std::sync::Arc::new(klapz_core::testing::InMemoryPlayQueues::default()),
        playlists: std::sync::Arc::new(klapz_core::testing::InMemoryPlaylists::default()),
        collections: std::sync::Arc::new(klapz_core::testing::InMemoryCollections::default()),
        access: std::sync::Arc::new(klapz_core::testing::InMemoryAccess::default()),
        directory: std::sync::Arc::new(klapz_core::testing::StubDirectory::default()),
        watch: std::sync::Arc::new(watch),
        streams: std::sync::Arc::new(klapz_core::testing::StubStreams::default()),
        session_locks: std::sync::Arc::default(),
        scans: std::sync::Arc::new(klapz_core::testing::RecordedScans::default()),
        matching: None,
    })
}

async fn send(uri: &str, json: bool) -> (StatusCode, String) {
    send_as(uri, json, true).await
}

async fn send_as(uri: &str, json: bool, played_bull: bool) -> (StatusCode, String) {
    let app = router(PlexState::new(
        ServerIdentity::new(MACHINE, "0.1.0-dev"),
        schema(played_bull).await,
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
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8(body.to_vec()).unwrap())
}

async fn status(uri: &str) -> StatusCode {
    send(uri, false).await.0
}

/// The capture, its flags' version replaced by ours.
fn expected(name: &str) -> (Value, String) {
    let mut json = fixture_json(name);
    json["MediaContainer"]["mediaTagVersion"] = Value::from(0);
    let xml = fixture(&format!("{name}.xml")).replace(
        &format!("mediaTagVersion=\"{REFERENCE_TAG_VERSION}\""),
        "mediaTagVersion=\"0\"",
    );
    (json, xml)
}

async fn assert_tree(uri: &str, name: &str) {
    let (json, xml) = expected(name);
    let (code, body) = send(uri, true).await;
    assert_eq!(code, StatusCode::OK, "{uri}: {body}");
    assert_eq!(serde_json::from_str::<Value>(&body).unwrap(), json, "{uri}");
    let (code, body) = send(uri, false).await;
    assert_eq!(code, StatusCode::OK, "{uri}");
    assert_eq!(body, xml, "{uri}");
}

#[tokio::test]
async fn a_film_is_its_versions_files_and_tracks() {
    assert_tree(&format!("/library/metadata/{SOUL}/tree"), "movie").await;
}

#[tokio::test]
async fn a_series_holds_its_seasons_episodes_and_preferences() {
    assert_tree(&format!("/library/metadata/{BULL}/tree"), "show").await;
}

#[tokio::test]
async fn a_season_roots_its_own_tree_under_its_series_title() {
    assert_tree(&format!("/library/metadata/{SEASON}/tree"), "season").await;
}

#[tokio::test]
async fn an_episode_is_the_branch_from_its_series() {
    assert_tree(&format!("/library/metadata/{EPISODE}/tree"), "episode").await;
}

#[tokio::test]
async fn exactly_one_number_of_the_list_must_name_something() {
    // Unknown numbers are dropped, and a trailing comma names nothing more.
    assert_tree(&format!("/library/metadata/{SOUL},999999999/tree"), "movie").await;
    assert_tree(&format!("/library/metadata/999999999,{SOUL}/tree"), "movie").await;
    assert_tree(&format!("/library/metadata/{SOUL},/tree"), "movie").await;
    for ids in [
        format!("{SOUL},{BULL}"),
        format!("{BULL},{SEASON}"),
        format!("{SOUL},{SOUL}"),
        "999999999".to_owned(),
        "abc".to_owned(),
        format!("%20{SOUL}"),
        "-1".to_owned(),
    ] {
        assert_eq!(
            status(&format!("/library/metadata/{ids}/tree")).await,
            StatusCode::NOT_FOUND,
            "{ids}"
        );
    }
}

#[tokio::test]
async fn nothing_here_is_a_track_to_find_neighbors_of() {
    for (uri, expected) in [
        (
            format!("/library/metadata/{SOUL}/nearest"),
            StatusCode::FORBIDDEN,
        ),
        (
            format!("/library/metadata/{BULL}/nearest"),
            StatusCode::FORBIDDEN,
        ),
        (
            format!("/library/metadata/{EPISODE}/nearest"),
            StatusCode::FORBIDDEN,
        ),
        (
            format!("/library/metadata/{SOUL},999999999/nearest"),
            StatusCode::FORBIDDEN,
        ),
        (
            format!("/library/metadata/999999999,{SOUL}/nearest"),
            StatusCode::FORBIDDEN,
        ),
        (
            format!("/library/metadata/{SOUL}/nearest?limit=-5&maxDistance=1e3"),
            StatusCode::FORBIDDEN,
        ),
        (
            "/library/metadata/999999999/nearest".to_owned(),
            StatusCode::NOT_FOUND,
        ),
        (
            "/library/metadata/abc/nearest".to_owned(),
            StatusCode::NOT_FOUND,
        ),
        (
            "/library/metadata/999999999/nearest?limit=abc".to_owned(),
            StatusCode::NOT_FOUND,
        ),
        (
            format!("/library/metadata/{SOUL}/nearest?limit=abc"),
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("/library/metadata/{SOUL}/nearest?limit=1.5"),
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("/library/metadata/{SOUL}/nearest?limit="),
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("/library/metadata/{SOUL}/nearest?maxDistance=abc"),
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("/library/metadata/{SOUL}/nearest?excludeParentID=abc"),
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("/library/metadata/{SOUL}/nearest?excludeGrandparentID=x"),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        assert_eq!(status(&uri).await, expected, "{uri}");
    }
}

#[tokio::test]
async fn no_library_holds_an_analysed_track() {
    let fifty = vec!["1"; 50].join(",");
    for (uri, expected) in [
        (
            format!("/library/sections/27/nearest?type=10&values={fifty}"),
            StatusCode::FORBIDDEN,
        ),
        (
            format!("/library/sections/28/nearest?type=10&values={fifty}"),
            StatusCode::FORBIDDEN,
        ),
        (
            format!("/library/sections/27/nearest?type=1&values={fifty}"),
            StatusCode::FORBIDDEN,
        ),
        (
            "/library/sections/27/nearest?type=10&values=1.5,-2".to_owned(),
            StatusCode::FORBIDDEN,
        ),
        (
            "/library/sections/27/nearest?type=10&values=1,,2".to_owned(),
            StatusCode::FORBIDDEN,
        ),
        (
            "/library/sections/27/nearest?type=10&values=1e3".to_owned(),
            StatusCode::FORBIDDEN,
        ),
        (
            "/library/sections/27/nearest?type=10&values=".to_owned(),
            StatusCode::FORBIDDEN,
        ),
        (
            "/library/sections/999/nearest".to_owned(),
            StatusCode::NOT_FOUND,
        ),
        (
            format!("/library/sections/999/nearest?type=10&values={fifty}"),
            StatusCode::NOT_FOUND,
        ),
        (
            "/library/sections/abc/nearest?type=10&values=1".to_owned(),
            StatusCode::NOT_FOUND,
        ),
        (
            "/library/sections/27/nearest".to_owned(),
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("/library/sections/27/nearest?values={fifty}"),
            StatusCode::BAD_REQUEST,
        ),
        (
            "/library/sections/27/nearest?type=10".to_owned(),
            StatusCode::BAD_REQUEST,
        ),
        (
            "/library/sections/27/nearest?type=abc&values=1".to_owned(),
            StatusCode::BAD_REQUEST,
        ),
        (
            "/library/sections/27/nearest?type=&values=1".to_owned(),
            StatusCode::BAD_REQUEST,
        ),
        (
            "/library/sections/27/nearest?type=1.0&values=1".to_owned(),
            StatusCode::BAD_REQUEST,
        ),
        (
            "/library/sections/27/nearest?type=10&values=a,b".to_owned(),
            StatusCode::BAD_REQUEST,
        ),
        (
            "/library/sections/27/nearest?type=10&values=%201".to_owned(),
            StatusCode::BAD_REQUEST,
        ),
        (
            "/library/sections/27/nearest?type=10&values=1x".to_owned(),
            StatusCode::BAD_REQUEST,
        ),
        (
            "/library/sections/27/nearest?type=10&values=1&limit=abc".to_owned(),
            StatusCode::BAD_REQUEST,
        ),
        (
            "/library/sections/27/nearest?type=10&values=1&limit=1.5".to_owned(),
            StatusCode::BAD_REQUEST,
        ),
        (
            "/library/sections/27/nearest?type=10&values=1&maxDistance=abc".to_owned(),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        assert_eq!(status(&uri).await, expected, "{uri}");
    }
}

#[tokio::test]
async fn a_series_nobody_touched_carries_no_preferences() {
    let (mut json, _) = expected("show");
    json["MediaContainer"]["MetadataItem"][0]
        .as_object_mut()
        .unwrap()
        .remove("Setting");
    let (code, body) = send_as(&format!("/library/metadata/{BULL}/tree"), true, false).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(serde_json::from_str::<Value>(&body).unwrap(), json);
}
