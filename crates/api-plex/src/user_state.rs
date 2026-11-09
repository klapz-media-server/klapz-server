//! `GET /library/metadata/{id}/userState`: what Plex Web asks to draw its watchlist button.
//!
//! A route of plex.tv's Discover provider rather than of a server, whose answer this copies. The
//! reference has no such route and answers `404`; Plex Web can still send it here for an item
//! without a Plex GUID. Nothing is watched or watchlisted as far as this server knows, because
//! watch state is not recorded yet.

use axum::extract::{Path, State};
use axum::routing::get;
use axum::Router;
use serde::Serialize;

use crate::auth::Caller;
use crate::error::ApiError;
use crate::items::LIBRARY_IDENTIFIER;
use crate::playback::item;
use crate::response::{PlexResponse, ResponseFormat};
use crate::PlexState;

pub fn router() -> Router<PlexState> {
    Router::new().route("/library/metadata/{id}/userState", get(user_state))
}

#[derive(Debug, Serialize)]
pub struct UserStateContainer {
    identifier: &'static str,
    size: u32,
    #[serde(rename = "UserState")]
    user_state: Vec<UserState>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UserState {
    rating_key: String,
    #[serde(rename = "type")]
    kind: &'static str,
    view_count: u32,
    view_offset: u32,
}

async fn user_state(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(id): Path<String>,
    format: ResponseFormat,
) -> Result<PlexResponse<UserStateContainer>, ApiError> {
    let key: i64 = id.parse().map_err(|_| ApiError::NotFound)?;
    let found = item(&state, key).await.map_err(|error| match error {
        // How that lookup says the item is absent, or hidden from this caller.
        ApiError::BadRequest => ApiError::NotFound,
        other => other,
    })?;
    Ok(PlexResponse::new(
        UserStateContainer {
            identifier: LIBRARY_IDENTIFIER,
            size: 1,
            user_state: vec![UserState {
                rating_key: key.to_string(),
                kind: found.plex_type(),
                view_count: 0,
                view_offset: 0,
            }],
        },
        format,
    ))
}
