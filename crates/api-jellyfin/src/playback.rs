//! Playing: what a client may play and how, the stream itself, and what it reports back.
//!
//! `PlaybackInfo` tells a client, per media source, whether it plays the file as it is — the
//! official API's `MediaFile.readableAsIs` decides, from the formats of the client's device
//! profile — and otherwise where to read a conversion. The stream addresses it hands out are
//! the binding's own, `/videos/{id}/stream` and `/videos/{id}/master.m3u8`; reached, they start
//! the stream through `startStream` and send the player to the stream's own address, as Plex's
//! `start.m3u8` does. Reports of progress go to the official API's viewing and session
//! mutations, keyed by the play session the client names.

use axum::body::Bytes;
use axum::extract::{Path, RawQuery, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::dto::{media_source, source_id, Item, ITEM_FIELDS};
use crate::error::JellyfinError;
use crate::gql;
use crate::id;
use crate::query::Query;
use crate::response::{json, no_content};
use crate::time;
use crate::track::Track;
use crate::JellyfinState;

pub fn router() -> Router<JellyfinState> {
    Router::new()
        .route(
            "/items/{id}/playbackinfo",
            get(playback_info).post(playback_info),
        )
        .route("/videos/{id}/{name}", get(video))
        .route("/sessions/playing", post(playing))
        .route("/sessions/playing/progress", post(progress))
        .route("/sessions/playing/ping", post(ping))
        .route("/sessions/playing/stopped", post(stopped))
        .route(
            "/userplayeditems/{id}",
            post(mark_played).delete(mark_unplayed),
        )
        .route(
            "/users/{user}/playeditems/{id}",
            post(legacy_mark_played).delete(legacy_mark_unplayed),
        )
        .route(
            "/userfavoriteitems/{id}",
            post(no_favorites).delete(no_favorites),
        )
        .route(
            "/users/{user}/favoriteitems/{id}",
            post(no_favorites).delete(no_favorites),
        )
}

/// What a client sends with `PlaybackInfo`. Every field may also come in the query string.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
struct PlaybackInfoRequest {
    media_source_id: Option<String>,
    #[serde(deserialize_with = "lenient::number")]
    audio_stream_index: Option<i32>,
    /// `-1` for no subtitle.
    #[serde(deserialize_with = "lenient::number")]
    subtitle_stream_index: Option<i32>,
    #[serde(deserialize_with = "lenient::number")]
    max_streaming_bitrate: Option<i64>,
    #[serde(deserialize_with = "lenient::number")]
    start_time_ticks: Option<i64>,
    enable_direct_play: Option<bool>,
    device_profile: Option<DeviceProfile>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
struct DeviceProfile {
    #[serde(deserialize_with = "lenient::number")]
    max_streaming_bitrate: Option<i64>,
    direct_play_profiles: Vec<DirectPlayProfile>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
struct DirectPlayProfile {
    container: Option<String>,
    video_codec: Option<String>,
    audio_codec: Option<String>,
    #[serde(rename = "Type")]
    kind: Option<String>,
}

/// Codec names in Jellyfin's vocabulary that our tracks keep under another. Plex's importer
/// stores DTS as `dca`, which a profile names `dts`.
fn our_codec(name: &str) -> String {
    match name.trim().to_ascii_lowercase().as_str() {
        "dts" => "dca".to_owned(),
        other => other.to_owned(),
    }
}

/// A comma-separated list of a profile, empty for any.
fn names(list: Option<&str>) -> Vec<String> {
    list.map(|list| {
        list.split(',')
            .map(our_codec)
            .filter(|name| !name.is_empty())
            .collect()
    })
    .unwrap_or_default()
}

/// The device's video formats, as the official API takes them.
fn player_formats(profile: &DeviceProfile) -> Vec<Value> {
    profile
        .direct_play_profiles
        .iter()
        .filter(|format| {
            format
                .kind
                .as_deref()
                .is_none_or(|kind| kind.eq_ignore_ascii_case("video"))
                // Entries for sources that already are HLS: no file of ours is.
                && !format
                    .container
                    .as_deref()
                    .is_some_and(|container| container.eq_ignore_ascii_case("hls"))
        })
        .map(|format| {
            json!({
                "containers": names(format.container.as_deref()),
                "videoCodecs": names(format.video_codec.as_deref()),
                "audioCodecs": names(format.audio_codec.as_deref()),
            })
        })
        .collect()
}

/// The name a file played as it is goes by: the first of its container's names the device
/// profile lists, as the reference names a Matroska file of `WebM`'s codecs `webm` to a browser.
fn played_container(file: &crate::dto::File, profile: &DeviceProfile) -> Option<String> {
    let names = file.format_names()?;
    names
        .split(',')
        .find(|name| {
            profile.direct_play_profiles.iter().any(|format| {
                format.container.as_deref().is_some_and(|containers| {
                    containers
                        .split(',')
                        .any(|listed| listed.trim().eq_ignore_ascii_case(name))
                })
            })
        })
        .map(ToOwned::to_owned)
}

fn item_query() -> String {
    format!(
        r"query($id: ID!, $people: Boolean!,
                $chapters: Boolean!) {{ node(id: $id) {{ ...JellyfinItem }} }}
        {ITEM_FIELDS}"
    )
}

const READABLE: &str = r"query($file: ID!, $formats: [PlayerFormatInput!]!, $max: Int,
                               $audio: ID) {
    node(id: $file) { ... on MediaFile {
        readableAsIs(formats: $formats, maxBitrate: $max, audioTrack: $audio)
    } }
}";

#[derive(Deserialize)]
struct NodeData<T> {
    node: Option<T>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Readable {
    readable_as_is: bool,
}

async fn playable_item(state: &JellyfinState, item: &str) -> Result<Item, JellyfinError> {
    let global = id::global_of_type(item, open_plex_api::item::ITEM_TYPE)?;
    let data: NodeData<Item> = gql::query(
        &state.schema,
        &item_query(),
        json!({ "id": global, "people": false, "chapters": false }),
    )
    .await?;
    data.node.ok_or(JellyfinError::NotFound)
}

/// The track of a file Jellyfin names by its index.
fn track_at<'a>(tracks: &'a [Track], index: Option<i32>, kind: &str) -> Option<&'a Track> {
    let index = index?;
    tracks
        .iter()
        .find(|track| track.position == index && track.kind == kind)
}

/// The play session a `PlaybackInfo` opens: what the client reports progress and stops under.
fn new_play_session() -> String {
    let mut token = open_plex_core::access::new_token();
    token.truncate(32);
    token
}

impl PlaybackInfoRequest {
    /// The request's body, completed by its query string.
    fn read(body: &[u8], query: &Query) -> Result<Self, JellyfinError> {
        let mut request: Self = if body.is_empty() {
            Self::default()
        } else {
            serde_json::from_slice(body)
                .map_err(|_| JellyfinError::BadRequest("a playback request is not JSON"))?
        };
        let number = |name: &str| query.get(name).and_then(|value| value.parse::<i64>().ok());
        let index = |name: &str| number(name).and_then(|value| i32::try_from(value).ok());
        request.media_source_id = request
            .media_source_id
            .or_else(|| query.get("mediaSourceId").map(str::to_owned));
        request.audio_stream_index = request
            .audio_stream_index
            .or_else(|| index("audioStreamIndex"));
        request.subtitle_stream_index = request
            .subtitle_stream_index
            .or_else(|| index("subtitleStreamIndex"));
        request.max_streaming_bitrate = request
            .max_streaming_bitrate
            .or_else(|| number("maxStreamingBitrate"));
        request.start_time_ticks = request
            .start_time_ticks
            .or_else(|| number("startTimeTicks"));
        Ok(request)
    }
}

async fn playback_info(
    State(state): State<JellyfinState>,
    Path(item): Path<String>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
    body: Bytes,
) -> Result<Response, JellyfinError> {
    let query = Query::parse(raw_query.as_deref());
    gql::viewer(&state.schema)
        .await?
        .require_self(query.get("userId"))?;
    let mut request = PlaybackInfoRequest::read(&body, &query)?;

    let item = playable_item(&state, &item).await?;
    let profile = request.device_profile.take().unwrap_or_default();
    let formats = player_formats(&profile);
    let max_bitrate = request
        .max_streaming_bitrate
        .or(profile.max_streaming_bitrate);
    let direct_play_allowed = request.enable_direct_play.unwrap_or(true);
    let token = crate::auth::token(&headers, raw_query.as_deref()).unwrap_or_default();
    let device = crate::auth::ClientInfo::of(&headers)
        .device_id
        .unwrap_or_default();
    let play_session = new_play_session();
    let item_id = id::of_global(&item.id).unwrap_or_default();

    let mut sources = Vec::new();
    for (index, version) in item.versions.iter().enumerate() {
        let (Some(file), Some(source_id)) = (version.files.first(), source_id(&item, index)) else {
            continue;
        };
        if request
            .media_source_id
            .as_deref()
            .is_some_and(|wanted| !wanted.eq_ignore_ascii_case(&source_id))
        {
            continue;
        }
        let audio = track_at(&file.streams, request.audio_stream_index, "AUDIO");
        let readable: NodeData<Readable> = gql::query(
            &state.schema,
            READABLE,
            json!({
                "file": file.id,
                "formats": formats,
                "max": max_bitrate,
                "audio": audio.map(|track| &track.id),
            }),
        )
        .await?;
        let direct_play = direct_play_allowed
            && readable
                .node
                .is_some_and(|readable| readable.readable_as_is);
        let Some(mut source) = media_source(version, &source_id, item.duration_ms) else {
            continue;
        };
        source["SupportsDirectPlay"] = json!(direct_play);
        if direct_play {
            if let Some(container) = played_container(file, &profile) {
                source["Container"] = json!(container);
            }
        }
        // A client that cannot open the file's path — any browser — reads a file it plays as
        // it is from `/videos/{id}/stream?static=true`, which Jellyfin calls direct
        // streaming. A copy of the tracks into another container is one of our conversions,
        // offered at the transcoding address.
        source["SupportsDirectStream"] = json!(direct_play);
        if !direct_play {
            source["TranscodingUrl"] = json!(transcoding_url(&TranscodingRequest {
                item: &item_id,
                source: &source_id,
                play_session: &play_session,
                device: &device,
                audio: request.audio_stream_index,
                subtitle: request.subtitle_stream_index,
                max_bitrate,
                start_ticks: request.start_time_ticks,
                token: &token,
            }));
            source["TranscodingSubProtocol"] = json!("hls");
            source["TranscodingContainer"] = json!("ts");
        }
        sources.push(source);
    }
    if sources.is_empty() {
        return Err(JellyfinError::NotFound);
    }
    Ok(json(&json!({
        "MediaSources": sources,
        "PlaySessionId": play_session,
    })))
}

struct TranscodingRequest<'a> {
    item: &'a str,
    source: &'a str,
    play_session: &'a str,
    device: &'a str,
    audio: Option<i32>,
    subtitle: Option<i32>,
    max_bitrate: Option<i64>,
    start_ticks: Option<i64>,
    token: &'a str,
}

/// Where a client reads a conversion: the binding's own address, which starts it. Relative to
/// the server's base, as the reference writes it.
fn transcoding_url(request: &TranscodingRequest<'_>) -> String {
    let mut query = form_urlencoded::Serializer::new(String::new());
    query.append_pair("DeviceId", request.device);
    query.append_pair("MediaSourceId", request.source);
    query.append_pair("PlaySessionId", request.play_session);
    if let Some(audio) = request.audio {
        query.append_pair("AudioStreamIndex", &audio.to_string());
    }
    if let Some(subtitle) = request.subtitle {
        query.append_pair("SubtitleStreamIndex", &subtitle.to_string());
    }
    if let Some(bitrate) = request.max_bitrate {
        query.append_pair("VideoBitrate", &bitrate.to_string());
    }
    if let Some(ticks) = request.start_ticks.filter(|ticks| *ticks > 0) {
        query.append_pair("StartTimeTicks", &ticks.to_string());
    }
    query.append_pair("ApiKey", request.token);
    format!(
        "/videos/{}/master.m3u8?{}",
        id::dashed(request.item),
        query.finish()
    )
}

const START_STREAM: &str = r"mutation($input: StartStreamInput!) {
    startStream(input: $input) { stream { url } userErrors { code } }
}";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartData {
    start_stream: StartPayload,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartPayload {
    stream: Option<Started>,
    user_errors: Vec<StreamError>,
}

#[derive(Deserialize)]
struct Started {
    url: String,
}

#[derive(Deserialize)]
struct StreamError {
    code: String,
}

/// The file a stream request names, with its tracks: `MediaSourceId`, or the item's first.
fn file_of<'a>(
    item: &'a Item,
    source: Option<&str>,
) -> Result<&'a crate::dto::File, JellyfinError> {
    let wanted = source.map(id::decode).transpose()?;
    item.versions
        .iter()
        .enumerate()
        .filter_map(|(index, version)| Some((source_id(item, index)?, version.files.first()?)))
        .find(|(source_id, _)| {
            wanted.as_ref().is_none_or(|wanted| {
                id::decode(source_id).is_ok_and(|candidate| candidate == *wanted)
            })
        })
        .map(|(_, file)| file)
        .ok_or(JellyfinError::NotFound)
}

/// `/videos/{id}/stream[.container]?static=true` plays the file as it is;
/// `/videos/{id}/master.m3u8` converts it to HLS. Either starts its stream and sends the
/// player to the stream's own address.
async fn video(
    State(state): State<JellyfinState>,
    Path((item, name)): Path<(String, String)>,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
) -> Result<Response, JellyfinError> {
    let query = Query::parse(query.as_deref());
    let direct = name == "stream" || name.starts_with("stream.");
    if !direct && name != "master.m3u8" {
        return Err(JellyfinError::NotFound);
    }
    if direct && query.flag("static")? != Some(true) {
        return Err(JellyfinError::NotImplemented(
            "a stream converted to the container named",
        ));
    }
    let item = playable_item(&state, &item).await?;
    let file = file_of(&item, query.get("mediaSourceId"))?;
    let number = |name: &str| query.get(name).and_then(|value| value.parse::<i64>().ok());
    let audio = track_at(
        &file.streams,
        number("audioStreamIndex").and_then(|index| i32::try_from(index).ok()),
        "AUDIO",
    );
    let subtitle = track_at(
        &file.streams,
        number("subtitleStreamIndex").and_then(|index| i32::try_from(index).ok()),
        "SUBTITLE",
    );
    let burn = subtitle.is_some()
        && query
            .get("subtitleMethod")
            .is_some_and(|method| method.eq_ignore_ascii_case("encode"));
    let client = crate::auth::ClientInfo::of(&headers);
    let session = query.get("playSessionId").map(str::to_owned);
    let input = json!({
        "file": file.id,
        "directPlay": direct,
        "protocol": "HLS",
        "audioTrack": audio.map(|track| &track.id),
        "subtitleTrack": subtitle.filter(|_| burn).map(|track| &track.id),
        "subtitles": if burn { "BURN" } else { "NONE" },
        "maxVideoBitrate": number("videoBitrate")
            .map(|bits| i32::try_from(bits).unwrap_or(i32::MAX)),
        "positionMs": number("startTimeTicks").map_or(0, time::ms),
        "key": session,
        "playbackId": session,
        "clientIdentifier": query.get("deviceId").map(str::to_owned).or(client.device_id),
        "clientProduct": client.client,
    });
    let data: StartData =
        gql::query(&state.schema, START_STREAM, json!({ "input": input })).await?;
    if let Some(error) = data.start_stream.user_errors.first() {
        return Err(match error.code.as_str() {
            "FILE_NOT_FOUND" | "FILE_MISSING" | "TRACK_NOT_FOUND" => JellyfinError::NotFound,
            "VIDEO_NOT_CONVERTIBLE" | "SUBTITLE_NOT_DELIVERABLE" => {
                JellyfinError::NotImplemented("a conversion this server does not make")
            }
            _ => JellyfinError::BadRequest("the stream could not be started"),
        });
    }
    let started = data
        .start_stream
        .stream
        .ok_or(JellyfinError::BadRequest("the stream could not be started"))?;
    Ok(Redirect::temporary(&started.url).into_response())
}

/// A player's report, as Jellyfin's clients send it on starting, progressing and stopping.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
struct PlaybackReport {
    item_id: Option<String>,
    play_session_id: Option<String>,
    #[serde(deserialize_with = "lenient::number")]
    position_ticks: Option<i64>,
    is_paused: bool,
}

/// Numbers as ASP.NET binds them: a JSON number, or a string holding one. Jellyfin Web sends
/// its stream indexes as strings.
mod lenient {
    use serde::{Deserialize, Deserializer};

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Number<T> {
        Number(T),
        Text(String),
    }

    pub fn number<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
    where
        D: Deserializer<'de>,
        T: Deserialize<'de> + std::str::FromStr,
    {
        match Option::<Number<T>>::deserialize(deserializer)? {
            None => Ok(None),
            Some(Number::Number(number)) => Ok(Some(number)),
            Some(Number::Text(text)) if text.trim().is_empty() => Ok(None),
            Some(Number::Text(text)) => text
                .trim()
                .parse()
                .map(Some)
                .map_err(|_| serde::de::Error::custom(format!("`{text}` is not a number"))),
        }
    }
}

const REPORT_VIEWING: &str = r"mutation($input: ReportViewingInput!) {
    reportViewing(input: $input) { outcome }
}";

const RUNTIME: &str = r"query($id: ID!) {
    node(id: $id) { ... on Movie { durationMs } ... on Episode { durationMs } }
}";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Runtime {
    duration_ms: Option<i64>,
}

const REPORT_PLAYBACK: &str = r"mutation($input: ReportPlaybackInput!) {
    reportPlayback(input: $input)
}";

fn report_of(body: &Bytes) -> Result<PlaybackReport, JellyfinError> {
    if body.is_empty() {
        return Ok(PlaybackReport::default());
    }
    serde_json::from_slice(body).map_err(|_| JellyfinError::BadRequest("a report is not JSON"))
}

/// Records where the player stands, for the item and for its session.
async fn report(
    state: &JellyfinState,
    report: &PlaybackReport,
    playback_state: &str,
) -> Result<(), JellyfinError> {
    gql::viewer(&state.schema).await?;
    let position_ms = report.position_ticks.map(time::ms);
    if let (Some(item), Some(session), Some(position_ms)) =
        (&report.item_id, &report.play_session_id, position_ms)
    {
        let item = id::global_of_type(item, open_plex_api::item::ITEM_TYPE)?;
        // Jellyfin's clients report no runtime: the item's own is what the position is
        // measured against.
        let runtime: NodeData<Runtime> =
            gql::query(&state.schema, RUNTIME, json!({ "id": item })).await?;
        let _: Value = gql::query(
            &state.schema,
            REPORT_VIEWING,
            json!({ "input": {
                "itemId": item,
                "playbackId": session,
                "positionMs": position_ms,
                "durationMs": runtime.node.and_then(|runtime| runtime.duration_ms),
            }}),
        )
        .await?;
    }
    if let Some(session) = &report.play_session_id {
        let _: Value = gql::query(
            &state.schema,
            REPORT_PLAYBACK,
            json!({ "input": {
                "playbackId": session,
                "state": playback_state,
                "positionMs": position_ms,
            }}),
        )
        .await?;
    }
    Ok(())
}

async fn playing(
    State(state): State<JellyfinState>,
    body: Bytes,
) -> Result<Response, JellyfinError> {
    let report_body = report_of(&body)?;
    report(&state, &report_body, "PLAYING").await?;
    Ok(no_content())
}

async fn progress(
    State(state): State<JellyfinState>,
    body: Bytes,
) -> Result<Response, JellyfinError> {
    let report_body = report_of(&body)?;
    let playback_state = if report_body.is_paused {
        "PAUSED"
    } else {
        "PLAYING"
    };
    report(&state, &report_body, playback_state).await?;
    Ok(no_content())
}

async fn stopped(
    State(state): State<JellyfinState>,
    body: Bytes,
) -> Result<Response, JellyfinError> {
    let report_body = report_of(&body)?;
    report(&state, &report_body, "STOPPED").await?;
    Ok(no_content())
}

const TOUCH: &str = "mutation($key: String!) { touchPlaybackSession(key: $key) }";

/// A sign of life for a play session, which keeps its stream running.
async fn ping(
    State(state): State<JellyfinState>,
    RawQuery(query): RawQuery,
) -> Result<Response, JellyfinError> {
    gql::viewer(&state.schema).await?;
    let query = Query::parse(query.as_deref());
    if let Some(session) = query.get("playSessionId") {
        let _: Value = gql::query(&state.schema, TOUCH, json!({ "key": session })).await?;
    }
    Ok(no_content())
}

const MARK_WATCHED: &str = r"mutation($id: ID!) {
    markWatched(id: $id) { viewCount isWatched offsetMs lastViewedAt }
}";
const MARK_UNWATCHED: &str = r"mutation($id: ID!) {
    markUnwatched(id: $id) { viewCount isWatched offsetMs lastViewedAt }
}";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Marked {
    mark_watched: Option<crate::dto::WatchState>,
    mark_unwatched: Option<crate::dto::WatchState>,
}

/// Jellyfin's `UserItemDataDto` for an item's state.
fn user_item_data(state: &crate::dto::WatchState, jellyfin_id: &str) -> Value {
    let mut data = json!({
        "PlaybackPositionTicks": time::ticks(state.offset_ms.unwrap_or(0)),
        "PlayCount": state.view_count,
        "IsFavorite": false,
        "Played": state.is_watched,
        "Key": id::dashed(jellyfin_id),
        "ItemId": jellyfin_id,
    });
    if let Some(last) = &state.last_viewed_at {
        data["LastPlayedDate"] = json!(time::date(last));
    }
    data
}

async fn mark(
    state: &JellyfinState,
    user: Option<&str>,
    item: &str,
    watched: bool,
) -> Result<Response, JellyfinError> {
    gql::viewer(&state.schema).await?.require_self(user)?;
    let global = id::global_of_type(item, open_plex_api::item::ITEM_TYPE)?;
    let mutation = if watched {
        MARK_WATCHED
    } else {
        MARK_UNWATCHED
    };
    let marked: Marked = gql::query(&state.schema, mutation, json!({ "id": global }))
        .await
        .map_err(|_| JellyfinError::NotFound)?;
    let watch_state = marked
        .mark_watched
        .or(marked.mark_unwatched)
        .ok_or(JellyfinError::NotFound)?;
    Ok(json(&user_item_data(
        &watch_state,
        &id::of_global(&global).unwrap_or_default(),
    )))
}

async fn mark_played(
    State(state): State<JellyfinState>,
    Path(item): Path<String>,
    RawQuery(query): RawQuery,
) -> Result<Response, JellyfinError> {
    let query = Query::parse(query.as_deref());
    mark(&state, query.get("userId"), &item, true).await
}

async fn mark_unplayed(
    State(state): State<JellyfinState>,
    Path(item): Path<String>,
    RawQuery(query): RawQuery,
) -> Result<Response, JellyfinError> {
    let query = Query::parse(query.as_deref());
    mark(&state, query.get("userId"), &item, false).await
}

async fn legacy_mark_played(
    State(state): State<JellyfinState>,
    Path((user, item)): Path<(String, String)>,
) -> Result<Response, JellyfinError> {
    mark(&state, Some(&user), &item, true).await
}

async fn legacy_mark_unplayed(
    State(state): State<JellyfinState>,
    Path((user, item)): Path<(String, String)>,
) -> Result<Response, JellyfinError> {
    mark(&state, Some(&user), &item, false).await
}

/// Favorites are not kept here: refused visibly rather than accepted and forgotten.
async fn no_favorites() -> JellyfinError {
    JellyfinError::NotImplemented("favorites")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn a_file_played_as_it_is_goes_by_the_name_the_profile_lists() {
        let track = |kind: &str, codec: &str| {
            json!({ "id": "t", "kind": kind, "position": 0, "codec": codec, "isDefault": false,
                    "isForced": false, "isExternal": false,
                    "traits": { "isHearingImpaired": false, "isOriginal": false } })
        };
        let noob: crate::dto::File = serde_json::from_value(json!({
            "id": "f", "path": "/media/Noob.mkv", "filename": "Noob.mkv",
            "stated": { "formatName": "matroska,webm" },
            "streams": [track("VIDEO", "vp9"), track("AUDIO", "vorbis")],
        }))
        .unwrap();
        let browser: DeviceProfile = serde_json::from_value(json!({
            "DirectPlayProfiles": [
                { "Container": "webm", "Type": "Video", "VideoCodec": "vp9" },
                { "Container": "mp4,m4v", "Type": "Video" },
            ],
        }))
        .unwrap();
        assert_eq!(played_container(&noob, &browser).as_deref(), Some("webm"));
        let mkv_player: DeviceProfile = serde_json::from_value(json!({
            "DirectPlayProfiles": [{ "Container": "mkv,webm", "Type": "Video" }],
        }))
        .unwrap();
        assert_eq!(played_container(&noob, &mkv_player).as_deref(), Some("mkv"));
    }

    #[test]
    fn a_device_profile_gives_its_video_formats_in_our_codec_names() {
        let profile: DeviceProfile = serde_json::from_value(json!({
            "MaxStreamingBitrate": 120_000_000,
            "DirectPlayProfiles": [
                { "Container": "mp4,m4v", "Type": "Video", "VideoCodec": "h264,hevc", "AudioCodec": "aac,DTS" },
                { "Container": "mp3", "Type": "Audio", "AudioCodec": "mp3" },
                { "Container": "webm", "Type": "Video" },
            ],
        }))
        .unwrap();
        assert_eq!(profile.max_streaming_bitrate, Some(120_000_000));
        assert_eq!(
            player_formats(&profile),
            [
                json!({ "containers": ["mp4", "m4v"], "videoCodecs": ["h264", "hevc"], "audioCodecs": ["aac", "dca"] }),
                json!({ "containers": ["webm"], "videoCodecs": [], "audioCodecs": [] }),
            ]
        );
    }

    #[test]
    fn the_transcoding_address_carries_what_the_stream_needs() {
        let url = transcoding_url(&TranscodingRequest {
            item: "0000000400000000000000000000006f",
            source: "000000060000000000000000000007d1",
            play_session: "abc",
            device: "d 1",
            audio: Some(1),
            subtitle: None,
            max_bitrate: Some(8_000_000),
            start_ticks: Some(0),
            token: "t",
        });
        assert_eq!(
            url,
            "/videos/00000004-0000-0000-0000-00000000006f/master.m3u8?DeviceId=d+1\
             &MediaSourceId=000000060000000000000000000007d1&PlaySessionId=abc\
             &AudioStreamIndex=1&VideoBitrate=8000000&ApiKey=t"
        );
    }

    #[test]
    fn a_play_session_is_32_digits_and_new_each_time() {
        let session = new_play_session();
        assert_eq!(session.len(), 32);
        assert_ne!(session, new_play_session());
    }

    #[test]
    fn indexes_sent_as_strings_are_read_as_numbers() {
        let request: PlaybackInfoRequest = serde_json::from_value(json!({
            "AudioStreamIndex": "1",
            "SubtitleStreamIndex": "-1",
            "MaxStreamingBitrate": 2_147_483_647,
            "StartTimeTicks": "",
            "DeviceProfile": { "MaxStreamingBitrate": "120000000" },
        }))
        .unwrap();
        assert_eq!(request.audio_stream_index, Some(1));
        assert_eq!(request.subtitle_stream_index, Some(-1));
        assert_eq!(request.max_streaming_bitrate, Some(2_147_483_647));
        assert_eq!(request.start_time_ticks, None);
        assert_eq!(
            request.device_profile.unwrap().max_streaming_bitrate,
            Some(120_000_000)
        );
        assert!(serde_json::from_value::<PlaybackInfoRequest>(json!({
            "AudioStreamIndex": "one"
        }))
        .is_err());
    }

    #[test]
    fn hls_entries_of_a_profile_are_no_file_format() {
        let profile: DeviceProfile = serde_json::from_value(json!({
            "DirectPlayProfiles": [
                { "Container": "hls", "Type": "Video", "VideoCodec": "h264" },
                { "Container": "mkv", "Type": "Video" },
            ],
        }))
        .unwrap();
        assert_eq!(player_formats(&profile).len(), 1);
    }

    #[test]
    fn a_report_reads_what_clients_send() {
        let report = report_of(&Bytes::from(
            r#"{"ItemId":"x","PlaySessionId":"p","PositionTicks":600000000,"IsPaused":true,"CanSeek":true}"#,
        ))
        .unwrap();
        assert_eq!(report.item_id.as_deref(), Some("x"));
        assert_eq!(report.play_session_id.as_deref(), Some("p"));
        assert_eq!(report.position_ticks.map(time::ms), Some(60_000));
        assert!(report.is_paused);
        assert!(report_of(&Bytes::new()).is_ok());
        assert!(report_of(&Bytes::from("nope")).is_err());
    }

    #[test]
    fn user_item_data_carries_the_state_in_jellyfins_units() {
        let data = user_item_data(
            &crate::dto::WatchState {
                view_count: 2,
                is_watched: true,
                offset_ms: None,
                last_viewed_at: Some("2026-11-20T08:00:00Z".to_owned()),
            },
            "0000000400000000000000000000006f",
        );
        assert_eq!(data["PlayCount"], 2);
        assert_eq!(data["Played"], true);
        assert_eq!(data["PlaybackPositionTicks"], 0);
        assert_eq!(data["LastPlayedDate"], "2026-11-20T08:00:00.0000000Z");
    }
}
