//! Scanning: `/library/sections/{sectionId}/refresh` and the all-libraries forms, bound onto
//! `scanLibrary` and `cancelLibraryScan`.
//!
//! As measured on the reference (`docs/scanner/discovery.md` §1): a scan request answers `200`
//! with an empty body at once and runs in the background, a request while the library is being
//! scanned is coalesced into the running scan, and `force=1` does not rescan unchanged
//! directories — on the reference it refreshes metadata, which this server has no agent for,
//! so it changes nothing here. `path=`, a partial scan of a directory the client names, is
//! refused `403`: no endpoint accepts a path from a client. `DELETE` asks the running scan to
//! stop and answers `200` whether one was running or not.

use async_graphql::Variables;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{delete, get};
use axum::Router;
use open_plex_api::library::LIBRARY_TYPE;
use open_plex_api::GlobalId;
use serde::Deserialize;
use serde_json::json;

use crate::auth::Caller;
use crate::error::ApiError;
use crate::gql;
use crate::PlexState;

pub fn router() -> Router<PlexState> {
    Router::new()
        .route(
            "/library/sections/{sectionId}/refresh",
            get(refresh).post(refresh).delete(cancel),
        )
        .route(
            "/library/sections/refresh",
            get(refresh_all).post(refresh_all),
        )
        .route("/library/sections/all/refresh", delete(cancel_all))
}

#[derive(Debug, Deserialize)]
struct RefreshParams {
    path: Option<String>,
}

const SCAN: &str = r"
mutation PlexScanLibrary($input: ScanLibraryInput!) {
  scanLibrary(input: $input) { userErrors { code } }
}
";

const CANCEL: &str = r"
mutation PlexCancelLibraryScan($input: CancelLibraryScanInput!) {
  cancelLibraryScan(input: $input) { userErrors { code } }
}
";

const LIBRARIES: &str = r"
query PlexScannedLibraries {
  libraries(first: 500) { edges { node { id } } }
}
";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScanData {
    scan_library: Refusals,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CancelData {
    cancel_library_scan: Refusals,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Refusals {
    user_errors: Vec<UserError>,
}

#[derive(Debug, Deserialize)]
struct UserError {
    code: String,
}

#[derive(Debug, Deserialize)]
struct LibrariesData {
    libraries: Connection,
}

#[derive(Debug, Deserialize)]
struct Connection {
    edges: Vec<Edge>,
}

#[derive(Debug, Deserialize)]
struct Edge {
    node: Node,
}

#[derive(Debug, Deserialize)]
struct Node {
    id: String,
}

fn library_id(section: &str) -> Result<String, ApiError> {
    let section: i64 = section.parse().map_err(|_| ApiError::NotFound)?;
    Ok(GlobalId::encode(LIBRARY_TYPE, section))
}

fn empty_ok() -> impl IntoResponse {
    (StatusCode::OK, [(header::CONTENT_TYPE, "text/html")])
}

/// Starts a scan of the library `id` names; the query runs as the request's caller. Whether it started or one was
/// already running is the same to Plex.
async fn scan(state: &PlexState, id: &str) -> Result<(), ApiError> {
    let variables = Variables::from_json(json!({ "input": { "libraryId": id } }));
    let data: ScanData = gql::execute(&state.schema, SCAN, variables).await?;
    match data
        .scan_library
        .user_errors
        .first()
        .map(|error| error.code.as_str())
    {
        None | Some("ALREADY_RUNNING") => Ok(()),
        Some("UNSUPPORTED_KIND") => Err(ApiError::NotImplemented),
        Some(_) => Err(ApiError::NotFound),
    }
}

async fn cancel_scan(state: &PlexState, id: &str) -> Result<(), ApiError> {
    let variables = Variables::from_json(json!({ "input": { "libraryId": id } }));
    let data: CancelData = gql::execute(&state.schema, CANCEL, variables).await?;
    if data.cancel_library_scan.user_errors.is_empty() {
        Ok(())
    } else {
        Err(ApiError::NotFound)
    }
}

/// `GET` or `POST /library/sections/{sectionId}/refresh`.
async fn refresh(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(section): Path<String>,
    Query(params): Query<RefreshParams>,
) -> Result<impl IntoResponse, ApiError> {
    if params.path.is_some() {
        return Err(ApiError::Forbidden);
    }
    scan(&state, &library_id(&section)?).await?;
    Ok(empty_ok())
}

/// `DELETE /library/sections/{sectionId}/refresh`.
async fn cancel(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(section): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    cancel_scan(&state, &library_id(&section)?).await?;
    Ok(empty_ok())
}

async fn every_library(state: &PlexState) -> Result<Vec<String>, ApiError> {
    let data: LibrariesData = gql::execute(&state.schema, LIBRARIES, Variables::default()).await?;
    Ok(data
        .libraries
        .edges
        .into_iter()
        .map(|edge| edge.node.id)
        .collect())
}

/// `POST /library/sections/refresh`: every library the caller administers, the others
/// skipped. Music and photo libraries, which are not scanned here, are skipped too.
async fn refresh_all(
    _caller: Caller,
    State(state): State<PlexState>,
) -> Result<impl IntoResponse, ApiError> {
    for id in every_library(&state).await? {
        match scan(&state, &id).await {
            Ok(()) | Err(ApiError::NotFound | ApiError::NotImplemented) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(empty_ok())
}

/// `DELETE /library/sections/all/refresh`.
async fn cancel_all(
    _caller: Caller,
    State(state): State<PlexState>,
) -> Result<impl IntoResponse, ApiError> {
    for id in every_library(&state).await? {
        match cancel_scan(&state, &id).await {
            Ok(()) | Err(ApiError::NotFound) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(empty_ok())
}
