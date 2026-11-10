//! `GET /library/metadata/{ids}/related` and `/similar`: the shelves under an item.
//!
//! A film gets films like it, then more by its director and with its two first-billed
//! actors, each only when at least five other films back it. A series gets series like it.
//! Seasons and episodes have none on the reference either.
//!
//! Not served: the reference's collection shelves (collections are not imported), its
//! `external.*` shelves, and the network and cast shelves of a series. Which films are alike
//! is an approximation; see `open_plex_core::similar`.

use async_graphql::{Name, Value, Variables};
use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::Router;
use open_plex_api::item::ITEM_TYPE;
use open_plex_api::library::LIBRARY_TYPE;
use open_plex_api::GlobalId;
use open_plex_core::page::MAX_ITEMS;
use serde::{Deserialize, Serialize};

use crate::auth::Caller;
use crate::client::ClientIdentity;
use crate::error::ApiError;
use crate::gql;
use crate::gql_items::{
    with_shared_fragments, GqlConnection, GqlItem, GqlItemEdge, GqlLibraryRef, GqlTag,
    SectionItemsData, SECTION_ITEMS_QUERY,
};
use crate::hubs::Hub;
use crate::items::{MetadataEntry, LIBRARY_IDENTIFIER, MEDIA_TAG_PREFIX, MEDIA_TAG_VERSION};
use crate::language::Language;
use crate::listing::{Filters, TagFilter};
use crate::response::{PlexResponse, ResponseFormat};
use crate::PlexState;

pub fn router() -> Router<PlexState> {
    Router::new()
        .route("/library/metadata/{ids}/related", get(related))
        .route("/library/metadata/{ids}/similar", get(similar))
}

const DEFAULT_SHELF: u32 = 6;
/// The spec's default for `/similar`.
const DEFAULT_SIMILAR: u32 = 200;

/// Other films a person needs for a shelf of their own. Measured on 150 films: every
/// first-billed actor and director with five or more has one, and of the 60 with three or
/// four only two do.
const MIN_OTHERS: usize = 5;

/// How many first-billed actors can get a shelf: the third never does, on those same films.
const BILLED_ACTORS: i32 = 2;

static RELATED_QUERY: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    with_shared_fragments(
        r"
query PlexRelated($id: ID!, $first: Int!, $actors: Int!) {
  node(id: $id) {
    ... on Item {
      __typename
      library { id title uuid kind }
    }
    ... on Movie {
      similar(first: $first) { edges { node { ...ListedItem } } }
      tags {
        directors(first: 1) { edges { node { id name } } }
        cast(first: $actors) { edges { node { id name } } }
      }
    }
    ... on Show {
      similar(first: $first) { edges { node { ...ListedItem } } }
    }
  }
}
",
    )
});

#[derive(Debug, Deserialize)]
struct RelatedData {
    node: Option<GqlRelated>,
}

#[derive(Debug, Deserialize)]
struct GqlRelated {
    #[serde(rename = "__typename")]
    typename: String,
    library: GqlLibraryRef,
    similar: Option<GqlSimilar>,
    tags: Option<GqlPeople>,
}

#[derive(Debug, Deserialize)]
struct GqlSimilar {
    edges: Vec<GqlItemEdge>,
}

#[derive(Debug, Deserialize)]
struct GqlPeople {
    directors: GqlConnection<GqlTag>,
    cast: GqlConnection<GqlTag>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RelatedContainer {
    size: u32,
    allow_sync: bool,
    identifier: &'static str,
    #[serde(rename = "librarySectionID")]
    library_section_id: i64,
    library_section_title: String,
    #[serde(rename = "librarySectionUUID")]
    library_section_uuid: String,
    media_tag_prefix: &'static str,
    media_tag_version: i64,
    #[serde(rename = "Hub", skip_serializing_if = "Vec::is_empty")]
    hub: Vec<Hub>,
}

/// Bare next to a listing's: no library attributes, no title, as the reference sends it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SimilarContainer {
    size: usize,
    identifier: &'static str,
    media_tag_prefix: &'static str,
    media_tag_version: i64,
    #[serde(rename = "Metadata", skip_serializing_if = "Vec::is_empty")]
    metadata: Vec<MetadataEntry>,
}

#[derive(Debug, Default, Deserialize)]
struct Params {
    count: Option<String>,
}

impl Params {
    fn count(&self, default: u32) -> u32 {
        self.count
            .as_deref()
            .and_then(|count| count.parse().ok())
            .unwrap_or(default)
            .min(MAX_ITEMS)
    }
}

/// The path takes a list, as every `/library/metadata/{ids}` does; only the first is read.
fn rating_key(ids: &str) -> Result<i64, ApiError> {
    ids.split(',')
        .next()
        .and_then(|key| key.trim().parse().ok())
        .ok_or(ApiError::NotFound)
}

fn shelf_of(
    items: Vec<MetadataEntry>,
    count: u32,
    available: usize,
) -> (Vec<MetadataEntry>, u32, bool, Option<String>) {
    let items: Vec<MetadataEntry> = items.into_iter().take(count as usize).collect();
    let keys: Vec<&str> = items.iter().map(|item| item.rating_key.as_str()).collect();
    let hub_key = (!keys.is_empty()).then(|| format!("/library/metadata/{}", keys.join(",")));
    let size = u32::try_from(items.len()).unwrap_or(u32::MAX);
    (items, size, available > count as usize, hub_key)
}

fn entries(edges: &[GqlItemEdge], language: Language) -> Vec<MetadataEntry> {
    edges
        .iter()
        .filter_map(|edge: &GqlItemEdge| MetadataEntry::from_graphql(&edge.node, false, language))
        .collect()
}

fn similar_shelf(
    item: i64,
    is_film: bool,
    edges: &[GqlItemEdge],
    count: u32,
    language: Language,
) -> Option<Hub> {
    if edges.is_empty() {
        return None;
    }
    let (kind, prefix, title) = if is_film {
        ("movie", "movie", language.related_movies())
    } else {
        ("show", "tv", language.related_shows())
    };
    let (metadata, size, more, hub_key) = shelf_of(entries(edges, language), count, edges.len());
    Some(Hub {
        hub_key,
        key: format!("/library/metadata/{item}/similar"),
        title: title.to_owned(),
        kind,
        hub_identifier: format!("{prefix}.similar"),
        context: if is_film {
            "hub.movie.similar"
        } else {
            "hub.tv.similar"
        },
        size,
        more,
        random: None,
        style: "shelf",
        promoted: None,
        metadata,
    })
}

/// One person's other films, or nothing when they have too few for a shelf.
async fn person_films(
    state: &PlexState,
    section: i64,
    item: i64,
    field: &'static str,
    tag: i64,
    count: u32,
) -> Result<Option<Vec<GqlItem>>, ApiError> {
    let filters = Filters {
        tags: vec![TagFilter {
            kind: field,
            ids: vec![tag],
            names: Vec::new(),
        }],
        excluded_id: Some(item),
        ..Filters::default()
    };
    let first = count.max(u32::try_from(MIN_OTHERS).unwrap_or(u32::MAX)) + 1;

    let mut variables = Variables::default();
    variables.insert(
        Name::new("id"),
        Value::String(GlobalId::encode(LIBRARY_TYPE, section)),
    );
    variables.insert(Name::new("first"), Value::Number(first.into()));
    variables.insert(Name::new("offset"), Value::Number(0.into()));
    variables.insert(Name::new("orderBy"), Value::Enum(Name::new("TITLE")));
    variables.insert(Name::new("direction"), Value::Enum(Name::new("ASCENDING")));
    variables.insert(Name::new("filter"), filters.to_graphql(Some("MOVIE")));

    let data: SectionItemsData =
        gql::execute(&state.schema, &SECTION_ITEMS_QUERY, variables).await?;
    let others: Vec<GqlItem> = data
        .node
        .ok_or(ApiError::NotFound)?
        .items
        .edges
        .into_iter()
        .map(|edge| edge.node)
        .collect();
    Ok((others.len() >= MIN_OTHERS).then_some(others))
}

/// What differs between a director's shelf and an actor's.
struct PersonShelf {
    tag: i64,
    /// The Plex filter parameter, and the schema's name for the same tag kind.
    parameter: &'static str,
    tag_kind: &'static str,
    title: String,
    identifier: String,
    context: &'static str,
}

impl PersonShelf {
    async fn hub(
        self,
        state: &PlexState,
        section: i64,
        item: i64,
        count: u32,
        language: Language,
    ) -> Result<Option<Hub>, ApiError> {
        let Some(others) =
            person_films(state, section, item, self.tag_kind, self.tag, count).await?
        else {
            return Ok(None);
        };
        let available = others.len();
        let items = others
            .iter()
            .filter_map(|other| MetadataEntry::from_graphql(other, false, language))
            .collect();
        let (metadata, size, more, hub_key) = shelf_of(items, count, available);
        Ok(Some(Hub {
            hub_key,
            key: format!(
                "/library/sections/{section}/all?{}={}&id!={item}",
                self.parameter, self.tag
            ),
            title: self.title,
            kind: "movie",
            hub_identifier: self.identifier,
            context: self.context,
            size,
            more,
            random: None,
            style: "shelf",
            promoted: None,
            metadata,
        }))
    }
}

async fn related(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    Path(ids): Path<String>,
    Query(params): Query<Params>,
    format: ResponseFormat,
) -> Result<PlexResponse<RelatedContainer>, ApiError> {
    let item = rating_key(&ids)?;
    let count = params.count(DEFAULT_SHELF);
    let language = Language::of(&client);

    let mut variables = Variables::default();
    variables.insert(
        Name::new("id"),
        Value::String(GlobalId::encode(ITEM_TYPE, item)),
    );
    variables.insert(Name::new("first"), Value::Number((count + 1).into()));
    variables.insert(Name::new("actors"), Value::Number(BILLED_ACTORS.into()));
    let data: RelatedData = gql::execute(&state.schema, &RELATED_QUERY, variables).await?;
    let node = data.node.ok_or(ApiError::NotFound)?;

    let section = node.library.database_id();
    let is_film = node.typename == "Movie";
    let mut hubs: Vec<Hub> = Vec::new();

    if let Some(similar) = &node.similar {
        hubs.extend(similar_shelf(
            item,
            is_film,
            &similar.edges,
            count,
            language,
        ));
    }
    if let Some(people) = node.tags.filter(|_| is_film) {
        let directors = people.directors.edges.into_iter().map(|edge| {
            let name = edge.node.name;
            (
                edge.node.id,
                "director",
                "DIRECTOR",
                language.more_by(&name),
                "movie.same.director".to_owned(),
                "hub.movie.same.director",
            )
        });
        let actors = people
            .cast
            .edges
            .into_iter()
            .enumerate()
            .map(|(index, edge)| {
                let name = edge.node.name;
                (
                    edge.node.id,
                    "actor",
                    "ACTOR",
                    language.more_with(&name),
                    format!("movie.same.actor.{index}"),
                    "hub.movie.same.actor",
                )
            });
        for (id, parameter, tag_kind, title, identifier, context) in directors.chain(actors) {
            let Some(tag) = id.as_deref().map(crate::gql_items::database_id) else {
                continue;
            };
            let shelf = PersonShelf {
                tag,
                parameter,
                tag_kind,
                title,
                identifier,
                context,
            };
            hubs.extend(shelf.hub(&state, section, item, count, language).await?);
        }
    }

    Ok(PlexResponse::new(
        RelatedContainer {
            size: u32::try_from(hubs.len()).unwrap_or(u32::MAX),
            allow_sync: true,
            identifier: LIBRARY_IDENTIFIER,
            library_section_id: section,
            library_section_title: node.library.title.clone(),
            library_section_uuid: node.library.uuid.clone(),
            media_tag_prefix: MEDIA_TAG_PREFIX,
            media_tag_version: MEDIA_TAG_VERSION,
            hub: hubs,
        },
        format,
    ))
}

async fn similar(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    Path(ids): Path<String>,
    Query(params): Query<Params>,
    format: ResponseFormat,
) -> Result<PlexResponse<SimilarContainer>, ApiError> {
    let item = rating_key(&ids)?;
    let count = params.count(DEFAULT_SIMILAR);
    let language = Language::of(&client);

    let mut variables = Variables::default();
    variables.insert(
        Name::new("id"),
        Value::String(GlobalId::encode(ITEM_TYPE, item)),
    );
    variables.insert(Name::new("first"), Value::Number(count.into()));
    variables.insert(Name::new("actors"), Value::Number(0.into()));
    let data: RelatedData = gql::execute(&state.schema, &RELATED_QUERY, variables).await?;
    let node = data.node.ok_or(ApiError::NotFound)?;

    let metadata = node
        .similar
        .map(|similar| entries(&similar.edges, language))
        .unwrap_or_default();
    Ok(PlexResponse::new(
        SimilarContainer {
            size: metadata.len(),
            identifier: LIBRARY_IDENTIFIER,
            media_tag_prefix: MEDIA_TAG_PREFIX,
            media_tag_version: MEDIA_TAG_VERSION,
            metadata,
        },
        format,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_first_key_of_the_list() {
        assert_eq!(rating_key("12").ok(), Some(12));
        assert_eq!(rating_key("12,13").ok(), Some(12));
        assert!(matches!(rating_key("abc"), Err(ApiError::NotFound)));
    }

    #[test]
    fn a_count_that_is_not_a_number_is_the_default() {
        let params = Params {
            count: Some("abc".to_owned()),
        };
        assert_eq!(params.count(DEFAULT_SHELF), 6);
        assert_eq!(Params::default().count(DEFAULT_SIMILAR), 200);
    }
}
