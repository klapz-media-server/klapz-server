//! A server holding a small copy of the reference's test library, reached as Jellyfin clients
//! reach it: tokens resolved as the server's authentication resolves them on the surface.
//!
//! The library rebuilds two items of the reference Jellyfin's, as our scan stores them: the
//! film `doodlebug` and the 21st episode of `90210`'s first season. `tests/fixtures` holds
//! what the reference answered about them (`tools/capture_jellyfin.py --fixtures`).

#![allow(clippy::expect_used, clippy::unwrap_used, dead_code)]

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Router;
use http_body_util::BodyExt;
use open_plex_api::{build_schema, ApiContext, ApiSchema};
use open_plex_api_jellyfin::{router, JellyfinState};
use open_plex_core::access::{
    token_digest, AccessRepository, Extension, LibraryPermission, LibraryRole, NewUser, ServerRole,
};
use open_plex_core::metadata::{
    AudioAnalysis, FileLayout, ItemKind, MediaFile, MediaStream, MediaVersion, MetadataItem,
    SidePicture, SideStream, SideStreamKind, StatedFile, StatedTrack, StreamKind, TrackTraits,
    VideoAnalysis,
};
use open_plex_core::testing::{InMemoryAccess, InMemoryLibrary, InMemoryMetadata, StubStreams};
use open_plex_core::{
    LibrarySection, MediaId, PartId, RatingKey, SectionId, SectionKind, ServerCapabilities,
    ServerIdentity, StreamId, UserId,
};
use serde_json::Value;
use tower::ServiceExt;

pub const PASSWORD: &str = "correct horse";
pub const MACHINE: &str = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";

pub const FILMS: i64 = 1;
pub const SERIES: i64 = 2;
/// A library nobody but an owner was given.
pub const PRIVATE: i64 = 3;

pub const DOODLEBUG: i64 = 101;
pub const HIDDEN_FILM: i64 = 102;
pub const SHOW: i64 = 201;
pub const SEASON: i64 = 202;
pub const EPISODE: i64 = 203;

pub const DOODLEBUG_FILE: i64 = 2001;
pub const DOODLEBUG_VIDEO: i64 = 3001;
pub const DOODLEBUG_AUDIO: i64 = 3002;
pub const EPISODE_FILE: i64 = 2003;

pub fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
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
        created_at: 1_795_163_514,
        updated_at: 1_795_163_514,
        scanned_at: None,
        content_changed_at: 0,
        hidden: false,
    }
}

fn item(id: i64, section: i64, parent: Option<i64>, kind: ItemKind, title: &str) -> MetadataItem {
    MetadataItem {
        id: RatingKey::new(id),
        section_id: SectionId::new(section),
        parent_id: parent.map(RatingKey::new),
        kind,
        guid: format!("local://{id}"),
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
        added_at: 1_507_388_853,
        updated_at: 1_507_388_853,
        refreshed_at: None,
        chapter_source: None,
    }
}

fn items() -> Vec<MetadataItem> {
    let mut doodlebug = item(DOODLEBUG, FILMS, None, ItemKind::Movie, "doodlebug");
    doodlebug.year = Some(1997);
    doodlebug.duration_ms = Some(178_301);
    doodlebug.studio = Some("Following Productions".to_owned());
    let mut hidden = item(HIDDEN_FILM, PRIVATE, None, ItemKind::Movie, "Hidden");
    hidden.year = Some(2001);
    let mut show = item(SHOW, SERIES, None, ItemKind::Show, "90210");
    show.year = Some(2008);
    show.content_rating = Some("TV-14".to_owned());
    let mut season = item(SEASON, SERIES, Some(SHOW), ItemKind::Season, "Saison 1");
    season.position = Some(1);
    let mut episode = item(
        EPISODE,
        SERIES,
        Some(SEASON),
        ItemKind::Episode,
        "The Dionysian Debacle",
    );
    episode.position = Some(21);
    episode.year = Some(2009);
    episode.duration_ms = Some(287_754);
    episode.summary = Some("Naomi's older sister, Jen, unexpectedly returns.".to_owned());
    vec![doodlebug, hidden, show, season, episode]
}

pub const THRILLER: i64 = 401;
pub const NOLAN_DIRECTOR: i64 = 402;
pub const NOLAN: i64 = 501;

/// `doodlebug`'s genre and director, as the import credits them.
fn tags() -> Vec<open_plex_core::metadata::ItemTag> {
    use open_plex_core::metadata::{ItemTag, Person, Tag, TagKind};
    use open_plex_core::{PersonId, TagId};
    let tag = |id: i64, kind, name: &str, person: Option<Person>| ItemTag {
        item_id: RatingKey::new(DOODLEBUG),
        tag: Tag {
            id: TagId::new(id),
            kind,
            name: name.to_owned(),
            person,
        },
        role: None,
    };
    vec![
        tag(THRILLER, TagKind::Genre, "Thriller", None),
        tag(
            NOLAN_DIRECTOR,
            TagKind::Director,
            "Christopher Nolan",
            Some(Person {
                id: PersonId::new(NOLAN),
                name: "Christopher Nolan".to_owned(),
                provider_key: None,
                thumb_url: None,
            }),
        ),
    ]
}

pub const INTRO: i64 = 601;

/// The episode's intro and final credits, as an analysis found them.
fn markers() -> Vec<open_plex_core::marker::Marker> {
    use open_plex_core::marker::{Marker, MarkerKind};
    let marker = |id: i64, kind, start_ms, end_ms| Marker {
        id: open_plex_core::MarkerId::new(id),
        item: RatingKey::new(EPISODE),
        kind,
        owner: None,
        start_ms,
        end_ms: Some(end_ms),
        is_final: kind == MarkerKind::Credits,
        version: Some(5),
        title: None,
        color: None,
    };
    vec![
        marker(INTRO, MarkerKind::Intro, 12_000, 61_500),
        marker(602, MarkerKind::Credits, 2_830_000, 2_877_000),
    ]
}

fn version(id: i64, item: i64, width: i32, height: i32) -> MediaVersion {
    MediaVersion {
        id: MediaId::new(id),
        item_id: RatingKey::new(item),
        width: Some(width),
        height: Some(height),
        aspect_ratio: Some(1.5),
        duration_ms: Some(178_301),
        bitrate: Some(667),
        container: Some("mkv".to_owned()),
        video_codec: Some("h264".to_owned()),
        audio_codec: Some("opus".to_owned()),
        audio_channels: Some(2),
        frame_rate: Some(29.97),
        interlaced: false,
        optimized_for_streaming: false,
    }
}

/// What ffprobe states of a track, as the scan keeps it.
fn stated(language: Option<&str>, time_base: &str, codec_tag: &str) -> StatedTrack {
    StatedTrack {
        language: language.map(ToOwned::to_owned),
        time_base: Some(time_base.to_owned()),
        codec_tag: Some(codec_tag.to_owned()),
        ..StatedTrack::default()
    }
}

/// What ffprobe states of a file, as the scan keeps it.
fn stated_file(format_name: &str, duration_us: i64, bitrate: i64) -> StatedFile {
    StatedFile {
        format_name: Some(format_name.to_owned()),
        duration_us: Some(duration_us),
        bitrate: Some(bitrate),
    }
}

fn file(id: i64, version: i64, path: &str, size: i64) -> MediaFile {
    MediaFile {
        id: PartId::new(id),
        version_id: MediaId::new(version),
        position: 0,
        path: path.to_owned(),
        size_bytes: Some(size),
        duration_ms: Some(178_301),
        layout: FileLayout::default(),
        stream_url: None,
        content_hash: None,
        open_subtitles_hash: None,
        stated: StatedFile::default(),
    }
}

fn track(id: i64, file: i64, kind: StreamKind, position: i32, codec: &str) -> MediaStream {
    MediaStream {
        id: StreamId::new(id),
        file_id: PartId::new(file),
        kind,
        position,
        codec: Some(codec.to_owned()),
        language: None,
        channels: None,
        bitrate: None,
        is_default: false,
        is_forced: false,
        external_path: None,
        title: None,
        stream_identifier: Some(i64::from(position)),
        profile: None,
        bit_depth: None,
        traits: TrackTraits::default(),
        video: None,
        audio: None,
        subtitle: None,
        required_bandwidths: Vec::new(),
        stated: StatedTrack::default(),
    }
}

/// `doodlebug` as our scan stores it: 720×480 H.264 and an English Opus track, and what
/// ffprobe states of them.
fn doodlebug_tracks() -> Vec<MediaStream> {
    let mut video = track(
        DOODLEBUG_VIDEO,
        DOODLEBUG_FILE,
        StreamKind::Video,
        0,
        "h264",
    );
    video.bitrate = Some(539);
    video.profile = Some("main".to_owned());
    video.bit_depth = Some(8);
    video.is_default = true;
    video.stated = StatedTrack {
        handler_name: Some("VideoHandler".to_owned()),
        ..stated(Some("und"), "1/1000", "[0][0][0][0]")
    };
    video.video = Some(Box::new(VideoAnalysis {
        width: Some(720),
        height: Some(480),
        frame_rate: Some(29.97),
        level: Some(30),
        ref_frames: Some(1),
        anamorphic: Some(false),
        chroma_subsampling: Some("4:2:0".to_owned()),
        average_frame_rate: Some("30000/1001".to_owned()),
        real_frame_rate: Some("30000/1001".to_owned()),
        display_aspect_ratio: Some("3:2".to_owned()),
        sample_aspect_ratio: Some("1:1".to_owned()),
        cropped: Some(false),
        pixel_format: Some("yuv420p".to_owned()),
        is_avc: Some(true),
        nal_length_size: Some(4),
        ..VideoAnalysis::default()
    }));
    let mut audio = track(
        DOODLEBUG_AUDIO,
        DOODLEBUG_FILE,
        StreamKind::Audio,
        1,
        "opus",
    );
    audio.language = Some("en".to_owned());
    audio.channels = Some(2);
    audio.bitrate = Some(128);
    audio.is_default = true;
    audio.stated = stated(Some("eng"), "1/1000", "[0][0][0][0]");
    audio.audio = Some(AudioAnalysis {
        sampling_rate: Some(48_000),
        channel_layout: Some("stereo".to_owned()),
        loudness: None,
    });
    vec![video, audio]
}

fn metadata(items: Vec<MetadataItem>) -> InMemoryMetadata {
    // The episode's file, as the scan stores an MP4 of the reference's library.
    let mut episode_video = track(3101, EPISODE_FILE, StreamKind::Video, 0, "h264");
    episode_video.bitrate = Some(1_376_000);
    episode_video.bit_depth = Some(8);
    episode_video.profile = Some("main".to_owned());
    episode_video.is_default = true;
    episode_video.stated = StatedTrack {
        bitrate: Some(1_510_916),
        ..stated(Some("und"), "1/30000", "avc1")
    };
    episode_video.video = Some(Box::new(VideoAnalysis {
        width: Some(720),
        height: Some(404),
        frame_rate: Some(29.97),
        level: Some(30),
        ref_frames: Some(1),
        chroma_subsampling: Some("4:2:0".to_owned()),
        average_frame_rate: Some("30000/1001".to_owned()),
        real_frame_rate: Some("30000/1001".to_owned()),
        cropped: Some(false),
        pixel_format: Some("yuv420p".to_owned()),
        is_avc: Some(true),
        nal_length_size: Some(4),
        ..VideoAnalysis::default()
    }));
    let mut episode_audio = track(3102, EPISODE_FILE, StreamKind::Audio, 1, "aac");
    episode_audio.channels = Some(6);
    episode_audio.is_default = true;
    episode_audio.profile = Some("lc".to_owned());
    episode_audio.stated = StatedTrack {
        name: Some("aac 5.1".to_owned()),
        bitrate: Some(448_006),
        ..stated(Some("und"), "1/48000", "mp4a")
    };
    episode_audio.audio = Some(AudioAnalysis {
        sampling_rate: Some(48_000),
        channel_layout: Some("5.1".to_owned()),
        loudness: None,
    });
    // Its cover, a picture stored as a third video track.
    let cover = SideStream {
        file_id: PartId::new(EPISODE_FILE),
        kind: SideStreamKind::Cover,
        position: 2,
        codec: Some("bmp".to_owned()),
        codec_tag: Some("[0][0][0][0]".to_owned()),
        profile: None,
        language: None,
        time_base: Some("1/90000".to_owned()),
        mime_type: None,
        filename: None,
        picture: Some(SidePicture {
            width: Some(640),
            height: Some(360),
            pixel_format: Some("bgra".to_owned()),
            real_frame_rate: Some("90000/1".to_owned()),
            average_frame_rate: Some("0/0".to_owned()),
            ..SidePicture::default()
        }),
        is_default: false,
        is_forced: false,
    };
    let mut tracks = doodlebug_tracks();
    tracks.extend([episode_video, episode_audio]);
    InMemoryMetadata::new(items)
        .with_media(
            vec![
                version(1001, DOODLEBUG, 720, 480),
                version(1003, EPISODE, 720, 404),
            ],
            vec![
                MediaFile {
                    stated: stated_file("matroska,webm", 178_301_000, 667_480),
                    ..file(
                        DOODLEBUG_FILE,
                        1001,
                        "/media/Movies/VO/doodlebug.1997.VO.H264.mkv",
                        14_876_546,
                    )
                },
                MediaFile {
                    stated: stated_file("mov,mp4,m4a,3gp,3g2,mj2", 287_754_134, 1_986_929),
                    ..file(
                        EPISODE_FILE,
                        1003,
                        "/media/TVShows/VF/90210/S01E21.mp4",
                        71_468_407,
                    )
                },
            ],
            tracks,
        )
        .with_side_streams(vec![cover])
        .with_tags(tags())
        .with_markers(markers())
}

pub struct Harness {
    pub schema: ApiSchema,
    pub access: Arc<InMemoryAccess>,
    pub streams: Arc<StubStreams>,
    sessions: Arc<open_plex_api::Sessions>,
    pub alice: UserId,
}

pub struct Answer {
    pub status: StatusCode,
    pub headers: axum::http::HeaderMap,
    pub body: Vec<u8>,
}

impl Answer {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&self.body)))
    }

    pub fn content_type(&self) -> Option<&str> {
        self.headers
            .get("content-type")
            .and_then(|value| value.to_str().ok())
    }
}

/// The server's authentication, as it applies to the Jellyfin surface: a Jellyfin token, or
/// nobody.
async fn jellyfin_tokens(
    axum::extract::State(access): axum::extract::State<Arc<InMemoryAccess>>,
    mut request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path().to_owned();
    let rest = open_plex_api_jellyfin::surface_path(&path).unwrap_or("");
    let token = open_plex_api_jellyfin::auth::token(request.headers(), request.uri().query());
    let grant = match token {
        Some(token) => access
            .grant(Extension::Jellyfin, &token_digest(&token))
            .await
            .unwrap(),
        None => None,
    };
    match grant {
        Some(grant) => {
            request.extensions_mut().insert(grant.into_principal());
            next.run(request).await
        }
        None if open_plex_api_jellyfin::auth::is_public(rest) => next.run(request).await,
        None => StatusCode::UNAUTHORIZED.into_response(),
    }
}

/// The films, the series and a library nobody but an owner was given.
fn library() -> InMemoryLibrary {
    InMemoryLibrary::new(vec![
        section(FILMS, SectionKind::Movie, "Films"),
        section(SERIES, SectionKind::Show, "Séries"),
        section(PRIVATE, SectionKind::Movie, "Private"),
    ])
    .with_roots(
        [(1, FILMS, "/media/Movies"), (2, SERIES, "/media/TVShows")]
            .into_iter()
            .map(|(id, section, path)| open_plex_core::SectionRoot {
                id,
                section_id: SectionId::new(section),
                path: path.to_owned(),
                created_at: 0,
            })
            .collect(),
    )
}

impl Harness {
    /// `alice`, a user given the films and the series and connected through Jellyfin;
    /// `bob`, given the same, connected through Plex only; `carol`, whose Jellyfin connection
    /// was switched off; `owner`, an owner connected through Jellyfin. All with one password.
    pub async fn new() -> Self {
        Self::with_items(items()).await
    }

    /// The same, with the episode released at this time.
    pub async fn with_episode_released(at: i64) -> Self {
        let mut items = items();
        for item in &mut items {
            if item.id == RatingKey::new(EPISODE) {
                item.released_on = Some(at);
            }
        }
        Self::with_items(items).await
    }

    async fn with_items(items: Vec<MetadataItem>) -> Self {
        let access = Arc::new(InMemoryAccess::default());
        let hash = open_plex_core::account::hash_password(PASSWORD).unwrap();
        let mut ids = HashMap::new();
        for (name, role) in [
            ("alice", ServerRole::User),
            ("bob", ServerRole::User),
            ("carol", ServerRole::User),
            ("owner", ServerRole::Owner),
        ] {
            let id = access
                .create_user(
                    NewUser {
                        username: name,
                        email: None,
                        role,
                        active: true,
                        password_hash: Some(&hash),
                    },
                    0,
                )
                .await
                .unwrap()
                .id;
            ids.insert(name, id);
        }
        for name in ["alice", "bob", "carol"] {
            for section in [FILMS, SERIES] {
                access
                    .set_permission(
                        LibraryPermission {
                            user: ids[name],
                            section: SectionId::new(section),
                            role: LibraryRole::Viewer,
                        },
                        0,
                    )
                    .await
                    .unwrap();
            }
        }
        for (name, extension) in [
            ("alice", Extension::Jellyfin),
            ("bob", Extension::Plex),
            ("carol", Extension::Jellyfin),
            ("owner", Extension::Jellyfin),
        ] {
            access
                .connect_extension(ids[name], extension, name, &token_digest(name), 0)
                .await
                .unwrap();
        }
        access
            .disable_extension(ids["carol"], Extension::Jellyfin, 0)
            .await
            .unwrap();
        let streams = Arc::new(StubStreams::default());
        let library = library();
        let collections = Arc::new(open_plex_core::testing::InMemoryCollections::new(
            items.clone(),
        ));
        let schema = build_schema(ApiContext {
            capabilities: ServerCapabilities::new(ServerIdentity::new(MACHINE, "0.1.0"), 0),
            library: Arc::new(library),
            metadata: Arc::new(metadata(items.clone()).with_collections(&collections)),
            playback: Arc::new(open_plex_core::testing::InMemoryPlayback::default()),
            play_queues: Arc::new(open_plex_core::testing::InMemoryPlayQueues::default()),
            playlists: Arc::new(open_plex_core::testing::InMemoryPlaylists::new(items)),
            collections,
            access: Arc::clone(&access) as Arc<dyn AccessRepository>,
            directory: Arc::new(open_plex_core::testing::StubDirectory::default()),
            watch: Arc::new(open_plex_core::testing::InMemoryWatch::default()),
            streams: Arc::clone(&streams) as Arc<dyn open_plex_core::stream::StreamEngine>,
            session_locks: Arc::default(),
            scans: Arc::new(open_plex_core::testing::RecordedScans::default()),
        });
        let sessions = Arc::new(open_plex_api::Sessions::new(
            Arc::new(open_plex_core::account::SessionKey::generate()),
            true,
        ));
        Self {
            schema,
            access,
            streams,
            sessions,
            alice: ids["alice"],
        }
    }

    fn app(&self) -> Router {
        let state = JellyfinState::new(
            self.schema.clone(),
            Arc::clone(&self.sessions),
            &ServerIdentity::new(MACHINE, "0.1.0"),
        );
        router(state).layer(axum::middleware::from_fn_with_state(
            Arc::clone(&self.access),
            jellyfin_tokens,
        ))
    }

    pub fn server_id() -> String {
        open_plex_api_jellyfin::ServerInfo::of(&ServerIdentity::new(MACHINE, "0.1.0")).id
    }

    pub async fn send(
        &self,
        method: Method,
        uri: &str,
        headers: &[(&str, String)],
        body: Option<Value>,
    ) -> Answer {
        let mut request = Request::builder().method(method).uri(uri);
        for (name, value) in headers {
            request = request.header(*name, value);
        }
        let body = match body {
            Some(body) => {
                request = request.header("content-type", "application/json");
                Body::from(body.to_string())
            }
            None => Body::empty(),
        };
        let response = self
            .app()
            .oneshot(request.body(body).unwrap())
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

    /// Signs a user in from a device as Jellyfin Web does.
    pub async fn sign_in_as(&self, username: &str, device: &str) -> Answer {
        self.send(
            Method::POST,
            "/jellyfin/Users/AuthenticateByName",
            &[("authorization", client_header(device, None))],
            Some(serde_json::json!({ "Username": username, "Pw": PASSWORD })),
        )
        .await
    }

    /// `alice`'s token on a new device.
    pub async fn token(&self) -> String {
        let answer = self.sign_in_as("alice", "device-1").await;
        assert_eq!(answer.status, StatusCode::OK);
        answer.json()["AccessToken"].as_str().unwrap().to_owned()
    }

    pub async fn get(&self, uri: &str, token: &str) -> Answer {
        self.send(
            Method::GET,
            uri,
            &[("authorization", client_header("device-1", Some(token)))],
            None,
        )
        .await
    }

    pub async fn post(&self, uri: &str, token: &str, body: Value) -> Answer {
        self.send(
            Method::POST,
            uri,
            &[("authorization", client_header("device-1", Some(token)))],
            Some(body),
        )
        .await
    }
}

impl Harness {
    /// A request without a body, as Jellyfin's clients send most changes.
    pub async fn bare(&self, method: Method, uri: &str, token: &str) -> Answer {
        self.send(
            method,
            uri,
            &[("authorization", client_header("device-1", Some(token)))],
            None,
        )
        .await
    }
}

pub fn client_header(device: &str, token: Option<&str>) -> String {
    let mut header = format!(
        r#"MediaBrowser Client="Jellyfin Web", Device="Firefox", DeviceId="{device}", Version="10.11.0""#
    );
    if let Some(token) = token {
        header = format!(r#"{header}, Token="{token}""#);
    }
    header
}

pub fn jellyfin_id(type_name: &str, key: i64) -> String {
    open_plex_api_jellyfin::id::of(type_name, key).unwrap()
}

/// Every key the reference sent that we do not, and every key we send that it does not, at
/// every depth of objects (the first element standing for each array), with each value's JSON
/// type compared where both have it. `allowed` names the keys either side may lack, by name.
pub fn shape_differences(ours: &Value, reference: &Value, allowed: &[&str]) -> Vec<String> {
    let mut differences = Vec::new();
    compare(ours, reference, "", allowed, &mut differences);
    differences
}

fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn compare(ours: &Value, reference: &Value, at: &str, allowed: &[&str], out: &mut Vec<String>) {
    match (ours, reference) {
        (Value::Object(ours), Value::Object(reference)) => {
            for (key, value) in reference {
                match ours.get(key) {
                    Some(mine) => compare(mine, value, &format!("{at}.{key}"), allowed, out),
                    None if allowed.contains(&key.as_str()) => {}
                    None => out.push(format!("missing {at}.{key}")),
                }
            }
            for key in ours.keys() {
                if !reference.contains_key(key) && !allowed.contains(&key.as_str()) {
                    out.push(format!("extra {at}.{key}"));
                }
            }
        }
        (Value::Array(ours), Value::Array(reference)) => {
            if let (Some(mine), Some(theirs)) = (ours.first(), reference.first()) {
                compare(mine, theirs, &format!("{at}[0]"), allowed, out);
            }
        }
        // A null where the reference has a value is fine both ways: optional fields.
        (Value::Null, _) | (_, Value::Null) => {}
        (ours, reference) if kind(ours) != kind(reference) => out.push(format!(
            "{at}: {} where the reference has {}",
            kind(ours),
            kind(reference)
        )),
        _ => {}
    }
}
