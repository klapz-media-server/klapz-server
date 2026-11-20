//! `GET /library/search`: what Plex Web's search field calls.
//!
//! Advertised as the `universalsearch` feature and absent from the spec. One flat list of
//! `SearchResult`s, each a `score` around a `Metadata` or a `Directory`, grouped by kind and
//! cut to `limit` as a whole: films, series, episodes, then actors, directors, genres and
//! collections, each group best first. Items are ranked by the official API's `search`, people,
//! genres and collections as `/hubs/search` ranks its shelves of them. `docs/spec-gaps.md` says
//! how far this follows the reference.

use std::collections::HashMap;

use async_graphql::{Name, Value, Variables};
use axum::extract::{RawQuery, State};
use axum::routing::get;
use axum::Router;
use klapz_core::page::MAX_ITEMS;
use serde::{Deserialize, Serialize};

use crate::auth::Caller;
use crate::client::ClientIdentity;
use crate::error::ApiError;
use crate::gql;
use crate::gql_items::with_shared_fragments;
use crate::hub_search::{
    ranked_collections, ranked_tags, CollectionMatches, FoundItems, Shelf, TagMatches, TagResult,
    SEARCH_FRAGMENTS,
};
use crate::items::MetadataEntry;
use crate::language::Language;
use crate::response::{PlexResponse, ResponseFormat};
use crate::PlexState;

pub fn router() -> Router<PlexState> {
    Router::new().route("/library/search", get(search))
}

const DEFAULT_LIMIT: u32 = 10;

const QUERY: &str = r"
query PlexLibrarySearch(
  $query: String!, $first: Int!, $tags: Int!,
  $movies: Boolean!, $tv: Boolean!, $people: Boolean!, $categories: Boolean!
) {
  movies: search(query: $query, kinds: [MOVIE], first: $first) @include(if: $movies) {
    ...FoundItems
  }
  shows: search(query: $query, kinds: [SHOW], first: $first) @include(if: $tv) {
    ...FoundItems
  }
  episodes: search(query: $query, kinds: [EPISODE], episodesBySeries: true, first: $first)
    @include(if: $tv) {
    ...FoundItems
  }
  actors: searchTags(query: $query, kinds: [ACTOR], first: $tags) @include(if: $people) {
    ...FoundTags
  }
  directors: searchTags(query: $query, kinds: [DIRECTOR], first: $tags) @include(if: $people) {
    ...FoundTags
  }
  genres: searchTags(query: $query, kinds: [GENRE], first: $tags) @include(if: $categories) {
    ...FoundTags
  }
  collections(matching: $query, first: $tags) @include(if: $categories) {
    ...FoundCollections
  }
}
";

static QUERY_WITH_FRAGMENTS: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| with_shared_fragments(&format!("{QUERY}{SEARCH_FRAGMENTS}")));

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct SearchData {
    movies: FoundItems,
    shows: FoundItems,
    episodes: FoundItems,
    actors: TagMatches,
    directors: TagMatches,
    genres: TagMatches,
    collections: CollectionMatches,
}

#[derive(Debug, Serialize)]
struct SearchContainer {
    size: usize,
    #[serde(rename = "SearchResult", skip_serializing_if = "Vec::is_empty")]
    results: Vec<SearchResult>,
}

/// One result: an item, or a person, a genre or a collection in one library.
#[derive(Debug, Serialize)]
struct SearchResult {
    score: f64,
    #[serde(rename = "Metadata", skip_serializing_if = "Option::is_none")]
    metadata: Option<MetadataEntry>,
    #[serde(rename = "Directory", skip_serializing_if = "Option::is_none")]
    directory: Option<TagResult>,
}

/// What `searchTypes` asks for.
///
/// `movies` brings films and the genres and collections of film libraries, `tv` series and
/// episodes and the same of series libraries, `people` the actors and directors of the
/// libraries the other values name — of every library when it comes alone. A value naming
/// nothing held here (`music`, `photos`) or nothing at all narrows `people` all the same:
/// `music,people` and `people,bogus` find nobody on the reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SearchTypes {
    movies: bool,
    tv: bool,
    people: People,
}

/// Which libraries' people a search lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum People {
    None,
    /// `people` came alone.
    EveryLibrary,
    /// `people` came with other values, which name the libraries: films for `movies`,
    /// series for `tv`, none for the rest.
    NamedLibraries,
}

impl SearchTypes {
    /// Absent or empty, films, series and people, as on the reference.
    fn parse(search_types: Option<&str>) -> Self {
        let values: Vec<&str> = search_types
            .filter(|types| !types.is_empty())
            .unwrap_or("movies,tv,people")
            .split(',')
            .collect();
        let people = if !values.contains(&"people") {
            People::None
        } else if values.iter().all(|value| *value == "people") {
            People::EveryLibrary
        } else {
            People::NamedLibraries
        };
        Self {
            movies: values.contains(&"movies"),
            tv: values.contains(&"tv"),
            people,
        }
    }

    /// Whether a tag in a library whose items are of Plex type `library_type` is wanted, a
    /// person when `person` is set.
    const fn wants(self, library_type: u8, person: bool) -> bool {
        let library = match library_type {
            1 => self.movies,
            2 => self.tv,
            _ => false,
        };
        if !person {
            return library;
        }
        match self.people {
            People::None => false,
            People::EveryLibrary => true,
            People::NamedLibraries => library,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Request {
    term: String,
    limit: u32,
    types: SearchTypes,
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
        Ok(Self {
            term,
            limit: limit.min(MAX_ITEMS),
            types: SearchTypes::parse(params.get("searchTypes").map(AsRef::as_ref)),
        })
    }
}

fn item_result(found: FoundItems, language: Language) -> impl Iterator<Item = SearchResult> {
    found.edges.into_iter().filter_map(move |found| {
        let mut metadata = MetadataEntry::from_graphql(&found.node, true, language)?;
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
            score: found.score,
            metadata: Some(metadata),
            directory: None,
        })
    })
}

fn tag_results(
    tags: Vec<TagResult>,
    types: SearchTypes,
    person: bool,
) -> impl Iterator<Item = SearchResult> {
    tags.into_iter()
        .filter(move |tag| types.wants(tag.library_type(), person))
        .map(|tag| {
            let (score, directory) = tag.into_bare();
            SearchResult {
                score,
                metadata: None,
                directory: Some(directory),
            }
        })
}

async fn search(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    RawQuery(query): RawQuery,
    format: ResponseFormat,
) -> Result<PlexResponse<SearchContainer>, ApiError> {
    let request = Request::parse(query.as_deref().unwrap_or_default())?;
    let types = request.types;
    // Nothing asked that this server holds: not a query with every field left out, which
    // the official API answers with no data at all.
    if !(types.movies || types.tv || types.people != People::None) {
        return Ok(PlexResponse::new(
            SearchContainer {
                size: 0,
                results: Vec::new(),
            },
            format,
        ));
    }
    let mut variables = Variables::default();
    variables.insert(Name::new("query"), Value::String(request.term.clone()));
    variables.insert(Name::new("first"), Value::Number(request.limit.into()));
    variables.insert(Name::new("tags"), Value::Number(MAX_ITEMS.into()));
    variables.insert(Name::new("movies"), Value::Boolean(types.movies));
    variables.insert(Name::new("tv"), Value::Boolean(types.tv));
    variables.insert(
        Name::new("people"),
        Value::Boolean(types.people != People::None),
    );
    variables.insert(
        Name::new("categories"),
        Value::Boolean(types.movies || types.tv),
    );
    let data: SearchData = gql::execute(&state.schema, &QUERY_WITH_FRAGMENTS, variables).await?;

    let language = Language::of(&client);
    let (term, limit) = (request.term.trim(), request.limit);
    let results: Vec<SearchResult> = item_result(data.movies, language)
        .chain(item_result(data.shows, language))
        .chain(item_result(data.episodes, language))
        .chain(tag_results(
            ranked_tags(Shelf::Actor, data.actors.edges, term, MAX_ITEMS),
            types,
            true,
        ))
        .chain(tag_results(
            ranked_tags(Shelf::Director, data.directors.edges, term, MAX_ITEMS),
            types,
            true,
        ))
        .chain(tag_results(
            ranked_tags(Shelf::Genre, data.genres.edges, term, MAX_ITEMS),
            types,
            false,
        ))
        .chain(tag_results(
            ranked_collections(data.collections.edges, term, MAX_ITEMS),
            types,
            false,
        ))
        .take(limit as usize)
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
    fn defaults_to_ten_films_series_and_people() {
        let request = Request::parse("query=dune").unwrap();
        assert_eq!(request.limit, 10);
        let types = request.types;
        assert!(types.movies && types.tv && types.people == People::NamedLibraries);
        assert_eq!(SearchTypes::parse(Some("")), types);
    }

    #[test]
    fn people_come_from_the_libraries_the_other_types_name() {
        let wants = |search_types: &str, library_type: u8, person: bool| {
            SearchTypes::parse(Some(search_types)).wants(library_type, person)
        };
        // As measured for `luc besson`, whose credits are all in film libraries.
        assert!(wants("people", 1, true));
        assert!(wants("people", 2, true));
        assert!(wants("movies,people", 1, true));
        assert!(!wants("tv,people", 1, true));
        assert!(!wants("music,people", 1, true));
        assert!(!wants("people,bogus", 1, true));
        assert!(!wants("movies", 1, true));
        // Genres and collections follow the item types alone.
        assert!(wants("movies", 1, false));
        assert!(!wants("movies", 2, false));
        assert!(!wants("people", 1, false));
        assert!(wants("tv", 2, false));
    }
}
