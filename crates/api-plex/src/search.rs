//! `GET /library/search`: what Plex Web's search field calls.
//!
//! Advertised as the `universalsearch` feature and absent from the spec. Results are films,
//! series and episodes. The people and collections the reference mixes in are not served yet:
//! `searchTypes=people` is tolerated rather than refused, because Plex Web sends it with every
//! search and refusing would take the search field down with it.
//!
//! The reference scores its matches with a fuzzy rule of its own; `docs/spec-gaps.md` says how
//! far this one follows it.

use std::cmp::Reverse;
use std::collections::HashMap;

use async_graphql::{Name, Value, Variables};
use axum::extract::{RawQuery, State};
use axum::routing::get;
use axum::Router;
use open_plex_core::page::MAX_ITEMS;
use open_plex_core::search::fold;
use serde::{Deserialize, Serialize};

use crate::auth::Caller;
use crate::client::ClientIdentity;
use crate::error::ApiError;
use crate::gql;
use crate::gql_items::{with_shared_fragments, GqlItem, GqlItemEdge};
use crate::items::MetadataEntry;
use crate::language::Language;
use crate::response::{PlexResponse, ResponseFormat};
use crate::PlexState;

pub fn router() -> Router<PlexState> {
    Router::new().route("/library/search", get(search))
}

const DEFAULT_LIMIT: u32 = 10;

/// The score of each [`tier`], lowest first: values the reference gives such titles.
const SCORES: [f64; 3] = [0.33, 0.73, 0.93];

/// How many matches are ranked before cutting to `limit`. Storage answers in title order, so
/// without this margin an exact match would lose its place to whatever sorts before it.
const RANKED_MATCHES: u32 = 100;

const QUERY: &str = r"
query PlexLibrarySearch($query: String!, $kinds: [ItemType!], $first: Int!) {
  search(query: $query, kinds: $kinds, first: $first) {
    edges { node { ...ListedItem } }
  }
}
";

static QUERY_WITH_FRAGMENTS: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| with_shared_fragments(QUERY));

#[derive(Debug, Deserialize)]
struct SearchData {
    search: SearchConnection,
}

#[derive(Debug, Deserialize)]
struct SearchConnection {
    edges: Vec<GqlItemEdge>,
}

#[derive(Debug, Serialize)]
struct SearchContainer {
    size: usize,
    #[serde(rename = "SearchResult", skip_serializing_if = "Vec::is_empty")]
    results: Vec<SearchResult>,
}

#[derive(Debug, Serialize)]
struct SearchResult {
    score: f64,
    #[serde(rename = "Metadata")]
    metadata: MetadataEntry,
}

#[derive(Debug, PartialEq, Eq)]
struct Request {
    term: String,
    limit: u32,
    kinds: Vec<&'static str>,
}

impl Request {
    /// A `limit` of zero answers `500` on the reference, which is a fault rather than a rule;
    /// it is a `400` here, as any other unusable limit is.
    fn parse(query: &str) -> Result<Self, ApiError> {
        let params: HashMap<_, _> = form_urlencoded::parse(query.as_bytes()).collect();
        let term = params.get("query").ok_or(ApiError::BadRequest)?.to_string();
        let limit = match params.get("limit") {
            Some(limit) => limit
                .parse()
                .ok()
                .filter(|limit| *limit > 0)
                .ok_or(ApiError::BadRequest)?,
            None => DEFAULT_LIMIT,
        };
        let kinds = params
            .get("searchTypes")
            .map_or_else(|| kinds_of("movies,tv"), |types| kinds_of(types));
        Ok(Self { term, limit, kinds })
    }
}

fn kinds_of(search_types: &str) -> Vec<&'static str> {
    search_types
        .split(',')
        .flat_map(|search_type| match search_type {
            "movies" => &["MOVIE"][..],
            "tv" => &["SHOW", "EPISODE"][..],
            _ => &[],
        })
        .copied()
        .collect()
}

/// How well a title answers the term: equal to it, starting with it, or merely holding it.
fn tier(item: &GqlItem, folded_term: &str) -> usize {
    let title = item.title.as_deref().map(fold).unwrap_or_default();
    if title == folded_term {
        2
    } else {
        usize::from(title.starts_with(folded_term))
    }
}

async fn matches(
    state: &PlexState,
    term: &str,
    kinds: Vec<&str>,
    limit: u32,
) -> Result<Vec<GqlItemEdge>, ApiError> {
    // Not sent as an empty list, which the official API reads as every kind.
    if kinds.is_empty() {
        return Ok(Vec::new());
    }
    let mut variables = Variables::default();
    variables.insert(Name::new("query"), Value::String(term.to_owned()));
    variables.insert(
        Name::new("kinds"),
        Value::List(
            kinds
                .into_iter()
                .map(|kind| Value::Enum(Name::new(kind)))
                .collect(),
        ),
    );
    variables.insert(
        Name::new("first"),
        Value::Number(limit.clamp(RANKED_MATCHES, MAX_ITEMS).into()),
    );
    let data: SearchData = gql::execute(&state.schema, &QUERY_WITH_FRAGMENTS, variables).await?;
    Ok(data.search.edges)
}

async fn search(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    RawQuery(query): RawQuery,
    format: ResponseFormat,
) -> Result<PlexResponse<SearchContainer>, ApiError> {
    let request = Request::parse(query.as_deref().unwrap_or_default())?;
    let limit = request.limit.min(MAX_ITEMS);
    let folded_term = fold(&request.term);

    // Asked apart: a popular term matches hundreds of episodes, whose titles would otherwise
    // fill the pool and leave out the series everyone is looking for.
    let (episodes, titles): (Vec<_>, Vec<_>) = request
        .kinds
        .into_iter()
        .partition(|kind| *kind == "EPISODE");
    let (titles, episodes) = tokio::join!(
        matches(&state, &request.term, titles, limit),
        matches(&state, &request.term, episodes, limit),
    );
    let (titles, episodes) = (titles?, episodes?);

    let language = Language::of(&client);
    let mut ranked: Vec<(usize, &GqlItem)> = titles
        .iter()
        .chain(&episodes)
        .map(|edge| (tier(&edge.node, &folded_term), &edge.node))
        .collect();
    // Stable, so title order survives inside a tier.
    ranked.sort_by_key(|(tier, _)| Reverse(*tier));

    let results: Vec<SearchResult> = ranked
        .into_iter()
        .take(limit as usize)
        .filter_map(|(tier, item)| {
            let mut metadata = MetadataEntry::from_graphql(item, true, language)?;
            // Every result names its library, but only a series says where that library is.
            if metadata.kind != "show" {
                metadata.library_section_id = None;
                metadata.library_section_key = None;
            }
            // Empty rather than absent, on the seven films measured with no synopsis.
            if metadata.kind == "movie" {
                metadata.summary.get_or_insert_with(String::new);
            }
            Some(SearchResult {
                score: SCORES[tier],
                metadata,
            })
        })
        .collect();
    Ok(PlexResponse::new(
        SearchContainer {
            size: results.len(),
            results,
        },
        format,
    ))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn a_search_needs_a_term_and_a_usable_limit() {
        for query in ["limit=30", "query=dune&limit=abc", "query=dune&limit=0"] {
            assert!(
                matches!(Request::parse(query), Err(ApiError::BadRequest)),
                "{query}"
            );
        }
    }

    #[test]
    fn defaults_to_ten_films_and_series() {
        let request = Request::parse("query=dune").unwrap();
        assert_eq!(request.limit, 10);
        assert_eq!(request.kinds, ["MOVIE", "SHOW", "EPISODE"]);
    }

    #[test]
    fn maps_search_types_to_kinds() {
        assert_eq!(kinds_of("movies"), ["MOVIE"]);
        assert_eq!(kinds_of("tv"), ["SHOW", "EPISODE"]);
        assert_eq!(kinds_of("movies,tv,people"), ["MOVIE", "SHOW", "EPISODE"]);
        assert!(kinds_of("music,livetv,bogus").is_empty());
    }
}
