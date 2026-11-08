//! Playback: a file served as it is, or remuxed over HLS.
//!
//! The binding over `open-plex-transcoder`, as `photo` is. It covers Direct Play and Direct
//! Stream, where the video is copied. A file whose video would have to be converted answers
//! `501`, like a DASH request.
//!
//! `/video/:/transcode/session/…` is the process's own way back in. Those routes are served
//! without a token and answer only to the unguessable id its session was given.

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
use open_plex_transcoder::plan::{plan_direct_stream, AudioAction, Track, AUDIO_BITRATE_KBPS};
use open_plex_transcoder::session::RemuxRequest;
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
        .route("/video/:/transcode/universal/start.mpd", get(refuse_dash))
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
    subtitles: Option<String>,
    offset: Option<f64>,
    #[serde(rename = "audioStreamID")]
    audio_stream_id: Option<i64>,
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

    fn check_supported(&self) -> Result<(), ApiError> {
        if self
            .protocol
            .as_deref()
            .is_some_and(|protocol| protocol != "hls")
        {
            return Err(ApiError::NotImplemented);
        }
        // Burning a subtitle in means drawing on the picture, so the video cannot be copied.
        if self.subtitles.as_deref() == Some("burn") {
            return Err(ApiError::NotImplemented);
        }
        Ok(())
    }

    /// `-1` is how a client says "the default".
    fn index_of(value: Option<i64>) -> usize {
        usize::try_from(value.unwrap_or(0)).unwrap_or(0)
    }
}

async fn item(state: &PlexState, key: i64) -> Result<GqlItem, ApiError> {
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

    fn audio_action(&self) -> Result<AudioAction, ApiError> {
        let codec = |stream: &GqlStream| stream.codec.clone().unwrap_or_default();
        Ok(plan_direct_stream(&codec(self.video), &codec(self.audio))?.audio)
    }
}

fn track(stream: &GqlStream) -> Track {
    Track {
        index: stream.position,
        codec: stream.codec.clone().unwrap_or_default(),
        language: stream.language.clone(),
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
    params.check_supported()?;
    let item = item(&state, params.rating_key()?).await?;
    let selection = Selection::of(&item, &params)?;
    let library = item.library.as_ref().ok_or(ApiError::BadRequest)?;
    let direct_play = params.wants_direct_play();
    let action = if direct_play {
        None
    } else {
        Some(selection.audio_action()?)
    };

    let entry = MetadataEntry::from_graphql(&item, true, Language::of(&client))
        .ok_or(ApiError::BadRequest)?;
    let mut metadata =
        serde_json::to_value(entry).map_err(|error| ApiError::Internal(Box::new(error)))?;
    mark_decision(&mut metadata, &selection, action);

    let mut body = json!({
        "size": 1,
        "allowSync": "1",
    });
    let fields = body.as_object_mut().ok_or(ApiError::BadRequest)?;
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
fn mark_decision(metadata: &mut Json, selection: &Selection<'_>, action: Option<AudioAction>) {
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
        if let Some(action) = action {
            version.insert("container".into(), json!("mpegts"));
            version.insert("protocol".into(), json!("hls"));
            if action == AudioAction::ConvertToAac {
                version.insert("audioCodec".into(), json!("aac"));
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

fn mark_part(part: &mut Json, selection: &Selection<'_>, action: Option<AudioAction>) {
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
    if action.is_some() {
        fields.insert("container".into(), json!("mpegts"));
        fields.insert("protocol".into(), json!("hls"));
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
        if id != video {
            fields.insert("selected".into(), json!(true));
        }
        let Some(action) = action else {
            fields.insert("location".into(), json!("direct"));
            continue;
        };
        if is_subtitle {
            fields.insert("decision".into(), json!("ignore"));
            fields.insert("location".into(), json!("sidecar-subs"));
        } else {
            let converted = id == audio && action == AudioAction::ConvertToAac;
            if converted {
                fields.insert("codec".into(), json!("aac"));
            }
            fields.insert(
                "decision".into(),
                json!(if converted { "transcode" } else { "copy" }),
            );
            fields.insert("location".into(), json!("segments-av"));
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
    audio_action: AudioAction,
    client: &ClientIdentity,
    offset: Option<f64>,
) -> Result<(), ApiError> {
    let optional = |value: Option<String>| value.map_or(Value::Null, Value::String);
    let fields = [
        ("key", Value::String(session.to_owned())),
        ("itemId", Value::String(item.id.clone())),
        ("fileId", Value::String(selection.part.id.clone())),
        (
            "clientIdentifier",
            optional(client.identifier.as_ref().map(|id| id.as_str().to_owned())),
        ),
        ("clientProduct", optional(client.product.clone())),
        ("videoDecision", Value::Enum(Name::new("COPY"))),
        (
            "audioDecision",
            Value::Enum(Name::new(match audio_action {
                AudioAction::Copy => "COPY",
                AudioAction::ConvertToAac => "TRANSCODE",
            })),
        ),
        ("positionMs", Value::Number(position_ms(offset).into())),
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

/// `GET /video/:/transcode/universal/start.m3u8`: starts the remux and answers the master
/// playlist, one variant as on the reference.
async fn start_hls(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    Query(params): Query<PlaybackParams>,
) -> Result<Response, ApiError> {
    params.check_supported()?;
    let session = params.session.clone().ok_or(ApiError::BadRequest)?;
    let item = item(&state, params.rating_key()?).await?;
    let selection = Selection::of(&item, &params)?;
    let audio_action = selection.audio_action()?;
    let source = file_of_part(&state, selection.part.database_id()).await?;

    state
        .transcoder
        .start_remux(RemuxRequest {
            session: session.clone(),
            source,
            video: track(selection.video),
            audio: track(selection.audio),
            audio_action,
            offset: params.offset,
        })
        .await?;
    if let Err(error) = record_start(
        &state,
        &session,
        &item,
        &selection,
        audio_action,
        &client,
        params.offset,
    )
    .await
    {
        // A session nobody can see or stop must not keep running.
        state.transcoder.stop_session(&session);
        return Err(error);
    }

    let audio_bits = match audio_action {
        AudioAction::Copy => 0,
        AudioAction::ConvertToAac => i64::from(AUDIO_BITRATE_KBPS) * 1000,
    };
    let bandwidth = i64::from(selection.version.bitrate.unwrap_or(0)) + audio_bits;
    let resolution = match (selection.version.width, selection.version.height) {
        (Some(width), Some(height)) => format!(",RESOLUTION={width}x{height}"),
        _ => String::new(),
    };
    let frame_rate = selection
        .version
        .frame_rate
        .map_or_else(String::new, |rate| format!(",FRAME-RATE={rate:.6}"));
    Ok(playlist(format!(
        "#EXTM3U\n#EXT-X-STREAM-INF:PROGRAM-ID=1,BANDWIDTH={bandwidth}{resolution}{frame_rate}\n\
         session/{session}/base/index.m3u8\n"
    )))
}

async fn refuse_dash(_caller: Caller) -> ApiError {
    ApiError::NotImplemented
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
    _caller: Caller,
    State(state): State<PlexState>,
    Path((session, _base, file)): Path<(String, String, String)>,
    request: Request,
) -> Result<Response, ApiError> {
    touch_session(&state, &session).await;
    if file == "index.m3u8" {
        let output = state.transcoder.session_output(&session).await?;
        return Ok(playlist(media_playlist(&output)));
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
    if !state.transcoder.ping_session(&session) {
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
    if !state.transcoder.stop_session(&session) {
        return StatusCode::NOT_FOUND;
    }
    let mut variables = Variables::default();
    variables.insert(Name::new("key"), Value::String(session));
    let ended: Result<serde::de::IgnoredAny, ApiError> =
        gql::execute(&state.schema, END_SESSION, variables).await;
    if let Err(error) = ended {
        // It expires by itself.
        tracing::warn!(%error, "could not close a playback session");
    }
    StatusCode::OK
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
            subtitles: parsed.get("subtitles").cloned(),
            offset: None,
            audio_stream_id: None,
        }
    }

    #[test]
    fn direct_play_needs_the_client_to_vouch_for_the_file() {
        assert!(params("directPlay=1&hasMDE=1").wants_direct_play());
        assert!(!params("directPlay=0&hasMDE=1").wants_direct_play());
        assert!(!params("directPlay=1").wants_direct_play());
    }

    #[test]
    fn refuses_what_needs_a_video_transcode_or_dash() {
        assert!(params("protocol=hls").check_supported().is_ok());
        assert!(params("subtitles=none").check_supported().is_ok());
        assert!(matches!(
            params("protocol=dash").check_supported(),
            Err(ApiError::NotImplemented)
        ));
        assert!(matches!(
            params("subtitles=burn").check_supported(),
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
