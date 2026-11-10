//! `/:/scrobble`, `/:/unscrobble` and `/:/progress`: a client marking an item watched,
//! unwatched, or remembering where it stopped.
//!
//! Each answers an empty `200` typed `text/html`, as the reference does. Declared `PUT` by
//! the spec; the reference answers `GET` too, which is what a browser address bar sends.

use async_graphql::{Name, Value, Variables};
use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use open_plex_api::item::ITEM_TYPE;
use open_plex_api::GlobalId;
use serde::Deserialize;

use crate::auth::Caller;
use crate::error::ApiError;
use crate::gql;
use crate::items::LIBRARY_IDENTIFIER;
use crate::PlexState;

const ITEM_EXISTS: &str = r"
query PlexWatchedItem($id: ID!) { node(id: $id) { id } }
";
const MARK_WATCHED: &str = r"
mutation PlexScrobble($id: ID!) { markWatched(id: $id) { viewCount } }
";
const MARK_UNWATCHED: &str = r"
mutation PlexUnscrobble($id: ID!) { markUnwatched(id: $id) { viewCount } }
";
const KEEP_POSITION: &str = r"
mutation PlexProgress($id: ID!, $position: Int!) {
  keepWatchPosition(id: $id, positionMs: $position) { viewCount }
}
";

pub fn add_routes(router: Router<PlexState>) -> Router<PlexState> {
    router
        .route("/:/scrobble", get(scrobble).put(scrobble))
        .route("/:/unscrobble", get(unscrobble).put(unscrobble))
        .route("/:/progress", get(progress).put(progress).post(progress))
}

#[derive(Debug, Deserialize)]
struct MarkParams {
    identifier: Option<String>,
    key: Option<String>,
    uri: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProgressParams {
    identifier: Option<String>,
    key: Option<String>,
    time: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NodeData {
    node: Option<serde::de::IgnoredAny>,
}

/// The empty acknowledgement the reference sends.
fn acknowledged() -> impl IntoResponse {
    (StatusCode::OK, [(header::CONTENT_TYPE, "text/html")])
}

/// The item a mark names, in the order the reference checks: an identifier first, then a
/// target. Anything but a library item — a foreign provider, a `uri`, a key that is no
/// number — is an item it does not have.
fn target(params: &MarkParams) -> Result<i64, ApiError> {
    let Some(identifier) = params.identifier.as_deref() else {
        return Err(ApiError::BadRequest);
    };
    match params.key.as_deref() {
        None if params.uri.is_none() => Err(ApiError::BadRequest),
        Some(key) if identifier == LIBRARY_IDENTIFIER => {
            key.parse().map_err(|_| ApiError::NotFound)
        }
        _ => Err(ApiError::NotFound),
    }
}

fn item_variables(key: i64) -> Variables {
    let mut variables = Variables::default();
    variables.insert(
        Name::new("id"),
        Value::String(GlobalId::encode(ITEM_TYPE, key)),
    );
    variables
}

/// Whether the caller can see the item: one it cannot is one that does not exist.
async fn visible(state: &PlexState, key: i64) -> Result<bool, ApiError> {
    let data: NodeData = gql::execute(&state.schema, ITEM_EXISTS, item_variables(key)).await?;
    Ok(data.node.is_some())
}

async fn mark(
    state: &PlexState,
    params: &MarkParams,
    mutation: &str,
) -> Result<impl IntoResponse, ApiError> {
    let key = target(params)?;
    if !visible(state, key).await? {
        return Err(ApiError::NotFound);
    }
    let _: serde::de::IgnoredAny =
        gql::execute(&state.schema, mutation, item_variables(key)).await?;
    Ok(acknowledged())
}

async fn scrobble(
    _caller: Caller,
    State(state): State<PlexState>,
    Query(params): Query<MarkParams>,
) -> Result<impl IntoResponse, ApiError> {
    mark(&state, &params, MARK_WATCHED).await
}

async fn unscrobble(
    _caller: Caller,
    State(state): State<PlexState>,
    Query(params): Query<MarkParams>,
) -> Result<impl IntoResponse, ApiError> {
    mark(&state, &params, MARK_UNWATCHED).await
}

/// Keeps a position with no check against the end, as the reference does: it keeps 95% this
/// way. An item it does not have is acknowledged all the same; a key that is no number is a
/// `404` here where the reference fails with a `500`.
async fn progress(
    _caller: Caller,
    State(state): State<PlexState>,
    Query(params): Query<ProgressParams>,
) -> Result<impl IntoResponse, ApiError> {
    if params.identifier.is_none() {
        return Err(ApiError::BadRequest);
    }
    let key: i64 = params
        .key
        .as_deref()
        .ok_or(ApiError::BadRequest)?
        .parse()
        .map_err(|_| ApiError::NotFound)?;
    let position: i64 = params
        .time
        .as_deref()
        .ok_or(ApiError::BadRequest)?
        .parse()
        .map_err(|_| ApiError::BadRequest)?;
    if !visible(&state, key).await? {
        return Ok(acknowledged());
    }
    let mut variables = item_variables(key);
    variables.insert(Name::new("position"), Value::Number(position.into()));
    let _: serde::de::IgnoredAny = gql::execute(&state.schema, KEEP_POSITION, variables).await?;
    Ok(acknowledged())
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    fn params(identifier: Option<&str>, key: Option<&str>, uri: Option<&str>) -> MarkParams {
        MarkParams {
            identifier: identifier.map(ToOwned::to_owned),
            key: key.map(ToOwned::to_owned),
            uri: uri.map(ToOwned::to_owned),
        }
    }

    #[test]
    fn checks_a_mark_in_the_order_the_reference_does() {
        let library = Some("com.plexapp.plugins.library");
        assert_eq!(
            target(&params(library, Some("577986"), None)).ok(),
            Some(577_986)
        );
        assert!(matches!(
            target(&params(None, Some("577986"), None)),
            Err(ApiError::BadRequest)
        ));
        assert!(matches!(
            target(&params(library, None, None)),
            Err(ApiError::BadRequest)
        ));
        // Measured as 404s: a foreign provider, a key that is no number, a `uri`.
        for case in [
            params(Some("tv.plex.provider.discover"), Some("577986"), None),
            params(library, Some("abc"), None),
            params(library, None, Some("library://x/item/577986")),
        ] {
            assert!(matches!(target(&case), Err(ApiError::NotFound)), "{case:?}");
        }
    }
}
