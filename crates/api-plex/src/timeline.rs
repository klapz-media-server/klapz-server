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
use crate::response::{PlexResponse, ResponseFormat};
use crate::PlexState;

const ITEM_EXISTS: &str = r"
query PlexTimelineItem($id: ID!) { node(id: $id) { id } }
";
const REPORT: &str = r"
mutation PlexReportPlayback($input: ReportPlaybackInput!) { reportPlayback(input: $input) }
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

    let item = params
        .rating_key
        .as_deref()
        .and_then(|key| key.parse().ok());
    if let Some(item) = item {
        require_item(&state, item).await?;
        if let Some(playback_id) = client.session_identifier {
            report(&state, playback_id, reported, position).await;
        }
    }

    Ok(PlexResponse::new(
        json!({ "size": 0, "playbackState": "ignore" }),
        format,
    ))
}
