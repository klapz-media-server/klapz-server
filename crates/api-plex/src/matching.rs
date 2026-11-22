//! Matching from Plex clients' "Fix match", "Unmatch" and "Refresh metadata":
//! `PUT /library/metadata/{ids}/matches`, `/match`, `/unmatch` and `/refresh`, bound onto
//! `metadataMatches`, `matchItem`, `unmatchItem` and `refreshItemMetadata`.
//!
//! The reference's results carry its own provider's guids (`plex://movie/…`). Ours name the
//! record at this server's provider, `{agent}://{id}` (`tmdb://603`), which `/match` takes back
//! in its `guid`. `/match` and `/refresh` answer once the work is started, as the reference's.

use async_graphql::Variables;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::put;
use axum::Router;
use klapz_api::item::ITEM_TYPE;
use klapz_api::GlobalId;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::Caller;
use crate::error::ApiError;
use crate::gql;
use crate::items::LIBRARY_IDENTIFIER;
use crate::response::{PlexResponse, ResponseFormat};
use crate::PlexState;

pub fn router() -> Router<PlexState> {
    Router::new()
        .route("/library/metadata/{ids}/matches", put(matches))
        .route("/library/metadata/{ids}/match", put(match_item))
        .route("/library/metadata/{ids}/unmatch", put(unmatch))
        .route("/library/metadata/{ids}/refresh", put(refresh))
}

const MATCHES: &str = r"
query PlexMatches($id: ID!, $title: String, $year: Int) {
  node(id: $id) { __typename }
  metadataMatches(itemId: $id, title: $title, year: $year) { agent providerId name year score }
}
";

#[derive(Debug, Deserialize)]
struct MatchesData {
    node: Option<Typed>,
    #[serde(rename = "metadataMatches")]
    matches: Vec<Found>,
}

#[derive(Debug, Deserialize)]
struct Typed {
    #[serde(rename = "__typename")]
    typename: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Found {
    agent: String,
    provider_id: String,
    name: String,
    year: Option<i32>,
    score: i32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MatchResults {
    size: usize,
    identifier: &'static str,
    #[serde(rename = "SearchResult", skip_serializing_if = "Vec::is_empty")]
    results: Vec<SearchResult>,
}

#[derive(Debug, Serialize)]
struct SearchResult {
    guid: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    year: Option<i32>,
    score: i32,
    #[serde(rename = "type")]
    kind: &'static str,
}

#[derive(Debug, Deserialize)]
struct MatchesParams {
    title: Option<String>,
    year: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MatchParams {
    guid: Option<String>,
}

/// The one item an `{ids}` names: these routes act on a film or a series at a time.
fn item(ids: &str) -> Result<String, ApiError> {
    let id: i64 = ids.parse().map_err(|_| ApiError::NotFound)?;
    Ok(GlobalId::encode(ITEM_TYPE, id))
}

/// `PUT /library/metadata/{ids}/matches`.
async fn matches(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(ids): Path<String>,
    Query(params): Query<MatchesParams>,
    format: ResponseFormat,
) -> Result<impl IntoResponse, ApiError> {
    let id = item(&ids)?;
    let year: Option<i32> = params.year.as_deref().and_then(|year| year.parse().ok());
    let title = params.title.filter(|title| !title.trim().is_empty());
    let variables = Variables::from_json(json!({ "id": id, "title": title, "year": year }));
    let data: MatchesData = gql::execute(&state.schema, MATCHES, variables).await?;
    let kind = match data.node.ok_or(ApiError::NotFound)?.typename.as_str() {
        "Movie" => "movie",
        "Show" => "show",
        _ => return Err(ApiError::NotFound),
    };
    let results: Vec<SearchResult> = data
        .matches
        .into_iter()
        .map(|found| SearchResult {
            guid: format!("{}://{}", found.agent, found.provider_id),
            name: found.name,
            year: found.year,
            score: found.score,
            kind,
        })
        .collect();
    Ok(PlexResponse::new(
        MatchResults {
            size: results.len(),
            identifier: LIBRARY_IDENTIFIER,
            results,
        },
        format,
    ))
}

const MATCH_ITEM: &str = r"
mutation PlexMatch($id: ID!, $providerId: String) {
  matchItem(input: { itemId: $id, providerId: $providerId }) { userErrors { code } }
}
";

const REFRESH: &str = r"
mutation PlexRefresh($id: ID!) {
  refreshItemMetadata(input: { itemId: $id }) { userErrors { code } }
}
";

const UNMATCH: &str = r"
mutation PlexUnmatch($id: ID!) {
  unmatchItem(input: { itemId: $id }) { userErrors { code } }
}
";

#[derive(Debug, Deserialize)]
struct Payload {
    #[serde(rename = "userErrors")]
    user_errors: Vec<UserError>,
}

#[derive(Debug, Deserialize)]
struct UserError {
    code: String,
}

/// Runs a matching mutation, whose payload sits under its own name, and answers as the
/// reference does: `200` and no body.
async fn run(
    state: &PlexState,
    query: &str,
    field: &str,
    variables: serde_json::Value,
) -> Result<impl IntoResponse, ApiError> {
    let data: serde_json::Value =
        gql::execute(&state.schema, query, Variables::from_json(variables)).await?;
    let payload: Payload = serde_json::from_value(data[field].clone())
        .map_err(|error| ApiError::Internal(Box::new(error)))?;
    match payload.user_errors.first().map(|error| error.code.as_str()) {
        None => Ok((StatusCode::OK, [(header::CONTENT_TYPE, "text/html")])),
        Some("ITEM_NOT_FOUND") => Err(ApiError::NotFound),
        Some(_) => Err(ApiError::NotImplemented),
    }
}

/// `PUT /library/metadata/{ids}/match?guid={agent}://{id}`.
async fn match_item(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(ids): Path<String>,
    Query(params): Query<MatchParams>,
) -> Result<impl IntoResponse, ApiError> {
    let id = item(&ids)?;
    // A guid of another provider, a reference server's `plex://` one, names nothing here.
    let provider_id = match params.guid.as_deref() {
        Some(guid) => {
            let (_, provider_id) = guid
                .split_once("://")
                .filter(|(agent, id)| matches!(*agent, "tmdb" | "tvdb") && !id.is_empty())
                .ok_or(ApiError::BadRequest)?;
            Some(provider_id.to_owned())
        }
        None => None,
    };
    run(
        &state,
        MATCH_ITEM,
        "matchItem",
        json!({ "id": id, "providerId": provider_id }),
    )
    .await
}

/// `PUT /library/metadata/{ids}/refresh`.
async fn refresh(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(ids): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let id = item(&ids)?;
    run(&state, REFRESH, "refreshItemMetadata", json!({ "id": id })).await
}

/// `PUT /library/metadata/{ids}/unmatch`.
async fn unmatch(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(ids): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let id = item(&ids)?;
    run(&state, UNMATCH, "unmatchItem", json!({ "id": id })).await
}
