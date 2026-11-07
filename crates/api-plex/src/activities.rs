//! `GET /activities`: the server's background work — scans, refreshes, analyses.
//!
//! Plex Web polls it to draw progress in its activity menu. This server runs no such work:
//! scanning, matching and analysis belong to the scanner, which is out of scope. So the
//! answer is the empty container the reference sends when it is idle, and that is the truth
//! rather than a placeholder.

use axum::routing::get;
use axum::Router;
use serde::Serialize;

use crate::auth::Caller;
use crate::response::{PlexResponse, ResponseFormat};
use crate::PlexState;

pub fn router() -> Router<PlexState> {
    Router::new().route("/activities", get(activities))
}

/// Response to `GET /activities`, as the reference sends it when idle:
/// `<MediaContainer size="0">` and nothing inside.
#[derive(Debug, Serialize)]
pub struct ActivitiesContainer {
    pub size: u32,
}

async fn activities(_caller: Caller, format: ResponseFormat) -> PlexResponse<ActivitiesContainer> {
    PlexResponse::new(ActivitiesContainer { size: 0 }, format)
}
