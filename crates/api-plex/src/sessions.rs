//! `GET /status/sessions`: what is playing on the server right now.
//!
//! Plex Web polls it to draw the activity indicator. Nothing can be playing yet — there is
//! no playback at all — so the answer is the empty container the reference sends when its
//! own players are idle, and that is the truth rather than a placeholder. Sessions will come
//! from the official API once playback exists there.

use axum::routing::get;
use axum::Router;
use serde::Serialize;

use crate::auth::AdminCaller;
use crate::response::{PlexResponse, ResponseFormat};
use crate::PlexState;

pub fn router() -> Router<PlexState> {
    Router::new().route("/status/sessions", get(sessions))
}

/// Response to `GET /status/sessions`, as the reference sends it with nothing playing:
/// `<MediaContainer size="0">` and nothing inside.
#[derive(Debug, Serialize)]
pub struct SessionsContainer {
    pub size: u32,
}

/// Admin only, as the spec declares: the list names every user and what they watch.
async fn sessions(_caller: AdminCaller, format: ResponseFormat) -> PlexResponse<SessionsContainer> {
    PlexResponse::new(SessionsContainer { size: 0 }, format)
}
