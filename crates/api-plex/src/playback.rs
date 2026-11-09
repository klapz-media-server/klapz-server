//! Playback: a file served as it is, or remuxed over HLS.
//!
//! The binding over `open-plex-transcoder`, as `photo` is. It covers Direct Play, Direct Stream
//! and H.264 re-encoded to a lower quality. Video of any other codec answers `501`.
//!
//! `/video/:/transcode/session/…` is the process's own way back in. Those routes are served
//! without a token and answer only to the unguessable id its session was given.
//!
//! A stream's segments carry no token either: the manifest names them without one, and players
//! fetch them as written, as on the reference. Knowing a session's name is what opens them, so
//! a manifest gives its stored name, the one that says whose session it is.

use std::fmt::Write as _;
use std::path::PathBuf;

use async_graphql::{Name, Value, Variables};
use axum::body::Body;
use axum::extract::{Path, Query, RawQuery, Request, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::Router;
use open_plex_api::item::ITEM_TYPE;
use open_plex_api::media::FILE_TYPE;
use open_plex_api::GlobalId;
use open_plex_transcoder::media::resolve_within;
use open_plex_transcoder::plan::{
    plan_stream, AudioAction, Limits, StreamPlan, StreamSource, Track, VideoAction, VideoEncode,
};
use open_plex_transcoder::session::{segment_seconds, Protocol, StreamRequest};
use serde::Deserialize;
use serde_json::{json, Value as Json};
use tower::ServiceExt as _;
use tower_http::services::ServeFile;

use crate::auth::Caller;
use crate::client::{ClientId, ClientIdentity};
use crate::error::ApiError;
use crate::gql;
use crate::gql_items::{GqlItem, GqlPart, GqlStream, GqlVersion, ItemsByIdData, ITEMS_BY_ID_QUERY};
use crate::items::{MetadataEntry, LIBRARY_IDENTIFIER, MEDIA_TAG_PREFIX, MEDIA_TAG_VERSION};
use crate::language::Language;
use crate::response::{PlexResponse, ResponseFormat};
use crate::PlexState;

/// Registered apart from the merged routers: these paths hold a segment starting with a colon.
pub fn add_routes(router: Router<PlexState>) -> Router<PlexState> {
    router
        .route("/video/:/transcode/universal/decision", get(decision))
        .route("/video/:/transcode/universal/start.m3u8", get(start_hls))
        .route("/video/:/transcode/universal/start.mpd", get(start_dash))
        .route("/video/:/transcode/universal/ping", get(ping))
        .route("/video/:/transcode/universal/stop", get(stop))
        .route(
            "/video/:/transcode/universal/session/{session}/{base}/{file}",
            get(session_file),
        )
        .route(
            "/video/:/transcode/session/{session}/{callback}/source",
            get(callback_source),
        )
        .route(
            "/video/:/transcode/session/{session}/{callback}/manifest",
            post(callback_manifest),
        )
        .route(
            "/video/:/transcode/session/{session}/{callback}/progress",
            put(acknowledge),
        )
        .route(
            "/video/:/transcode/session/{session}/{callback}/progress/{detail}",
            put(acknowledge),
        )
        .route(
            "/library/parts/{partId}/{changestamp}/{filename}",
            get(part_file),
        )
}

const PATHS_QUERY: &str = r"
query PlexFilePaths($id: ID!) {
  node(id: $id) { ... on MediaFile { path } }
  libraries(first: 500) { edges { node { roots { path } } } }
}
";

#[derive(Debug, Deserialize)]
struct PathsData {
    node: Option<GqlPath>,
    libraries: GqlLibraries,
}

#[derive(Debug, Deserialize)]
struct GqlPath {
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GqlLibraries {
    edges: Vec<GqlLibraryEdge>,
}

#[derive(Debug, Deserialize)]
struct GqlLibraryEdge {
    node: GqlRoots,
}

#[derive(Debug, Deserialize)]
struct GqlRoots {
    roots: Vec<GqlRoot>,
}

#[derive(Debug, Deserialize)]
struct GqlRoot {
    path: String,
}

/// A part's file on disk, by id: no route takes a path, and this one is checked against the
/// libraries even so.
async fn file_of_part(state: &PlexState, part: i64) -> Result<PathBuf, ApiError> {
    let mut variables = Variables::default();
    variables.insert(
        Name::new("id"),
        Value::String(GlobalId::encode(FILE_TYPE, part)),
    );
    let data: PathsData = gql::execute(&state.schema, PATHS_QUERY, variables).await?;
    let path = data
        .node
        .and_then(|node| node.path)
        .ok_or(ApiError::NotFound)?;
    let roots: Vec<String> = data
        .libraries
        .edges
        .into_iter()
        .flat_map(|edge| edge.node.roots)
        .map(|root| root.path)
        .collect();
    Ok(resolve_within(&path, &roots).await?)
}

/// `Range`, conditional requests and the content type come from `ServeFile`.
async fn serve(path: PathBuf, request: Request) -> Response {
    match ServeFile::new(path).oneshot(request).await {
        Ok(response) => response.map(Body::new),
        Err(never) => match never {},
    }
}

/// `GET /library/parts/{id}/{changestamp}/{filename}`: Direct Play. The file name in the URL
/// is cosmetic, and is never looked at.
async fn part_file(
    _caller: Caller,
    State(state): State<PlexState>,
    Path((part, _changestamp, _filename)): Path<(i64, String, String)>,
    request: Request,
) -> Result<Response, ApiError> {
    let path = file_of_part(&state, part).await?;
    Ok(serve(path, request).await)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlaybackParams {
    path: Option<String>,
    #[serde(rename = "mediaIndex")]
    media_index: Option<i64>,
    #[serde(rename = "partIndex")]
    part_index: Option<i64>,
    protocol: Option<String>,
    #[serde(rename = "directPlay")]
    direct_play: Option<String>,
    #[serde(rename = "hasMDE")]
    has_mde: Option<String>,
    session: Option<String>,
    offset: Option<f64>,
    #[serde(rename = "audioStreamID")]
    audio_stream_id: Option<i64>,
    #[serde(rename = "directStreamAudio")]
    direct_stream_audio: Option<String>,
    /// In kilobits per second.
    #[serde(rename = "maxVideoBitrate")]
    max_video_bitrate: Option<u32>,
    #[serde(rename = "autoAdjustQuality")]
    auto_adjust_quality: Option<String>,
}

impl PlaybackParams {
    /// With `hasMDE`, a client has already judged for itself that it can play the file, and
    /// the reference does not second-guess it: `directplay` even for MPEG-4 in AVI.
    fn wants_direct_play(&self) -> bool {
        self.direct_play.as_deref() == Some("1") && self.has_mde.as_deref() == Some("1")
    }

    fn rating_key(&self) -> Result<i64, ApiError> {
        self.path
            .as_deref()
            .and_then(|path| path.strip_prefix("/library/metadata/"))
            .and_then(|key| key.parse().ok())
            .ok_or(ApiError::BadRequest)
    }

    fn protocol(&self) -> Result<Protocol, ApiError> {
        match self.protocol.as_deref() {
            None | Some("hls") => Ok(Protocol::Hls),
            Some("dash") => Ok(Protocol::Dash),
            Some(_) => Err(ApiError::NotImplemented),
        }
    }

    fn adaptive(&self) -> bool {
        self.auto_adjust_quality.as_deref() == Some("1")
    }

    /// `-1` is how a client says "the default".
    fn index_of(value: Option<i64>) -> usize {
        usize::try_from(value.unwrap_or(0)).unwrap_or(0)
    }
}

pub(crate) async fn item(state: &PlexState, key: i64) -> Result<GqlItem, ApiError> {
    let mut variables = Variables::default();
    variables.insert(
        Name::new("ids"),
        Value::List(vec![Value::String(GlobalId::encode(ITEM_TYPE, key))]),
    );
    let data: ItemsByIdData = gql::execute(&state.schema, &ITEMS_BY_ID_QUERY, variables).await?;
    data.items
        .into_iter()
        .flatten()
        .next()
        .ok_or(ApiError::BadRequest)
}

/// The file a request means, and its tracks.
struct Selection<'a> {
    version: &'a GqlVersion,
    part: &'a GqlPart,
    video: &'a GqlStream,
    audio: &'a GqlStream,
    version_index: usize,
}

impl<'a> Selection<'a> {
    fn of(item: &'a GqlItem, params: &PlaybackParams) -> Result<Self, ApiError> {
        let version_index = PlaybackParams::index_of(params.media_index);
        let version = item
            .versions
            .get(version_index)
            .ok_or(ApiError::BadRequest)?;
        let part = version
            .files
            .get(PlaybackParams::index_of(params.part_index))
            .ok_or(ApiError::BadRequest)?;
        let of_kind = |kind: &'a str| part.streams.iter().filter(move |s| s.kind == kind);
        let video = of_kind("VIDEO").next().ok_or(ApiError::BadRequest)?;
        let audio = params
            .audio_stream_id
            .and_then(|id| of_kind("AUDIO").find(|stream| stream.database_id() == id))
            .or_else(|| of_kind("AUDIO").find(|stream| stream.is_default))
            .or_else(|| of_kind("AUDIO").next())
            .ok_or(ApiError::BadRequest)?;
        Ok(Self {
            version,
            part,
            video,
            audio,
            version_index,
        })
    }

    fn plan(&self, params: &PlaybackParams) -> Result<StreamPlan, ApiError> {
        let source = StreamSource {
            video_codec: self.video.codec.as_deref().unwrap_or_default(),
            audio_codec: self.audio.codec.as_deref().unwrap_or_default(),
            width: self.version.width,
            height: self.version.height,
            bits_per_second: self.version.bitrate,
            frame_rate: self.version.frame_rate,
        };
        let limits = Limits {
            max_video_kbps: params.max_video_bitrate,
            convert_audio: params.direct_stream_audio.as_deref() == Some("0"),
            adaptive: params.adaptive(),
        };
        Ok(plan_stream(&source, limits)?)
    }
}

fn track(stream: &GqlStream) -> Track {
    Track {
        index: stream.position,
        codec: stream.codec.clone().unwrap_or_default(),
        language: stream.language.clone(),
    }
}

/// What happens to a file that is not played directly.
type Conversion = (StreamPlan, Protocol);

const fn container_of(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::Hls => "mpegts",
        Protocol::Dash => "mp4",
    }
}

const fn protocol_name(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::Hls => "hls",
        Protocol::Dash => "dash",
    }
}

const DIRECT_PLAY_OK: &str = "Direct play OK.";
const DIRECT_PLAY_DISABLED: &str = "App cannot direct play this item. Direct play is disabled.";
const CONVERSION_OK: &str = "Direct play not available; Conversion OK.";

/// `GET /video/:/transcode/universal/decision`.
///
/// Built from the item as a lookup shows it, then narrowed to the chosen version and marked,
/// as the reference's answer is: the same document with a decision on each level.
async fn decision(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    Query(params): Query<PlaybackParams>,
    format: ResponseFormat,
) -> Result<PlexResponse<Json>, ApiError> {
    let protocol = params.protocol()?;
    let item = item(&state, params.rating_key()?).await?;
    let selection = Selection::of(&item, &params)?;
    let library = item.library.as_ref().ok_or(ApiError::BadRequest)?;
    let direct_play = params.wants_direct_play();
    let action = if direct_play {
        None
    } else {
        Some((selection.plan(&params)?, protocol))
    };

    let entry = MetadataEntry::from_graphql(&item, true, Language::of(&client))
        .ok_or(ApiError::BadRequest)?;
    let mut metadata =
        serde_json::to_value(entry).map_err(|error| ApiError::Internal(Box::new(error)))?;
    mark_decision(&mut metadata, &selection, action);

    let mut body = json!({ "size": 1 });
    let fields = body.as_object_mut().ok_or(ApiError::BadRequest)?;
    if !direct_play && params.adaptive() {
        fields.insert("abr".into(), json!(true));
    }
    fields.insert("allowSync".into(), json!("1"));
    if direct_play {
        fields.insert("mdeDecisionCode".into(), json!(1000));
        fields.insert("mdeDecisionText".into(), json!(DIRECT_PLAY_OK));
    } else {
        fields.insert("directPlayDecisionCode".into(), json!(3000));
        fields.insert("directPlayDecisionText".into(), json!(DIRECT_PLAY_DISABLED));
        fields.insert("generalDecisionCode".into(), json!(1001));
        fields.insert("generalDecisionText".into(), json!(CONVERSION_OK));
    }
    fields.insert("identifier".into(), json!(LIBRARY_IDENTIFIER));
    fields.insert(
        "librarySectionID".into(),
        json!(library.database_id().to_string()),
    );
    fields.insert("librarySectionTitle".into(), json!(library.title));
    fields.insert("librarySectionUUID".into(), json!(library.uuid));
    fields.insert("mediaTagPrefix".into(), json!(MEDIA_TAG_PREFIX));
    fields.insert(
        "mediaTagVersion".into(),
        json!(MEDIA_TAG_VERSION.to_string()),
    );
    if let Some(identifier) = client.identifier.as_ref().map(ClientId::as_str) {
        fields.insert("resourceSession".into(), json!(identifier));
    }
    if !direct_play {
        fields.insert("transcodeDecisionCode".into(), json!(1001));
        fields.insert("transcodeDecisionText".into(), json!(CONVERSION_OK));
    }
    fields.insert("Metadata".into(), json!([metadata]));
    Ok(PlexResponse::new(body, format))
}

/// `None` is Direct Play.
fn mark_decision(metadata: &mut Json, selection: &Selection<'_>, action: Option<Conversion>) {
    let Some(item) = metadata.as_object_mut() else {
        return;
    };
    let Some(mut media) = item
        .get_mut("Media")
        .and_then(Json::as_array_mut)
        .filter(|all| selection.version_index < all.len())
        .map(|all| all.swap_remove(selection.version_index))
    else {
        return;
    };

    let selected_part = selection.part.database_id();
    if let Some(version) = media.as_object_mut() {
        version.insert("selected".into(), json!(true));
        if let Some((plan, protocol)) = action {
            version.insert("container".into(), json!(container_of(protocol)));
            version.insert("protocol".into(), json!(protocol_name(protocol)));
            if plan.audio == AudioAction::ConvertToAac {
                version.insert("audioCodec".into(), json!("aac"));
                version.insert("audioChannels".into(), json!(CONVERTED_CHANNELS));
            }
            if let VideoAction::Encode(encode) = plan.video {
                version.insert("videoCodec".into(), json!("h264"));
                version.insert(
                    "videoResolution".into(),
                    json!(resolution_label(encode.width)),
                );
                mark_encoded_size(version, &encode);
            }
        }
        if let Some(parts) = version.get_mut("Part").and_then(Json::as_array_mut) {
            parts.retain(|part| part["id"] == json!(selected_part));
            for part in parts {
                mark_part(part, selection, action);
            }
        }
    }
    item.insert("Media".into(), json!([media]));
}

/// The conversion always ends in stereo: `ochl='stereo'` in its filter.
const CONVERTED_CHANNELS: i32 = 2;

/// As the reference labels what it produces, unlike the labels of a library's files.
pub(crate) const fn resolution_label(width: u32) -> &'static str {
    match width {
        0..=720 => "SD",
        721..=1280 => "720p",
        _ => "1080p",
    }
}

fn mark_encoded_size(fields: &mut serde_json::Map<String, Json>, encode: &VideoEncode) {
    fields.insert(
        "bitrate".into(),
        json!(encode.video_kbps + encode.audio_kbps),
    );
    fields.insert("width".into(), json!(encode.width));
    fields.insert("height".into(), json!(encode.height));
    fields.insert("optimizedForStreaming".into(), json!(true));
}

/// What the analysis of the source says, and the output no longer matches.
const SOURCE_ONLY_VIDEO_FIELDS: [&str; 12] = [
    "chromaLocation",
    "chromaSubsampling",
    "codedHeight",
    "codedWidth",
    "colorPrimaries",
    "colorRange",
    "colorSpace",
    "hasScalingMatrix",
    "level",
    "profile",
    "refFrames",
    "scanType",
];

fn mark_encoded_video(fields: &mut serde_json::Map<String, Json>, encode: &VideoEncode) {
    for name in SOURCE_ONLY_VIDEO_FIELDS {
        fields.remove(name);
    }
    fields.insert("codec".into(), json!("h264"));
    fields.insert("bitrate".into(), json!(encode.video_kbps));
    fields.insert("width".into(), json!(encode.width));
    fields.insert("height".into(), json!(encode.height));
}

fn mark_converted_audio(fields: &mut serde_json::Map<String, Json>, plan: &StreamPlan) {
    fields.insert("codec".into(), json!("aac"));
    fields.insert("bitrate".into(), json!(plan.audio_kbps()));
    fields.insert("bitrateMode".into(), json!("cbr"));
    fields.insert("channels".into(), json!(CONVERTED_CHANNELS));
}

fn mark_part(part: &mut Json, selection: &Selection<'_>, action: Option<Conversion>) {
    let Some(fields) = part.as_object_mut() else {
        return;
    };
    fields.insert("selected".into(), json!(true));
    fields.insert(
        "decision".into(),
        json!(if action.is_some() {
            "transcode"
        } else {
            "directplay"
        }),
    );
    if let Some((plan, protocol)) = action {
        fields.insert("container".into(), json!(container_of(protocol)));
        fields.insert("protocol".into(), json!(protocol_name(protocol)));
        if let VideoAction::Encode(encode) = plan.video {
            mark_encoded_size(fields, &encode);
        }
        // Nothing to fetch directly: a client that took this key would bypass the session.
        fields.remove("key");
    }
    let Some(streams) = fields.get_mut("Stream").and_then(Json::as_array_mut) else {
        return;
    };
    let video = json!(selection.video.database_id());
    let audio = json!(selection.audio.database_id());
    let subtitle = selection
        .part
        .streams
        .iter()
        .find(|stream| stream.kind == "SUBTITLE" && stream.is_default)
        .map(|stream| json!(stream.database_id()));

    streams.retain(|stream| {
        stream["id"] == video || stream["id"] == audio || Some(&stream["id"]) == subtitle.as_ref()
    });
    for stream in streams {
        let id = stream["id"].clone();
        let Some(fields) = stream.as_object_mut() else {
            continue;
        };
        let is_subtitle = Some(&id) == subtitle.as_ref();
        // No subtitle is ever selected: nothing here records a viewer's choice of one yet, so
        // `subtitles=burn` has nothing to burn and the video can be copied.
        if id == audio {
            fields.insert("selected".into(), json!(true));
        }
        let Some((plan, protocol)) = action else {
            fields.insert("location".into(), json!("direct"));
            continue;
        };
        if is_subtitle {
            fields.insert("decision".into(), json!("ignore"));
            fields.insert("location".into(), json!("sidecar-subs"));
        } else {
            let mut converted = false;
            if id == audio && plan.audio == AudioAction::ConvertToAac {
                mark_converted_audio(fields, &plan);
                converted = true;
            }
            if let (true, VideoAction::Encode(encode)) = (id == video, plan.video) {
                mark_encoded_video(fields, &encode);
                converted = true;
            }
            fields.insert(
                "decision".into(),
                json!(if converted { "transcode" } else { "copy" }),
            );
            let location = match protocol {
                Protocol::Hls => "segments-av",
                Protocol::Dash if id == video => "segments-video",
                Protocol::Dash => "segments-audio",
            };
            fields.insert("location".into(), json!(location));
        }
    }
}

const START_SESSION: &str = r"
mutation PlexStartPlayback($input: StartPlaybackInput!) {
  startPlaybackSession(input: $input) { key }
}
";
const TOUCH_SESSION: &str = r"
mutation PlexTouchPlayback($key: String!) { touchPlaybackSession(key: $key) }
";
const END_SESSION: &str = r"
mutation PlexEndPlayback($key: String!) { endPlaybackSession(key: $key) }
";

async fn record_start(
    state: &PlexState,
    session: &str,
    item: &GqlItem,
    selection: &Selection<'_>,
    plan: &StreamPlan,
    client: &ClientIdentity,
    offset: Option<f64>,
) -> Result<(), ApiError> {
    let optional = |value: Option<String>| value.map_or(Value::Null, Value::String);
    let picture = Picture::of(selection, plan);
    let fields = [
        ("key", Value::String(session.to_owned())),
        ("playbackId", optional(client.session_identifier.clone())),
        ("itemId", Value::String(item.id.clone())),
        ("fileId", Value::String(selection.part.id.clone())),
        (
            "clientIdentifier",
            optional(client.identifier.as_ref().map(|id| id.as_str().to_owned())),
        ),
        ("clientProduct", optional(client.product.clone())),
        (
            "videoDecision",
            Value::Enum(Name::new(match plan.video {
                VideoAction::Copy => "COPY",
                VideoAction::Encode(_) => "TRANSCODE",
            })),
        ),
        (
            "audioDecision",
            Value::Enum(Name::new(match plan.audio {
                AudioAction::Copy => "COPY",
                AudioAction::ConvertToAac => "TRANSCODE",
            })),
        ),
        ("positionMs", Value::Number(position_ms(offset).into())),
        (
            "videoBitrate",
            Value::Number(picture.video_bandwidth.into()),
        ),
        (
            "audioBitrate",
            Value::Number(audio_bits(selection, plan).into()),
        ),
        ("width", Value::Number(picture.width.into())),
        ("height", Value::Number(picture.height.into())),
    ];
    let input = fields
        .into_iter()
        .map(|(name, value)| (Name::new(name), value))
        .collect();
    let mut variables = Variables::default();
    variables.insert(Name::new("input"), Value::Object(input));
    let _: serde::de::IgnoredAny = gql::execute(&state.schema, START_SESSION, variables).await?;
    Ok(())
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "a position in seconds, far from the limits of an i64 in milliseconds"
)]
fn position_ms(offset: Option<f64>) -> i64 {
    (offset.unwrap_or(0.0) * 1000.0) as i64
}

/// Records a sign of life. Failing to is no reason to interrupt a film: the session expires
/// a little sooner.
async fn touch_session(state: &PlexState, session: &str) {
    let mut variables = Variables::default();
    variables.insert(Name::new("key"), Value::String(session.to_owned()));
    let touched: Result<serde::de::IgnoredAny, ApiError> =
        gql::execute(&state.schema, TOUCH_SESSION, variables).await;
    if let Err(error) = touched {
        tracing::warn!(%error, "could not record a playback session's sign of life");
    }
}

/// Starts the conversion a request describes, and records the session.
async fn open_session(
    state: &PlexState,
    client: &ClientIdentity,
    params: &PlaybackParams,
    protocol: Protocol,
) -> Result<(String, GqlItem, StreamPlan), ApiError> {
    let session = params.session.clone().ok_or(ApiError::BadRequest)?;
    let item = item(state, params.rating_key()?).await?;
    let selection = Selection::of(&item, params)?;
    let plan = selection.plan(params)?;
    let source = file_of_part(state, selection.part.database_id()).await?;
    let conversion = gql::session_key(&session);

    let replaced = state
        .transcoder
        .start_stream(StreamRequest {
            session: conversion.clone(),
            playback: client.session_identifier.as_deref().map(gql::session_key),
            source,
            video: track(selection.video),
            audio: track(selection.audio),
            plan,
            offset: params.offset,
            protocol,
        })
        .await?;
    for stored in replaced {
        end_record(state, gql::client_key(&stored)).await;
    }
    let recorded = record_start(
        state,
        &session,
        &item,
        &selection,
        &plan,
        client,
        params.offset,
    )
    .await;
    if let Err(error) = recorded {
        // A session nobody can see or stop must not keep running.
        state.transcoder.stop_session(&conversion);
        return Err(error);
    }
    Ok((session, item, plan))
}

/// `GET /video/:/transcode/universal/start.m3u8`: the master playlist, one variant as on the
/// reference.
async fn start_hls(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    Query(params): Query<PlaybackParams>,
) -> Result<Response, ApiError> {
    let (session, item, plan) = open_session(&state, &client, &params, Protocol::Hls).await?;
    let selection = Selection::of(&item, &params)?;

    let picture = Picture::of(&selection, &plan);
    let audio_bits = match plan.audio {
        AudioAction::Copy => 0,
        AudioAction::ConvertToAac => i64::from(plan.audio_kbps()) * 1000,
    };
    let bandwidth = picture.video_bandwidth + audio_bits;
    let resolution = if picture.width > 0 && picture.height > 0 {
        format!(",RESOLUTION={}x{}", picture.width, picture.height)
    } else {
        String::new()
    };
    let frame_rate = selection
        .version
        .frame_rate
        .map_or_else(String::new, |rate| format!(",FRAME-RATE={rate:.6}"));
    let stored = gql::session_key(&session);
    Ok(playlist(format!(
        "#EXTM3U\n#EXT-X-STREAM-INF:PROGRAM-ID=1,BANDWIDTH={bandwidth}{resolution}{frame_rate}\n\
         session/{stored}/base/index.m3u8\n"
    )))
}

/// `GET /video/:/transcode/universal/start.mpd`: the reference's manifest, whole and static.
///
/// A copy's codec strings are known only to the process, and read from the manifest it writes.
/// An encode's are fixed: the reference announces them at once, and so does this.
async fn start_dash(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    Query(params): Query<PlaybackParams>,
) -> Result<Response, ApiError> {
    let (session, item, plan) = open_session(&state, &client, &params, Protocol::Dash).await?;
    let selection = Selection::of(&item, &params)?;
    let written = match plan.video {
        VideoAction::Copy => {
            state
                .transcoder
                .session_manifest(&gql::session_key(&session))
                .await?
        }
        VideoAction::Encode(_) => String::new(),
    };

    let duration = selection.version.duration_ms.unwrap_or(0).max(0);
    let picture = Picture::of(&selection, &plan);
    let audio_bandwidth = audio_bits(&selection, &plan);
    let body = dash_manifest(&DashManifest {
        session: &gql::session_key(&session),
        duration_ms: duration,
        segment_seconds: segment_seconds(&plan, Protocol::Dash),
        video_bandwidth: picture.video_bandwidth,
        width: picture.width,
        height: picture.height,
        audio_bandwidth,
        written: &written,
    });
    Ok(([(header::CONTENT_TYPE, "application/dash+xml")], body).into_response())
}

/// What the audio track weighs on its way to the player.
fn audio_bits(selection: &Selection<'_>, plan: &StreamPlan) -> i64 {
    match plan.audio {
        AudioAction::ConvertToAac => i64::from(plan.audio_kbps()) * 1000,
        AudioAction::Copy => i64::from(selection.audio.bitrate.unwrap_or(0)),
    }
}

/// What a manifest announces of the video: the file's, unless it is being re-encoded.
struct Picture {
    video_bandwidth: i64,
    width: i64,
    height: i64,
}

impl Picture {
    fn of(selection: &Selection<'_>, plan: &StreamPlan) -> Self {
        match plan.video {
            VideoAction::Copy => Self {
                video_bandwidth: i64::from(selection.version.bitrate.unwrap_or(0)),
                width: i64::from(selection.version.width.unwrap_or(0)),
                height: i64::from(selection.version.height.unwrap_or(0)),
            },
            VideoAction::Encode(encode) => Self {
                video_bandwidth: i64::from(encode.video_kbps) * 1000,
                width: i64::from(encode.width),
                height: i64::from(encode.height),
            },
        }
    }
}

struct DashManifest<'a> {
    session: &'a str,
    duration_ms: i64,
    segment_seconds: u32,
    video_bandwidth: i64,
    width: i64,
    height: i64,
    audio_bandwidth: i64,
    /// The manifest the process wrote, which alone states the codecs.
    written: &'a str,
}

/// The value of `name` on the `Representation` element with this id.
fn representation_attribute<'a>(written: &'a str, id: u32, name: &str) -> Option<&'a str> {
    let opening = format!("<Representation id=\"{id}\"");
    let element = &written[written.find(&opening)?..];
    let element = &element[..element.find('>')?];
    let key = format!("{name}=\"");
    let value = &element[element.find(&key)? + key.len()..];
    Some(&value[..value.find('"')?])
}

fn dash_manifest(manifest: &DashManifest<'_>) -> String {
    let session = manifest.session;
    let seconds = manifest.duration_ms / 1000;
    let duration = format!(
        "PT{}H{}M{}S",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    );
    let template = format!(
        "<SegmentTemplate timescale=\"1\" duration=\"{segment}\" \
         initialization=\"session/{session}/$RepresentationID$/header\" \
         media=\"session/{session}/$RepresentationID$/$Number$.m4s\" startNumber=\"0\">\n\
         \t\t\t</SegmentTemplate>",
        segment = manifest.segment_seconds
    );
    let video_codecs =
        representation_attribute(manifest.written, 0, "codecs").unwrap_or("avc1.640028");
    let audio_codecs =
        representation_attribute(manifest.written, 1, "codecs").unwrap_or("mp4a.40.2");
    let sampling =
        representation_attribute(manifest.written, 1, "audioSamplingRate").unwrap_or("48000");
    let channels = manifest
        .written
        .find("audio_channel_configuration")
        .map(|at| &manifest.written[at..])
        .and_then(|rest| rest.split("value=\"").nth(1))
        .and_then(|rest| rest.split('"').next())
        .unwrap_or("2");
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
         <MPD xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"\n\
         \txmlns=\"urn:mpeg:dash:schema:mpd:2011\"\n\
         \txmlns:xlink=\"http://www.w3.org/1999/xlink\"\n\
         \txsi:schemaLocation=\"urn:mpeg:dash:schema:mpd:2011 http://standards.iso.org/ittf/PubliclyAvailableStandards/MPEG-DASH_schema_files/DASH-MPD.xsd\"\n\
         \tprofiles=\"urn:mpeg:dash:profile:isoff-live:2011\"\n\
         \ttype=\"static\"\n\
         \tmediaPresentationDuration=\"{duration}\"\n\
         \tmaxSegmentDuration=\"PT{longest}S\"\n\
         \tminBufferTime=\"PT10S\">\n\
         \t<Period start=\"PT0S\" id=\"0\" duration=\"{duration}\">\n\
         \t\t<AdaptationSet segmentAlignment=\"true\">\n\
         \t\t\t{template}\n\
         \t\t\t<Representation id=\"0\" mimeType=\"video/mp4\" codecs=\"{video_codecs}\" \
         bandwidth=\"{video}\" width=\"{width}\" height=\"{height}\">\n\
         \t\t\t</Representation>\n\
         \t\t</AdaptationSet>\n\
         \t\t<AdaptationSet segmentAlignment=\"true\">\n\
         \t\t\t{template}\n\
         \t\t\t<Representation id=\"1\" mimeType=\"audio/mp4\" codecs=\"{audio_codecs}\" \
         bandwidth=\"{audio}\" audioSamplingRate=\"{sampling}\">\n\
         \t\t\t\t<AudioChannelConfiguration \
         schemeIdUri=\"urn:mpeg:dash:23003:3:audio_channel_configuration:2011\" \
         value=\"{channels}\"/>\n\
         \t\t\t</Representation>\n\
         \t\t</AdaptationSet>\n\
         \t</Period>\n\
         </MPD>\n",
        longest = manifest.segment_seconds * 2,
        video = manifest.video_bandwidth,
        width = manifest.width,
        height = manifest.height,
        audio = manifest.audio_bandwidth,
    )
}

fn playlist(body: String) -> Response {
    (
        [(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")],
        body,
    )
        .into_response()
}

/// `…/session/{id}/base/index.m3u8` and its segments `…/base/00000.ts`.
async fn session_file(
    State(state): State<PlexState>,
    Path((session, base, file)): Path<(String, String, String)>,
    request: Request,
) -> Result<Response, ApiError> {
    if file == "index.m3u8" {
        let output = state.transcoder.session_output(&session).await?;
        return Ok(playlist(media_playlist(&output)));
    }
    // A DASH stream is addressed by its number: `…/0/header`, `…/0/12.m4s`.
    if let Ok(stream) = base.parse::<u32>() {
        let path = if file == "header" {
            state.transcoder.dash_header(&session, stream).await?
        } else {
            let number = file
                .strip_suffix(".m4s")
                .and_then(|stem| stem.parse().ok())
                .ok_or(ApiError::NotFound)?;
            state
                .transcoder
                .dash_chunk(&session, stream, number)
                .await?
        };
        return Ok(serve(path, request).await);
    }
    let number = file
        .strip_suffix(".ts")
        .and_then(|stem| stem.parse().ok())
        .ok_or(ApiError::NotFound)?;
    let path = state.transcoder.session_segment(&session, number).await?;
    Ok(serve(path, request).await)
}

/// A growing playlist, where the reference predicts every segment up front. It can only do
/// that when it controls where they are cut: a remux cuts on the file's own key frames.
fn media_playlist(output: &open_plex_transcoder::session::Output) -> String {
    let longest = output
        .segments
        .iter()
        .map(|segment| segment.seconds)
        .fold(10.0_f64, f64::max);
    let mut text = format!(
        "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:{}\n#EXT-X-ALLOW-CACHE:NO\n\
         #EXT-X-PLAYLIST-TYPE:EVENT\n#EXT-X-MEDIA-SEQUENCE:0\n",
        longest.ceil()
    );
    for segment in &output.segments {
        // Writing to a `String` cannot fail.
        let _ = write!(
            text,
            "#EXTINF:{:.6}, nodesc\n{:05}.ts\n",
            segment.seconds, segment.number
        );
    }
    if output.ended {
        text.push_str("#EXT-X-ENDLIST\n");
    }
    text
}

/// The reference answers `200` to a session it knows and `404` otherwise, for both.
#[derive(Debug, Deserialize)]
struct SessionParams {
    session: Option<String>,
}

async fn ping(
    _caller: Caller,
    State(state): State<PlexState>,
    Query(params): Query<SessionParams>,
) -> StatusCode {
    let Some(session) = params.session else {
        return StatusCode::NOT_FOUND;
    };
    if !state.transcoder.ping_session(&gql::session_key(&session)) {
        return StatusCode::NOT_FOUND;
    }
    touch_session(&state, &session).await;
    StatusCode::OK
}

async fn stop(
    _caller: Caller,
    State(state): State<PlexState>,
    Query(params): Query<SessionParams>,
) -> StatusCode {
    let Some(session) = params.session else {
        return StatusCode::NOT_FOUND;
    };
    if !state.transcoder.stop_session(&gql::session_key(&session)) {
        return StatusCode::NOT_FOUND;
    }
    end_record(&state, session).await;
    StatusCode::OK
}

async fn end_record(state: &PlexState, session: String) {
    let mut variables = Variables::default();
    variables.insert(Name::new("key"), Value::String(session));
    let ended: Result<serde::de::IgnoredAny, ApiError> =
        gql::execute(&state.schema, END_SESSION, variables).await;
    if let Err(error) = ended {
        // It expires by itself.
        tracing::warn!(%error, "could not close a playback session");
    }
}

async fn callback_source(
    State(state): State<PlexState>,
    Path((_session, callback)): Path<(String, String)>,
    request: Request,
) -> Result<Response, ApiError> {
    let path = state
        .transcoder
        .callback_source(&callback)
        .ok_or(ApiError::NotFound)?;
    Ok(serve(path, request).await)
}

async fn callback_manifest(
    State(state): State<PlexState>,
    Path((_session, callback)): Path<(String, String)>,
    body: String,
) -> StatusCode {
    if state.transcoder.announce_segments(&callback, &body) {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    }
}

/// Progress reports. Nothing reads them yet, and the process only needs an answer.
async fn acknowledge(RawQuery(_query): RawQuery) -> StatusCode {
    StatusCode::OK
}

#[cfg(test)]
mod tests {
    use open_plex_transcoder::session::{Output, Segment};

    use super::*;

    fn params(query: &str) -> PlaybackParams {
        let parsed: std::collections::HashMap<String, String> =
            form_urlencoded::parse(query.as_bytes())
                .into_owned()
                .collect();
        PlaybackParams {
            path: parsed.get("path").cloned(),
            media_index: None,
            part_index: None,
            protocol: parsed.get("protocol").cloned(),
            direct_play: parsed.get("directPlay").cloned(),
            has_mde: parsed.get("hasMDE").cloned(),
            session: None,
            offset: None,
            audio_stream_id: None,
            direct_stream_audio: None,
            max_video_bitrate: None,
            auto_adjust_quality: parsed.get("autoAdjustQuality").cloned(),
        }
    }

    #[test]
    fn automatic_quality_is_asked_for_by_one_value() {
        assert!(params("autoAdjustQuality=1").adaptive());
        assert!(!params("autoAdjustQuality=0").adaptive());
        assert!(!params("").adaptive());
    }

    #[test]
    fn direct_play_needs_the_client_to_vouch_for_the_file() {
        assert!(params("directPlay=1&hasMDE=1").wants_direct_play());
        assert!(!params("directPlay=0&hasMDE=1").wants_direct_play());
        assert!(!params("directPlay=1").wants_direct_play());
    }

    #[test]
    fn serves_hls_and_dash_and_refuses_the_rest() {
        assert_eq!(params("protocol=hls").protocol().ok(), Some(Protocol::Hls));
        assert_eq!(params("").protocol().ok(), Some(Protocol::Hls));
        assert_eq!(
            params("protocol=dash").protocol().ok(),
            Some(Protocol::Dash)
        );
        assert!(matches!(
            params("protocol=http").protocol(),
            Err(ApiError::NotImplemented)
        ));
    }

    /// As the process wrote it for a real file.
    const WRITTEN: &str = r#"<Representation id="0" mimeType="video/mp4" codecs="avc1.64001f" bandwidth="3174670" width="1280">
<Representation id="1" mimeType="audio/mp4" codecs="mp4a.40.2" bandwidth="256000" audioSamplingRate="48000">
<AudioChannelConfiguration schemeIdUri="urn:mpeg:dash:23003:3:audio_channel_configuration:2011" value="2" />"#;

    #[test]
    fn reads_the_codecs_the_process_found() {
        assert_eq!(
            representation_attribute(WRITTEN, 0, "codecs"),
            Some("avc1.64001f")
        );
        assert_eq!(
            representation_attribute(WRITTEN, 1, "audioSamplingRate"),
            Some("48000")
        );
        assert_eq!(representation_attribute(WRITTEN, 2, "codecs"), None);
    }

    #[test]
    fn writes_the_references_manifest() {
        let text = dash_manifest(&DashManifest {
            session: "s1",
            duration_ms: 7_200_000,
            segment_seconds: 5,
            video_bandwidth: 4_273_000,
            width: 1920,
            height: 1080,
            audio_bandwidth: 256_000,
            written: WRITTEN,
        });
        assert!(text.contains("type=\"static\""));
        assert!(text.contains("mediaPresentationDuration=\"PT2H0M0S\""));
        assert!(text.contains("maxSegmentDuration=\"PT10S\""));
        assert!(
            text.contains(r#"duration="5" initialization="session/s1/$RepresentationID$/header""#)
        );
        assert!(
            text.contains(r#"media="session/s1/$RepresentationID$/$Number$.m4s" startNumber="0""#)
        );
        assert!(
            text.contains(r#"codecs="avc1.64001f" bandwidth="4273000" width="1920" height="1080""#)
        );
        assert!(text.contains(r#"value="2"/>"#));
    }

    #[test]
    fn reads_the_item_out_of_the_path() {
        assert_eq!(
            params("path=/library/metadata/42").rating_key().ok(),
            Some(42)
        );
        assert!(matches!(
            params("path=/etc/passwd").rating_key(),
            Err(ApiError::BadRequest)
        ));
    }

    #[test]
    fn a_playlist_grows_until_the_process_ends() {
        let segments = vec![
            Segment {
                number: 0,
                seconds: 9.0,
            },
            Segment {
                number: 1,
                seconds: 12.5,
            },
        ];
        let running = media_playlist(&Output {
            segments: segments.clone(),
            ended: false,
        });
        assert!(running.contains("#EXT-X-TARGETDURATION:13\n"));
        assert!(running.contains("#EXTINF:9.000000, nodesc\n00000.ts\n"));
        assert!(!running.contains("ENDLIST"));
        assert!(media_playlist(&Output {
            segments,
            ended: true
        })
        .ends_with("#EXT-X-ENDLIST\n"));
    }
}
