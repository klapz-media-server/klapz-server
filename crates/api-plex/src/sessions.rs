//! `GET /status/sessions`: what is playing on the server right now, and the operator ending a
//! session of it.
//!
//! Plex Web polls it to draw the activity indicator and its dashboard. Sessions come from the
//! official API, and expire there. The attributes are the ones Plex Web reads.

use std::sync::LazyLock;

use async_graphql::{Name, Value, Variables};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value as Json};

use crate::auth::AdminCaller;
use crate::error::ApiError;
use crate::gql;
use crate::gql_items::{with_shared_fragments, GqlItem};
use crate::items::MetadataEntry;
use crate::language::Language;
use crate::response::{PlexResponse, ResponseFormat};
use crate::PlexState;

pub fn router() -> Router<PlexState> {
    Router::new()
        .route("/status/sessions", get(sessions))
        .route("/status/sessions/background", get(background))
        // Declared `POST` by the spec; Plex Web sends `GET`.
        .route("/status/sessions/terminate", get(terminate).post(terminate))
}

static SESSIONS_QUERY: LazyLock<String> = LazyLock::new(|| {
    with_shared_fragments(
        r"
query PlexSessions {
  playbackSessions(first: 500) {
    edges {
      node {
        id
        key
        playbackId
        state
        videoDecision
        audioDecision
        clientIdentifier
        clientProduct
        positionMs
        item { ...ListedItem }
      }
    }
  }
}
",
    )
});

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
    id: String,
    key: String,
    playback_id: Option<String>,
    state: String,
    video_decision: String,
    audio_decision: String,
    client_identifier: Option<String>,
    client_product: Option<String>,
    position_ms: i64,
    item: Option<GqlItem>,
}

/// `Session.id`, which `terminate` names: the player's identifier for the playback, as on the
/// reference, or the conversion's key for a player that gave none.
fn plex_session_id(key: &str, playback_id: Option<&str>) -> String {
    playback_id.unwrap_or(key).to_owned()
}

fn entry(session: &GqlSession, item: &GqlItem) -> Option<Json> {
    let metadata = MetadataEntry::from_graphql(item, true, Language::default())?;
    let mut metadata = serde_json::to_value(metadata).ok()?;
    let fields = metadata.as_object_mut()?;
    // A number of the server's on the reference, as a string.
    let session_key = open_plex_api::GlobalId::decode(&session.id).ok()?.key;
    fields.insert("sessionKey".into(), json!(session_key.to_string()));
    fields.insert("viewOffset".into(), json!(session.position_ms));
    fields.insert(
        "Session".into(),
        json!({
            "id": plex_session_id(&session.key, session.playback_id.as_deref()),
            "location": "lan",
        }),
    );
    fields.insert(
        "Player".into(),
        json!({
            "machineIdentifier": session.client_identifier,
            "product": session.client_product,
            "state": session.state.to_ascii_lowercase(),
            "local": true,
        }),
    );
    fields.insert(
        "TranscodeSession".into(),
        json!({
            "key": format!("/transcode/sessions/{}", session.key),
            "protocol": "hls",
            "container": "mpegts",
            "videoDecision": session.video_decision.to_ascii_lowercase(),
            "audioDecision": session.audio_decision.to_ascii_lowercase(),
        }),
    );
    Some(metadata)
}

async fn sessions(
    _caller: AdminCaller,
    State(state): State<PlexState>,
    format: ResponseFormat,
) -> Result<PlexResponse<Json>, ApiError> {
    let data: SessionsData =
        gql::execute(&state.schema, &SESSIONS_QUERY, Variables::default()).await?;
    let items: Vec<Json> = data
        .playback_sessions
        .edges
        .iter()
        .filter_map(|edge| entry(&edge.node, edge.node.item.as_ref()?))
        .collect();

    let mut body = json!({ "size": items.len() });
    if !items.is_empty() {
        body["Metadata"] = Json::Array(items);
    }
    Ok(PlexResponse::new(body, format))
}

/// `GET /status/sessions/background`: the optimizer's and downloads' conversions, which this
/// server does not run. The reference with none running answers the same empty container.
async fn background(_caller: AdminCaller, format: ResponseFormat) -> PlexResponse<Json> {
    PlexResponse::new(json!({ "size": 0 }), format)
}

const PLAYING: &str = r"
query PlexPlaying {
  playbackSessions(first: 500) { edges { node { id key playbackId } } }
}
";

const TERMINATE: &str = r"
mutation PlexTerminate($input: TerminatePlaybackSessionInput!) {
  terminatePlaybackSession(input: $input) { userErrors { code } }
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
#[serde(rename_all = "camelCase")]
struct PlayingSession {
    id: String,
    key: String,
    playback_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TerminateData {
    terminate_playback_session: TerminatePayload,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TerminatePayload {
    user_errors: Vec<serde::de::IgnoredAny>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TerminateParams {
    session_id: Option<String>,
    reason: Option<String>,
}

/// `/status/sessions/terminate`: ends every session listed under `Session.id`, on the official
/// API. The player hears the reason at its next timeline report.
///
/// No `sessionId` is `400`, as on the reference; an empty one `403` and an unknown one `404`, as
/// the spec says. The reference refuses everything else `401` without a subscription.
async fn terminate(
    _caller: AdminCaller,
    State(state): State<PlexState>,
    Query(params): Query<TerminateParams>,
) -> Result<StatusCode, ApiError> {
    let session_id = params.session_id.ok_or(ApiError::BadRequest)?;
    if session_id.is_empty() {
        return Err(ApiError::Forbidden);
    }
    let playing: PlayingData = gql::execute(&state.schema, PLAYING, Variables::default()).await?;
    let mut terminated = false;
    for session in playing
        .playback_sessions
        .edges
        .into_iter()
        .map(|edge| edge.node)
    {
        if plex_session_id(&session.key, session.playback_id.as_deref()) != session_id {
            continue;
        }
        let input = [
            (Name::new("session"), Value::String(session.id)),
            (
                Name::new("reason"),
                params.reason.clone().map_or(Value::Null, Value::String),
            ),
        ];
        let mut variables = Variables::default();
        variables.insert(
            Name::new("input"),
            Value::Object(input.into_iter().collect()),
        );
        let data: TerminateData = gql::execute(&state.schema, TERMINATE, variables).await?;
        terminated |= data.terminate_playback_session.user_errors.is_empty();
    }
    terminated
        .then_some(StatusCode::OK)
        .ok_or(ApiError::NotFound)
}
