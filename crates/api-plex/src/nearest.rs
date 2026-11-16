//! `GET /library/metadata/{ids}/nearest` and `/library/sections/{sectionId}/nearest`: the
//! tracks that sound closest to a track, or to a sonic analysis.
//!
//! Music is held nowhere here, so no number names a track and no library holds an analysed
//! one. Both routes answer as the reference does for what is not music, measured on its film
//! and series libraries: what names nothing is a `404`, a parameter it cannot read a `400`,
//! and the rest a `403`.

use async_graphql::{Name, Value, Variables};
use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::Router;
use open_plex_api::collection::COLLECTION_TYPE;
use open_plex_api::item::ITEM_TYPE;
use open_plex_api::library::LIBRARY_TYPE;
use open_plex_api::GlobalId;
use open_plex_core::page::MAX_ITEMS;
use serde::Deserialize;

use crate::auth::Caller;
use crate::error::ApiError;
use crate::gql;
use crate::PlexState;

pub fn router() -> Router<PlexState> {
    Router::new()
        .route("/library/metadata/{ids}/nearest", get(item_nearest))
        .route(
            "/library/sections/{sectionId}/nearest",
            get(section_nearest),
        )
}

const NODES_QUERY: &str = r"
query PlexNearestNodes($items: [ID!]!, $collections: [ID!]!) {
  items: nodes(ids: $items) { id }
  collections: nodes(ids: $collections) { ... on Collection { id } }
}
";

#[derive(Debug, Deserialize)]
struct NodesData {
    items: Vec<Option<serde_json::Value>>,
    collections: Vec<Option<serde_json::Value>>,
}

const LIBRARY_QUERY: &str = r"
query PlexNearestLibrary($id: ID!) { node(id: $id) { ... on Library { id } } }
";

#[derive(Debug, Deserialize)]
struct LibraryData {
    node: Option<serde_json::Value>,
}

type Pairs = Query<Vec<(String, String)>>;

/// Any number of the list that names something, item or collection, makes it a request
/// about that, and nothing here is a track.
async fn item_nearest(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(ids): Path<String>,
    Query(pairs): Pairs,
) -> ApiError {
    match names_anything(&state, &ids).await {
        Ok(true) => {}
        Ok(false) => return ApiError::NotFound,
        Err(error) => return error,
    }
    let readable = readable::<i64>(&pairs, "limit")
        && readable::<f64>(&pairs, "maxDistance")
        && readable::<i64>(&pairs, "excludeParentID")
        && readable::<i64>(&pairs, "excludeGrandparentID");
    if readable {
        ApiError::Forbidden
    } else {
        ApiError::BadRequest
    }
}

/// `type` and `values` are required, `values` a list of numbers where an empty element is
/// skipped and an empty list read as one.
async fn section_nearest(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(section): Path<String>,
    Query(pairs): Pairs,
) -> ApiError {
    let Ok(section) = section.parse::<i64>() else {
        return ApiError::NotFound;
    };
    let mut variables = Variables::default();
    variables.insert(
        Name::new("id"),
        Value::String(GlobalId::encode(LIBRARY_TYPE, section)),
    );
    match gql::execute::<LibraryData>(&state.schema, LIBRARY_QUERY, variables).await {
        Ok(LibraryData { node: Some(_) }) => {}
        Ok(LibraryData { node: None }) => return ApiError::NotFound,
        Err(error) => return error,
    }

    let values = last(&pairs, "values").is_some_and(|values| {
        values
            .split(',')
            .filter(|value| !value.is_empty())
            .all(|value| value.parse::<f64>().is_ok())
    });
    let readable = values
        && last(&pairs, "type").is_some_and(|kind| kind.parse::<i64>().is_ok())
        && readable::<i64>(&pairs, "limit")
        && readable::<f64>(&pairs, "maxDistance");
    if readable {
        ApiError::Forbidden
    } else {
        ApiError::BadRequest
    }
}

/// Untrimmed, as on the reference, where ` 21824` names nothing.
async fn names_anything(state: &PlexState, ids: &str) -> Result<bool, ApiError> {
    let keys: Vec<i64> = ids.split(',').filter_map(|key| key.parse().ok()).collect();
    if keys.is_empty() || keys.len() > MAX_ITEMS as usize {
        return Ok(false);
    }
    let encoded = |kind: &str| {
        Value::List(
            keys.iter()
                .map(|key| Value::String(GlobalId::encode(kind, *key)))
                .collect(),
        )
    };
    let mut variables = Variables::default();
    variables.insert(Name::new("items"), encoded(ITEM_TYPE));
    variables.insert(Name::new("collections"), encoded(COLLECTION_TYPE));
    let data: NodesData = gql::execute(&state.schema, NODES_QUERY, variables).await?;
    Ok(data
        .items
        .iter()
        .chain(&data.collections)
        .any(Option::is_some))
}

fn last<'a>(pairs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    pairs
        .iter()
        .rev()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

/// Absent, or present and a `T`: an empty value is not one.
fn readable<T: std::str::FromStr>(pairs: &[(String, String)], name: &str) -> bool {
    last(pairs, name).is_none_or(|value| value.parse::<T>().is_ok())
}
