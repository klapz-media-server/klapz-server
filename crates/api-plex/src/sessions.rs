//! `GET /status/sessions`: what is playing on the server right now.
//!
//! Plex Web polls it to draw the activity indicator and its dashboard. Sessions come from the
//! official API, and expire there. The attributes are the ones Plex Web reads; no live capture
//! exists to hold them against, since the reference cannot read its files here.

use std::sync::LazyLock;

use async_graphql::Variables;
use axum::extract::State;
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
    Router::new().route("/status/sessions", get(sessions))
}

static SESSIONS_QUERY: LazyLock<String> = LazyLock::new(|| {
    with_shared_fragments(
        r"
query PlexSessions {
  playbackSessions(first: 500) {
    edges {
      node {
        key
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
    key: String,
    state: String,
    video_decision: String,
    audio_decision: String,
    client_identifier: Option<String>,
    client_product: Option<String>,
    position_ms: i64,
    item: Option<GqlItem>,
}

fn entry(session: &GqlSession, item: &GqlItem) -> Option<Json> {
    let metadata = MetadataEntry::from_graphql(item, true, Language::default())?;
    let mut metadata = serde_json::to_value(metadata).ok()?;
    let fields = metadata.as_object_mut()?;
    fields.insert("sessionKey".into(), json!(session.key));
    fields.insert("viewOffset".into(), json!(session.position_ms));
    fields.insert(
        "Session".into(),
        json!({ "id": session.key, "location": "lan" }),
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
