//! The updater: Plex's way of updating itself from plex.tv.
//!
//! Plex Web checks it on every load to decide whether to show an update banner. open-plex
//! is not updated through plex.tv, so the honest answer is the updater of an install that
//! cannot update itself: `canInstall` false and no release. Inventing one would show a
//! banner nothing can act on; borrowing Plex's would offer a Plex build.

use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, put};
use axum::Router;
use serde::Serialize;

use crate::auth::AdminCaller;
use crate::response::{PlexResponse, ResponseFormat};
use crate::PlexState;

pub fn router() -> Router<PlexState> {
    Router::new()
        .route("/updater/status", get(status))
        .route("/updater/check", put(check))
}

/// Response to `GET /updater/status`, in the reference's attribute order.
///
/// `checkedAt` and `downloadURL` are absent: no check has looked anywhere, and there is
/// nothing to download.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdaterStatus {
    pub size: u32,
    /// The updater protocol's version, which the spec pins at 1.
    pub auto_update_version: u8,
    pub can_install: bool,
    /// `0` is "no error".
    pub status: u8,
}

async fn status(_caller: AdminCaller, format: ResponseFormat) -> PlexResponse<UpdaterStatus> {
    PlexResponse::new(
        UpdaterStatus {
            size: 0,
            auto_update_version: 1,
            can_install: false,
            status: 0,
        },
        format,
    )
}

/// `PUT /updater/check` starts a check, whose result a client reads back from the status.
///
/// The reference acknowledges with an empty `200` typed `text/html`, and so does this: the
/// check finds nothing, since there is no channel to look in, and `download` has nothing to
/// fetch.
async fn check(_caller: AdminCaller) -> impl IntoResponse {
    (StatusCode::OK, [(header::CONTENT_TYPE, "text/html")])
}
