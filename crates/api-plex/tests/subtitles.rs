//! Subtitle files, against what the reference (1.43.5) answered on its test film with sidecars
//! put beside it: `tests/fixtures/subtitles`. The film, its version and its tracks are rebuilt
//! as stored; the sidecars' files sit in the stub engine, which stands for the disk.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use http_body_util::BodyExt;
use klapz_api::{build_schema, ApiContext, ApiSchema};
use klapz_api_plex::{router, PlexState};
use klapz_core::access::{LibraryAccess, LibraryRole};
use klapz_core::metadata::{
    FileLayout, ItemKind, MediaFile, MediaStream, MediaVersion, MetadataItem, StreamKind,
    SubtitleAnalysis, TrackTraits,
};
use klapz_core::testing::{InMemoryLibrary, InMemoryMetadata, StubStreams};
use klapz_core::{
    LibrarySection, MediaId, PartId, Principal, RatingKey, SectionId, SectionKind, SectionRoot,
    ServerCapabilities, ServerIdentity, StreamId,
};
use serde_json::Value;
use tower::ServiceExt;

const MACHINE: &str = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
const FILM: i64 = 577_986;
const VERSION: i64 = 5_571_000;
const PART: i64 = 5_571_261;
const AUDIO: i64 = 12_262_673;
const FORCED: i64 = 12_262_675;
const EMBEDDED: i64 = 12_262_676;
const SDH: i64 = 12_262_677;
const GERMAN: i64 = 12_265_847;
const ITALIAN: i64 = 12_265_848;
/// A subtitle the reference downloaded into its own storage.
const DOWNLOADED: i64 = 12_265_900;

const DIRECTORY: &str = r"C:\Media\TEST";
const STEM: &str = "Soul.2020.MULTi.TRUEFRENCH.1080p.WEB.H264-EXTREME_wWw.Extreme-Down.Video";
/// A sidecar's content: what the engine reads off the disk.
const GERMAN_TEXT: &str = "1\r\n00:00:32,616 --> 00:00:34,826\r\nOK.\r\n";
const ITALIAN_TEXT: &str = "[Script Info]\nScriptType: v4.00+\n";

fn fixture(name: &str) -> Value {
    let path = format!(
        "{}/tests/fixtures/subtitles/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn path_of(extension: &str) -> String {
    format!(r"{DIRECTORY}\{STEM}.{extension}")
}

fn track(id: i64, kind: StreamKind, position: i32, codec: &str, language: &str) -> MediaStream {
    MediaStream {
        id: StreamId::new(id),
        file_id: PartId::new(PART),
        kind,
        position,
        codec: Some(codec.to_owned()),
        language: Some(language.to_owned()),
        channels: None,
        bitrate: None,
        is_default: false,
        is_forced: false,
        external_path: None,
        title: None,
        stream_identifier: None,
        profile: None,
        bit_depth: None,
        traits: TrackTraits::default(),
        video: None,
        audio: None,
        subtitle: (kind == StreamKind::Subtitle).then(SubtitleAnalysis::default),
        required_bandwidths: Vec::new(),
        stated: klapz_core::metadata::StatedTrack::default(),
    }
}

fn sidecar(id: i64, codec: &str, language: &str, url: &str) -> MediaStream {
    let mut sidecar = track(id, StreamKind::Subtitle, 0, codec, language);
    sidecar.external_path = Some(url.to_owned());
    sidecar.subtitle = Some(SubtitleAnalysis {
        format: Some(codec.to_owned()),
        ..SubtitleAnalysis::default()
    });
    sidecar
}

fn streams() -> Vec<MediaStream> {
    let mut audio = track(AUDIO, StreamKind::Audio, 0, "eac3", "fr");
    audio.is_default = true;
    let mut forced = track(FORCED, StreamKind::Subtitle, 2, "srt", "fr");
    forced.is_default = true;
    forced.is_forced = true;
    forced.title = Some("French (Forced)".to_owned());
    let mut embedded = track(EMBEDDED, StreamKind::Subtitle, 3, "srt", "fr");
    embedded.title = Some("French".to_owned());
    let mut sdh = track(SDH, StreamKind::Subtitle, 4, "srt", "fr");
    sdh.title = Some("French (SDH)".to_owned());
    let url = |extension: &str| format!("file:///C:/Media/TEST/{STEM}.{extension}");
    vec![
        audio,
        forced,
        embedded,
        sdh,
        sidecar(GERMAN, "srt", "de", &url("de.srt")),
        sidecar(ITALIAN, "ass", "it", &url("it.ass")),
        sidecar(DOWNLOADED, "srt", "fr", "blob://"),
    ]
}

fn metadata() -> InMemoryMetadata {
    let soul = MetadataItem {
        id: RatingKey::new(FILM),
        section_id: SectionId::new(27),
        parent_id: None,
        kind: ItemKind::Movie,
        guid: "plex://movie/5d776d1b7a53e9001e75c5e1".to_owned(),
        title: Some("Soul".to_owned()),
        title_sort: Some("Soul".to_owned()),
        original_title: None,
        summary: None,
        tagline: None,
        studio: None,
        content_rating: None,
        content_rating_age: None,
        position: None,
        year: Some(2020),
        released_on: None,
        duration_ms: None,
        rating: None,
        audience_rating: None,
        added_at: 1_794_839_063,
        updated_at: 1_794_839_065,
        refreshed_at: None,
        chapter_source: None,
    };
    let version = MediaVersion {
        id: MediaId::new(VERSION),
        item_id: RatingKey::new(FILM),
        width: Some(1920),
        height: Some(800),
        aspect_ratio: None,
        duration_ms: None,
        bitrate: None,
        container: Some("mkv".to_owned()),
        video_codec: Some("h264".to_owned()),
        audio_codec: Some("eac3".to_owned()),
        audio_channels: Some(6),
        frame_rate: None,
        interlaced: false,
        optimized_for_streaming: false,
    };
    let file = MediaFile {
        id: PartId::new(PART),
        version_id: MediaId::new(VERSION),
        position: 0,
        path: path_of("mkv"),
        size_bytes: None,
        duration_ms: None,
        layout: FileLayout::default(),
        stream_url: None,
        content_hash: None,
        open_subtitles_hash: None,
        stated: klapz_core::metadata::StatedFile::default(),
    };
    InMemoryMetadata::new(vec![soul]).with_media(vec![version], vec![file], streams())
}

fn section(id: i64) -> LibrarySection {
    LibrarySection {
        id: SectionId::new(id),
        kind: SectionKind::Movie,
        title: "klapz tests".to_owned(),
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

struct Server {
    schema: ApiSchema,
    disk: Arc<StubStreams>,
}

impl Server {
    fn new() -> Self {
        let disk = Arc::new(StubStreams::default());
        disk.sidecars.lock().unwrap().extend([
            (path_of("de.srt"), GERMAN_TEXT.as_bytes().to_vec()),
            (path_of("it.ass"), ITALIAN_TEXT.as_bytes().to_vec()),
        ]);
        let library = InMemoryLibrary::new(vec![section(27)]).with_roots(vec![SectionRoot {
            id: 1,
            section_id: SectionId::new(27),
            path: DIRECTORY.to_owned(),
            created_at: 0,
        }]);
        let schema = build_schema(ApiContext {
            capabilities: ServerCapabilities::new(ServerIdentity::new(MACHINE, "0.1.0-dev"), 0),
            library: Arc::new(library),
            metadata: Arc::new(metadata()),
            playback: Arc::new(klapz_core::testing::InMemoryPlayback::default()),
            play_queues: Arc::new(klapz_core::testing::InMemoryPlayQueues::default()),
            playlists: Arc::new(klapz_core::testing::InMemoryPlaylists::default()),
            collections: Arc::new(klapz_core::testing::InMemoryCollections::default()),
            access: Arc::new(klapz_core::testing::InMemoryAccess::default()),
            directory: Arc::new(klapz_core::testing::StubDirectory::default()),
            watch: Arc::new(klapz_core::testing::InMemoryWatch::default()),
            streams: Arc::clone(&disk) as Arc<dyn klapz_core::stream::StreamEngine>,
            session_locks: Arc::default(),
            scans: std::sync::Arc::new(klapz_core::testing::RecordedScans::default()),
            matching: None,
        });
        Self { schema, disk }
    }

    async fn send_as(
        &self,
        caller: Principal,
        method: Method,
        uri: &str,
        headers: &[(&str, &str)],
    ) -> Answer {
        let app = router(PlexState::new(
            ServerIdentity::new(MACHINE, "0.1.0-dev"),
            self.schema.clone(),
        ))
        .layer(axum::Extension(caller));
        let mut request = Request::builder().method(method).uri(uri);
        for (name, value) in headers {
            request = request.header(*name, *value);
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
            body: body.to_vec(),
        }
    }

    async fn send(&self, method: Method, uri: &str) -> Answer {
        self.send_as(klapz_core::testing::owner(), method, uri, &[])
            .await
    }

    async fn status(&self, method: Method, uri: &str) -> StatusCode {
        self.send(method, uri).await.status
    }

    /// The subtitle `Stream` elements of the film's lookup, as JSON.
    async fn subtitle_streams(&self) -> Vec<Value> {
        let answer = self
            .send_as(
                klapz_core::testing::owner(),
                Method::GET,
                &format!("/library/metadata/{FILM}"),
                &[("Accept", "application/json")],
            )
            .await;
        let body: Value = serde_json::from_slice(&answer.body).unwrap();
        body["MediaContainer"]["Metadata"][0]["Media"][0]["Part"][0]["Stream"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|stream| stream["streamType"] == 3)
            .cloned()
            .collect()
    }
}

struct Answer {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    body: Vec<u8>,
}

fn viewer() -> Principal {
    Principal {
        user: klapz_core::UserId::new(2_000),
        name: "viewer".to_owned(),
        role: klapz_core::ServerRole::User,
        libraries: LibraryAccess::Only(HashMap::from([(SectionId::new(27), LibraryRole::Viewer)])),
        server_identifier: None,
    }
}

/// An element without its labels, which name languages in the client's language: the
/// captures name each in itself, which no client asks for. Their rules have tests of their
/// own.
fn unlabelled(stream: &Value) -> Vec<(String, Value)> {
    stream
        .as_object()
        .unwrap()
        .iter()
        .filter(|(key, _)| {
            !matches!(
                key.as_str(),
                "language" | "displayTitle" | "extendedDisplayTitle"
            )
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

#[tokio::test]
async fn a_lookup_names_each_sidecars_address_and_file_as_the_reference() {
    let server = Server::new();
    let ours = server.subtitle_streams().await;
    let reference = fixture("lookup_streams");
    let reference = reference.as_array().unwrap();
    // The downloaded subtitle, which the capture does not hold, comes last.
    assert_eq!(ours.len(), reference.len() + 1);
    for (ours, reference) in ours.iter().zip(reference) {
        assert_eq!(unlabelled(ours), unlabelled(reference));
    }
    let downloaded = ours.last().unwrap();
    assert_eq!(downloaded["key"], format!("/library/streams/{DOWNLOADED}"));
    assert!(downloaded.get("file").is_none());
}

#[tokio::test]
async fn a_sidecar_is_served_as_it_is_under_its_own_format() {
    let server = Server::new();
    for (uri, status) in [
        (format!("/library/streams/{GERMAN}"), StatusCode::OK),
        (format!("/library/streams/{GERMAN}.srt"), StatusCode::OK),
        (format!("/library/streams/{GERMAN}.SRT"), StatusCode::OK),
        (format!("/library/streams/{GERMAN}."), StatusCode::OK),
        (format!("/library/streams/0{GERMAN}.srt"), StatusCode::OK),
        (
            format!("/library/streams/{GERMAN}?format=vtt"),
            StatusCode::OK,
        ),
        (
            format!("/library/streams/{GERMAN}?encoding=utf-8"),
            StatusCode::OK,
        ),
        (
            format!("/library/streams/{GERMAN}.vtt"),
            StatusCode::NOT_FOUND,
        ),
        (
            format!("/library/streams/{GERMAN}.ass"),
            StatusCode::NOT_FOUND,
        ),
        (
            format!("/library/streams/{GERMAN}.srt.srt"),
            StatusCode::NOT_FOUND,
        ),
        (format!("/library/streams/{ITALIAN}.ass"), StatusCode::OK),
        (
            format!("/library/streams/{ITALIAN}.srt"),
            StatusCode::NOT_FOUND,
        ),
        // Inside the media, whatever its kind.
        (
            format!("/library/streams/{EMBEDDED}.srt"),
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            format!("/library/streams/{AUDIO}"),
            StatusCode::NOT_IMPLEMENTED,
        ),
        // Downloaded into the reference's storage, which was not imported.
        (
            format!("/library/streams/{DOWNLOADED}"),
            StatusCode::NOT_FOUND,
        ),
        (
            "/library/streams/99999999.srt".to_owned(),
            StatusCode::NOT_FOUND,
        ),
        ("/library/streams/abc.srt".to_owned(), StatusCode::NOT_FOUND),
    ] {
        assert_eq!(server.status(Method::GET, &uri).await, status, "{uri}");
    }

    let answer = server
        .send(Method::GET, &format!("/library/streams/{GERMAN}.srt"))
        .await;
    assert_eq!(answer.body, GERMAN_TEXT.as_bytes());
    assert_eq!(
        answer.headers[header::CONTENT_TYPE],
        "text/plain; charset=utf-8"
    );
    assert_eq!(answer.headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");

    let head = server
        .send(Method::HEAD, &format!("/library/streams/{GERMAN}"))
        .await;
    assert_eq!(head.status, StatusCode::OK);
    assert!(head.body.is_empty());
}

#[tokio::test]
async fn one_range_is_served_alone() {
    let server = Server::new();
    let answer = server
        .send_as(
            klapz_core::testing::owner(),
            Method::GET,
            &format!("/library/streams/{GERMAN}"),
            &[("Range", "bytes=0-9")],
        )
        .await;
    assert_eq!(answer.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(answer.body, &GERMAN_TEXT.as_bytes()[..10]);
    let total = GERMAN_TEXT.len();
    assert_eq!(
        answer.headers[header::CONTENT_RANGE],
        format!("bytes 0-9/{total}").as_str()
    );
}

#[tokio::test]
async fn an_offset_is_the_callers_and_published_where_the_reference_puts_it() {
    let server = Server::new();
    for (uri, status) in [
        (
            format!("/library/streams/{EMBEDDED}?offset=300"),
            StatusCode::OK,
        ),
        (
            format!("/library/streams/{GERMAN}?offset=-1200"),
            StatusCode::OK,
        ),
    ] {
        assert_eq!(server.status(Method::PUT, &uri).await, status, "{uri}");
    }
    let ours = server.subtitle_streams().await;
    for reference in fixture("offset_streams").as_array().unwrap() {
        let id = &reference["id"];
        let shifted = ours.iter().find(|stream| &stream["id"] == id).unwrap();
        assert_eq!(unlabelled(shifted), unlabelled(reference), "{id}");
    }

    // Another caller sees none of it.
    let other = server
        .send_as(
            viewer(),
            Method::GET,
            &format!("/library/metadata/{FILM}"),
            &[("Accept", "application/json")],
        )
        .await;
    assert!(!String::from_utf8(other.body)
        .unwrap()
        .contains("\"offset\""));

    // 0 removes it.
    server
        .send(
            Method::PUT,
            &format!("/library/streams/{EMBEDDED}?offset=0"),
        )
        .await;
    let ours = server.subtitle_streams().await;
    let embedded = ours.iter().find(|stream| stream["id"] == EMBEDDED).unwrap();
    assert!(embedded.get("offset").is_none());
}

#[tokio::test]
async fn an_offset_is_refused_as_the_reference_refuses_it() {
    let server = Server::new();
    for (uri, status) in [
        (
            format!("/library/streams/{GERMAN}?offset=%2B40"),
            StatusCode::OK,
        ),
        // Nothing to change.
        (format!("/library/streams/{GERMAN}"), StatusCode::OK),
        // Accepted on audio, and published nowhere.
        (
            format!("/library/streams/{AUDIO}?offset=100"),
            StatusCode::OK,
        ),
        (
            format!("/library/streams/{GERMAN}.srt?offset=700"),
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("/library/streams/{EMBEDDED}.?offset=0"),
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("/library/streams/{GERMAN}?offset=abc"),
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("/library/streams/{GERMAN}?offset=12.5"),
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("/library/streams/{GERMAN}?offset=99999999999"),
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("/library/streams/{GERMAN}?offset="),
            StatusCode::BAD_REQUEST,
        ),
        (
            "/library/streams/99999999?offset=1".to_owned(),
            StatusCode::BAD_REQUEST,
        ),
        (
            "/library/streams/99999999".to_owned(),
            StatusCode::BAD_REQUEST,
        ),
        (
            "/library/streams/abc?offset=1".to_owned(),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        assert_eq!(server.status(Method::PUT, &uri).await, status, "{uri}");
    }
    let ours = server.subtitle_streams().await;
    let german = ours.iter().find(|stream| stream["id"] == GERMAN).unwrap();
    assert_eq!(german["offset"], 40);
}

#[tokio::test]
async fn deleting_a_sidecar_takes_its_file_and_its_track() {
    let server = Server::new();
    for (uri, status) in [
        (
            format!("/library/streams/{ITALIAN}.srt"),
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("/library/streams/{ITALIAN}.ass"),
            StatusCode::BAD_REQUEST,
        ),
        ("/library/streams/abc".to_owned(), StatusCode::BAD_REQUEST),
        (
            "/library/streams/99999999".to_owned(),
            StatusCode::NOT_FOUND,
        ),
        (format!("/library/streams/{SDH}"), StatusCode::FORBIDDEN),
        (format!("/library/streams/{ITALIAN}"), StatusCode::OK),
        (format!("/library/streams/{ITALIAN}"), StatusCode::NOT_FOUND),
    ] {
        assert_eq!(server.status(Method::DELETE, &uri).await, status, "{uri}");
    }
    assert!(!server
        .disk
        .sidecars
        .lock()
        .unwrap()
        .contains_key(&path_of("it.ass")));
    assert_eq!(
        server
            .status(Method::GET, &format!("/library/streams/{ITALIAN}"))
            .await,
        StatusCode::NOT_FOUND
    );
    let ours = server.subtitle_streams().await;
    assert!(ours.iter().all(|stream| stream["id"] != ITALIAN));
    assert!(ours.iter().any(|stream| stream["id"] == SDH));

    // A downloaded subtitle has no file here: its track goes alone.
    assert_eq!(
        server
            .status(Method::DELETE, &format!("/library/streams/{DOWNLOADED}"))
            .await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn only_the_librarys_admins_delete_a_sidecar() {
    let server = Server::new();
    let answer = server
        .send_as(
            viewer(),
            Method::DELETE,
            &format!("/library/streams/{GERMAN}"),
            &[],
        )
        .await;
    assert_eq!(answer.status, StatusCode::FORBIDDEN);
    assert!(server
        .disk
        .sidecars
        .lock()
        .unwrap()
        .contains_key(&path_of("de.srt")));
    // Reading it stays open to the viewer.
    let answer = server
        .send_as(
            viewer(),
            Method::GET,
            &format!("/library/streams/{GERMAN}"),
            &[],
        )
        .await;
    assert_eq!(answer.status, StatusCode::OK);
}

#[tokio::test]
async fn a_search_finds_nothing_after_the_references_checks() {
    let server = Server::new();
    let answer = server
        .send_as(
            klapz_core::testing::owner(),
            Method::GET,
            &format!("/library/metadata/{FILM}/subtitles?language=zu"),
            &[("Accept", "application/json")],
        )
        .await;
    assert_eq!(answer.status, StatusCode::OK);
    let body: Value = serde_json::from_slice(&answer.body).unwrap();
    let mut reference = fixture("search_none");
    // The generation of flag images, which this server does not serve (`MEDIA_TAG_VERSION`).
    reference["MediaContainer"]["mediaTagVersion"] = Value::from(0);
    assert_eq!(body, reference);
    let keys: Vec<&String> = body["MediaContainer"].as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        ["size", "identifier", "mediaTagPrefix", "mediaTagVersion"]
    );

    for (uri, status) in [
        (
            format!("/library/metadata/{FILM}/subtitles"),
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("/library/metadata/{FILM}/subtitles?language=fr-FR"),
            StatusCode::OK,
        ),
        (
            format!("/library/metadata/{FILM}/subtitles?language=fr&mediaItemID={VERSION}"),
            StatusCode::OK,
        ),
        (
            format!("/library/metadata/{FILM}/subtitles?language=fr&mediaItemID=1"),
            StatusCode::NOT_FOUND,
        ),
        // A `500` on the reference.
        (
            format!("/library/metadata/{FILM}/subtitles?language=fra"),
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("/library/metadata/{FILM}/subtitles?language="),
            StatusCode::BAD_REQUEST,
        ),
        (
            "/library/metadata/99999999/subtitles?language=fr".to_owned(),
            StatusCode::NOT_FOUND,
        ),
        (
            format!("/library/metadata/{FILM},{FILM}/subtitles?language=fr"),
            StatusCode::NOT_FOUND,
        ),
    ] {
        assert_eq!(server.status(Method::GET, &uri).await, status, "{uri}");
    }

    let answer = server
        .send_as(
            viewer(),
            Method::GET,
            &format!("/library/metadata/{FILM}/subtitles?language=fr"),
            &[],
        )
        .await;
    assert_eq!(answer.status, StatusCode::FORBIDDEN);
}
