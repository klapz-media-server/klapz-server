//! Playback: a file served as it is, or converted for a player.
//!
//! The binding over `klapz-transcoder`, as `photo` is. It covers Direct Play, Direct Stream
//! and H.264 re-encoded to a lower quality. Video of any other codec answers `501`.
//!
//! A conversion is started through the official API's `startStream`, and `start.mpd` and
//! `start.m3u8` send the player to the stream's own address, where its manifest and segments
//! are served.

use std::path::PathBuf;
use std::sync::Arc;

use async_graphql::{Name, Value, Variables};
use axum::body::Body;
use axum::extract::{Path, Query, Request, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use klapz_api::item::ITEM_TYPE;
use klapz_api::media::FILE_TYPE;
use klapz_api::GlobalId;
use klapz_transcoder::media::resolve_within;
use klapz_transcoder::plan::{
    plan_stream, Adaptation, AudioAction, Limits, StreamPlan, StreamSource, VideoAction,
    VideoEncode,
};
use klapz_transcoder::session::Protocol;
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
        .route("/video/:/transcode/universal/subtitles", get(subtitles))
        // Any other method is the reference's `404`, as for a path it does not serve.
        .route(
            "/{transcodeType}/:/transcode/universal/fallback",
            post(fall_back).fallback(|| async { ApiError::NotFound }),
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
    /// How a client wants subtitles: Plex Web in Chrome asks for `burn`.
    subtitles: Option<String>,
    /// The size to draw them at, in percent: Plex Web's 50 to 200.
    #[serde(rename = "subtitleSize")]
    subtitle_size: Option<u32>,
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

    /// Plex Web's "Convert automatically": the reference announces one quality and changes it
    /// itself.
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
    /// The subtitle the caller plays the file with, if any.
    subtitle: Option<&'a GqlStream>,
    /// How that subtitle reaches the player of a conversion, as the client asked.
    delivery: Option<Delivery>,
    version_index: usize,
}

/// What Plex Web asks for a converted file's subtitle, by its burn setting: `burn`, or `auto`
/// for soft subtitles it draws itself when the server supports them, as this one says it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Delivery {
    Burn,
    /// As ASS beside a DASH stream, read from `/video/:/transcode/universal/subtitles`.
    Text,
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
        let (selected_audio, selected_subtitle) = part.selected();
        let video = of_kind("VIDEO").next().ok_or(ApiError::BadRequest)?;
        // A request naming its audio wins over the caller's choice, which the official API
        // applied, default included.
        let audio = params
            .audio_stream_id
            .or(selected_audio)
            .and_then(|id| of_kind("AUDIO").find(|stream| stream.database_id() == id))
            .or_else(|| of_kind("AUDIO").next())
            .ok_or(ApiError::BadRequest)?;
        let subtitle = selected_subtitle
            .and_then(|id| of_kind("SUBTITLE").find(|stream| stream.database_id() == id));
        Ok(Self {
            version,
            part,
            video,
            audio,
            subtitle,
            delivery: subtitle.and_then(|_| match params.subtitles.as_deref() {
                Some("burn") => Some(Delivery::Burn),
                // Measured with DASH only; an HLS stream's soft subtitles are not captured.
                Some("auto") if params.protocol.as_deref() == Some("dash") => Some(Delivery::Text),
                _ => None,
            }),
            version_index,
        })
    }

    fn plan(&self, params: &PlaybackParams, protocol: Protocol) -> Result<StreamPlan, ApiError> {
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
            adaptation: if params.adaptive() {
                Adaptation::Server
            } else {
                Adaptation::Fixed
            },
            burn_subtitle: self.delivery == Some(Delivery::Burn),
        };
        Ok(plan_stream(&source, protocol, limits)?)
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
        Some((selection.plan(&params, protocol)?, protocol))
    };

    tracing::info!(
        item = params.rating_key()?,
        ?protocol,
        direct_play,
        subtitles = params.subtitles.as_deref().unwrap_or("-"),
        subtitle = ?selection.subtitle.map(GqlStream::database_id),
        delivery = ?selection.delivery,
        "decided a playback"
    );
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
    let subtitle = selection.subtitle.map(|stream| json!(stream.database_id()));

    streams.retain(|stream| {
        stream["id"] == video || stream["id"] == audio || Some(&stream["id"]) == subtitle.as_ref()
    });
    for stream in streams {
        let id = stream["id"].clone();
        let Some(fields) = stream.as_object_mut() else {
            continue;
        };
        let is_subtitle = Some(&id) == subtitle.as_ref();
        // The lookup marks the caller's tracks, which a request naming its audio overrides.
        if id == audio || is_subtitle {
            fields.insert("selected".into(), json!(true));
        } else {
            fields.shift_remove("selected");
        }
        let Some((plan, protocol)) = action else {
            fields.insert("location".into(), json!("direct"));
            continue;
        };
        if is_subtitle && selection.delivery == Some(Delivery::Burn) {
            // Drawn into the picture, so carried by the video's segments.
            fields.insert("decision".into(), json!("burn"));
            fields.insert("location".into(), json!("segments-video"));
            fields.insert("burn".into(), json!("1"));
        } else if is_subtitle && selection.delivery == Some(Delivery::Text) {
            // Converted to ASS, which the player fetches apart and draws.
            fields.insert("codec".into(), json!("ass"));
            fields.insert("container".into(), json!("ass"));
            fields.insert("format".into(), json!("ass"));
            fields.insert("decision".into(), json!("transcode"));
            fields.insert("location".into(), json!("segments-subs"));
        } else if is_subtitle {
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

const TOUCH_SESSION: &str = r"
mutation PlexTouchPlayback($key: String!) { touchPlaybackSession(key: $key) }
";
const END_SESSION: &str = r"
mutation PlexEndPlayback($key: String!) { endPlaybackSession(key: $key) }
";

#[allow(
    clippy::cast_possible_truncation,
    reason = "a position in seconds, far from the limits of an i64 in milliseconds"
)]
fn position_ms(offset: Option<f64>) -> i64 {
    (offset.unwrap_or(0.0) * 1000.0) as i64
}

const START_STREAM: &str = r"
mutation PlexStartStream($input: StartStreamInput!) {
  startStream(input: $input) { stream { url } userErrors { code } }
}
";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartStreamData {
    start_stream: GqlStartStream,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlStartStream {
    stream: Option<GqlStarted>,
    user_errors: Vec<GqlStreamError>,
}

#[derive(Debug, Deserialize)]
struct GqlStarted {
    url: String,
}

#[derive(Debug, Deserialize)]
struct GqlStreamError {
    code: String,
}

/// Starts the conversion a request describes, through the official API, and sends the player
/// to the stream's own address: the manifest is the official API's, not rebuilt here.
async fn start_stream(
    state: &PlexState,
    client: &ClientIdentity,
    params: &PlaybackParams,
    protocol: Protocol,
) -> Result<Response, ApiError> {
    let session = params.session.as_deref().ok_or(ApiError::BadRequest)?;
    let item = item(state, params.rating_key()?).await?;
    let selection = Selection::of(&item, params)?;
    let subtitles = match selection.delivery {
        Some(Delivery::Burn) => "BURN",
        Some(Delivery::Text) => "TEXT",
        None => "NONE",
    };
    let input = json!({
        "file": selection.part.id,
        "protocol": match protocol {
            Protocol::Hls => "HLS",
            Protocol::Dash => "DASH",
        },
        "audioTrack": selection.audio.id,
        "subtitleTrack": selection.subtitle.map(|subtitle| &subtitle.id),
        "subtitles": subtitles,
        "subtitleScale": f64::from(params.subtitle_size.unwrap_or(100)) / 100.0,
        "maxVideoBitrate": params
            .max_video_bitrate
            .map(|kbps| i32::try_from(u64::from(kbps) * 1000).unwrap_or(i32::MAX)),
        "convertAudio": params.direct_stream_audio.as_deref() == Some("0"),
        "quality": if params.adaptive() { "SERVER_ADAPTIVE" } else { "FIXED" },
        "positionMs": position_ms(params.offset),
        "key": session,
        "playbackId": client.session_identifier,
        "clientIdentifier": client.identifier.as_ref().map(ClientId::as_str),
        "clientProduct": client.product,
    });
    let variables = Variables::from_json(json!({ "input": input }));
    let data: StartStreamData = gql::execute(&state.schema, START_STREAM, variables).await?;
    if let Some(error) = data.start_stream.user_errors.first() {
        return Err(match error.code.as_str() {
            "FILE_MISSING" => ApiError::NotFound,
            "VIDEO_NOT_CONVERTIBLE" | "SUBTITLE_NOT_DELIVERABLE" => ApiError::NotImplemented,
            // The reference denies a terminated session's transcodes; its status is unmeasured.
            "SESSION_TERMINATED" => ApiError::Forbidden,
            // The reference answers `400` too when it will not start another stream.
            _ => ApiError::BadRequest,
        });
    }
    let url = data.start_stream.stream.ok_or(ApiError::BadRequest)?.url;
    Ok(Redirect::temporary(&url).into_response())
}

/// `GET /video/:/transcode/universal/start.m3u8`.
async fn start_hls(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    Query(params): Query<PlaybackParams>,
) -> Result<Response, ApiError> {
    start_stream(&state, &client, &params, Protocol::Hls).await
}

/// `GET /video/:/transcode/universal/start.mpd`.
async fn start_dash(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    Query(params): Query<PlaybackParams>,
) -> Result<Response, ApiError> {
    start_stream(&state, &client, &params, Protocol::Dash).await
}

/// `GET /video/:/transcode/universal/subtitles`: the soft subtitles of a running stream, as the
/// reference sends them. The ASS script whole, written out as the process converts it, on a
/// response that stays open until the process ends: header first, then a second at a time.
/// Declared `text/vtt`, though it is ASS, as the reference declares it.
async fn subtitles(
    _caller: Caller,
    State(state): State<PlexState>,
    Query(params): Query<PlaybackParams>,
) -> Result<Response, ApiError> {
    let session = params.session.as_deref().ok_or(ApiError::BadRequest)?;
    // Answered at once, the body waiting for the stream to start, as the reference does:
    // Plex Web asks for the subtitles before it starts the stream they belong to.
    let feed = Feed::Waiting(Arc::clone(&state.transcoder), gql::session_key(session));
    let pieces = futures_util::stream::unfold(feed, |feed| async move {
        let mut reader = match feed {
            Feed::Waiting(transcoder, key) => transcoder.subtitles(&key).await.ok()?,
            Feed::Reading(reader) => reader,
        };
        let piece = reader.next_piece().await?;
        Some((
            Ok::<_, std::convert::Infallible>(piece),
            Feed::Reading(reader),
        ))
    });
    Ok((
        [(header::CONTENT_TYPE, "text/vtt")],
        axum::body::Body::from_stream(pieces),
    )
        .into_response())
}

/// Where a subtitle response stands: waiting for its stream, then reading it.
enum Feed {
    Waiting(Arc<klapz_transcoder::Transcoder>, String),
    Reading(klapz_transcoder::session::SubtitleReader),
}

/// The reference answers `200` to a session it knows and `404` otherwise, for both.
#[derive(Debug, Deserialize)]
struct SessionParams {
    session: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TouchedData {
    touch_playback_session: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EndedData {
    end_playback_session: bool,
}

fn key_variable(session: String) -> Variables {
    let mut variables = Variables::default();
    variables.insert(Name::new("key"), Value::String(session));
    variables
}

/// A sign of life for the session and its conversion, on the official API.
async fn ping(
    _caller: Caller,
    State(state): State<PlexState>,
    Query(params): Query<SessionParams>,
) -> Result<StatusCode, ApiError> {
    let session = params.session.ok_or(ApiError::NotFound)?;
    let data: TouchedData =
        gql::execute(&state.schema, TOUCH_SESSION, key_variable(session)).await?;
    data.touch_playback_session
        .then_some(StatusCode::OK)
        .ok_or(ApiError::NotFound)
}

/// Ends the conversion and its session together, on the official API. The reference keeps the
/// session listed until the player's timeline stops; ours goes at once.
async fn stop(
    _caller: Caller,
    State(state): State<PlexState>,
    Query(params): Query<SessionParams>,
) -> Result<StatusCode, ApiError> {
    let session = params.session.ok_or(ApiError::NotFound)?;
    let data: EndedData = gql::execute(&state.schema, END_SESSION, key_variable(session)).await?;
    data.end_playback_session
        .then_some(StatusCode::OK)
        .ok_or(ApiError::NotFound)
}

const PLAYING: &str = r"
query PlexPlaying {
  playbackSessions(first: 500) { edges { node { id key } } }
}
";

const FALL_BACK: &str = r"
mutation PlexFallBack($input: FallBackStreamInput!) {
  fallBackStream(input: $input) { userErrors { code } }
}
";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlayingData {
    playback_sessions: PlayingSessions,
}

#[derive(Debug, Deserialize)]
struct PlayingSessions {
    edges: Vec<PlayingEdge>,
}

#[derive(Debug, Deserialize)]
struct PlayingEdge {
    node: PlayingSession,
}

#[derive(Debug, Deserialize)]
struct PlayingSession {
    id: String,
    key: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FallBackData {
    fall_back_stream: FallBackPayload,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FallBackPayload {
    user_errors: Vec<GqlStreamError>,
}

/// `POST /{transcodeType}/:/transcode/universal/fallback`: the official API's `fallBackStream`
/// for the conversion `session` names, whatever kind the path says, as on the reference.
/// `session`, not the spec's `transcodeSessionId`: no `session` is `400`, an unknown one `404`,
/// one that cannot fall back `412`.
async fn fall_back(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(kind): Path<String>,
    Query(params): Query<SessionParams>,
) -> Result<StatusCode, ApiError> {
    if !matches!(kind.as_str(), "video" | "music" | "audio" | "subtitles") {
        return Err(ApiError::NotFound);
    }
    let key = params.session.ok_or(ApiError::BadRequest)?;
    let playing: PlayingData = gql::execute(&state.schema, PLAYING, Variables::default()).await?;
    let session = playing
        .playback_sessions
        .edges
        .into_iter()
        .map(|edge| edge.node)
        .find(|session| session.key == key)
        .ok_or(ApiError::NotFound)?;
    let variables = Variables::from_json(json!({ "input": { "session": session.id } }));
    let data: FallBackData = gql::execute(&state.schema, FALL_BACK, variables).await?;
    match data.fall_back_stream.user_errors.first() {
        None => Ok(StatusCode::OK),
        Some(error) if error.code == "NO_FALLBACK" => Err(ApiError::PreconditionFailed),
        Some(_) => Err(ApiError::NotFound),
    }
}

#[cfg(test)]
mod tests {
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
            subtitles: parsed.get("subtitles").cloned(),
            subtitle_size: parsed
                .get("subtitleSize")
                .and_then(|size| size.parse().ok()),
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
}
