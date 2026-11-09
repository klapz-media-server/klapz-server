//! `/:/timeline`: a player saying where it stands, every few seconds and at every change.
//!
//! Declared `POST` by the spec; Plex Web sends `GET`, and the reference answers `PUT` too.

use async_graphql::{Name, Value, Variables};
use axum::extract::{Query, State};
use axum::routing::get;
use axum::Router;
use open_plex_api::item::ITEM_TYPE;
use open_plex_api::GlobalId;
use serde::Deserialize;
use serde_json::{json, Value as Json};

use crate::auth::Caller;
use crate::client::ClientIdentity;
use crate::error::ApiError;
use crate::gql;
use crate::playback::resolution_label;
use crate::response::{PlexResponse, ResponseFormat};
use crate::PlexState;

const ITEM_EXISTS: &str = r"
query PlexTimelineItem($id: ID!) { node(id: $id) { id } }
";
const REPORT: &str = r"
mutation PlexReportPlayback($input: ReportPlaybackInput!) { reportPlayback(input: $input) }
";

const SESSIONS: &str = r"
query PlexTimelineSessions {
  playbackSessions(first: 500) {
    edges {
      node { key playbackId videoDecision audioDecision videoBitrate audioBitrate width }
    }
  }
}
";

pub fn add_routes(router: Router<PlexState>) -> Router<PlexState> {
    router.route("/:/timeline", get(timeline).post(timeline).put(timeline))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TimelineParams {
    state: Option<String>,
    time: Option<String>,
    rating_key: Option<String>,
}

fn reported_state(state: Option<&str>) -> Result<&'static str, ApiError> {
    match state {
        Some("playing") => Ok("PLAYING"),
        Some("paused") => Ok("PAUSED"),
        Some("buffering") => Ok("BUFFERING"),
        Some("stopped") => Ok("STOPPED"),
        _ => Err(ApiError::BadRequest),
    }
}

#[derive(Debug, Deserialize)]
struct NodeData {
    node: Option<serde::de::IgnoredAny>,
}

async fn require_item(state: &PlexState, key: i64) -> Result<(), ApiError> {
    let mut variables = Variables::default();
    variables.insert(
        Name::new("id"),
        Value::String(GlobalId::encode(ITEM_TYPE, key)),
    );
    let data: NodeData = gql::execute(&state.schema, ITEM_EXISTS, variables).await?;
    data.node.map(|_| ()).ok_or(ApiError::NotFound)
}

/// Failing to record a report is no reason to disturb a film: the next one comes in seconds.
async fn report(state: &PlexState, playback_id: String, reported: &str, position: Option<i64>) {
    let fields = [
        (Name::new("playbackId"), Value::String(playback_id)),
        (Name::new("state"), Value::Enum(Name::new(reported))),
        (
            Name::new("positionMs"),
            position.map_or(Value::Null, |position| Value::Number(position.into())),
        ),
    ];
    let mut variables = Variables::default();
    variables.insert(
        Name::new("input"),
        Value::Object(fields.into_iter().collect()),
    );
    let recorded: Result<serde::de::IgnoredAny, ApiError> =
        gql::execute(&state.schema, REPORT, variables).await;
    if let Err(error) = recorded {
        tracing::warn!(%error, "could not record a playback report");
    }
}

/// The checks run in the order the reference applies them: a bad `state` or `time` is refused
/// even for an item that does not exist, and a `ratingKey` that is not a number is ignored
/// rather than refused.
///
/// `playbackState` says what became of the item's watch state, `progress` or `complete` on the
/// reference, and Plex Web moves its own copy of the position on those. Nothing here stores a
/// watch state yet, so `ignore` is the true answer even when the session was updated.
async fn timeline(
    _caller: Caller,
    State(state): State<PlexState>,
    client: ClientIdentity,
    format: ResponseFormat,
    Query(params): Query<TimelineParams>,
) -> Result<PlexResponse<Json>, ApiError> {
    let reported = reported_state(params.state.as_deref())?;
    let position = params
        .time
        .as_deref()
        .map(str::parse::<i64>)
        .transpose()
        .map_err(|_| ApiError::BadRequest)?;

    let mut body = json!({ "size": 0, "playbackState": "ignore" });
    let item = params
        .rating_key
        .as_deref()
        .and_then(|key| key.parse().ok());
    if let Some(item) = item {
        require_item(&state, item).await?;
        if let Some(playback_id) = client.session_identifier {
            report(&state, playback_id.clone(), reported, position).await;
            if let Some(session) = latest_session(&state, &playback_id).await {
                describe_session(&mut body, &session);
            }
        }
    }

    Ok(PlexResponse::new(body, format))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionsData {
    playback_sessions: GqlSessions,
}

#[derive(Debug, Deserialize)]
struct GqlSessions {
    edges: Vec<GqlSessionEdge>,
}

#[derive(Debug, Deserialize)]
struct GqlSessionEdge {
    node: GqlSession,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlSession {
    key: String,
    playback_id: Option<String>,
    video_decision: String,
    audio_decision: String,
    video_bitrate: Option<i32>,
    audio_bitrate: Option<i32>,
    width: Option<i32>,
}

/// A change of quality leaves the previous conversion's row behind until it expires, and rows
/// come oldest first. This only decorates the answer: failing to read it costs a report nothing.
async fn latest_session(state: &PlexState, playback_id: &str) -> Option<GqlSession> {
    let data: Result<SessionsData, ApiError> =
        gql::execute(&state.schema, SESSIONS, Variables::default()).await;
    let data = data
        .map_err(|error| tracing::warn!(%error, "could not read the playback sessions"))
        .ok()?;
    data.playback_sessions
        .edges
        .into_iter()
        .rev()
        .map(|edge| edge.node)
        .find(|session| session.playback_id.as_deref() == Some(playback_id))
}

/// A copied track weighs 2.1 times its bitrate and an encoded one 1.05, rounded up over their
/// sum: every `bandwidth` measured on the reference fits this.
fn bandwidth_kbps(session: &GqlSession) -> i64 {
    let weighted = |bits: Option<i32>, decision: &str| {
        let kbps = i64::from(bits.unwrap_or(0)) / 1000;
        kbps * if decision == "COPY" { 210 } else { 105 }
    };
    let total = weighted(session.video_bitrate, &session.video_decision)
        + weighted(session.audio_bitrate, &session.audio_decision);
    (total + 99) / 100
}

/// What Plex Web reads to show the current quality. It compares `TranscodeSession.key` with the
/// session it started, so the key has to be that session's. Quality never changes during a
/// session here, so one entry covers it from the start.
fn describe_session(body: &mut Json, session: &GqlSession) {
    let resolution = resolution_label(session.width.map_or(0, i32::unsigned_abs));
    body["size"] = json!(2);
    body["Bandwidths"] = json!([{
        "Bandwidth": [{
            "time": 0,
            "bandwidth": bandwidth_kbps(session),
            "resolution": resolution,
        }],
    }]);
    body["TranscodeSession"] = json!([{
        "key": format!("/transcode/sessions/{}", session.key),
        "throttled": false,
        "complete": false,
        "context": "streaming",
        "videoDecision": session.video_decision.to_ascii_lowercase(),
        "audioDecision": session.audio_decision.to_ascii_lowercase(),
    }]);
}
