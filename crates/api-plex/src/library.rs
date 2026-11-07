//! Library navigation, served by translating GraphQL results into `MediaContainer`s.

use async_graphql::{Name, Value, Variables};
use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::Router;
use open_plex_api::item::ITEM_TYPE;
use open_plex_api::library::LIBRARY_TYPE;
use open_plex_api::GlobalId;
use open_plex_core::page::MAX_ITEMS;
use serde::Deserialize;

use crate::auth::Caller;
use crate::children;
use crate::container::{kind_rank, LibraryKind, SectionDirectory, SectionsContainer};
use crate::error::ApiError;
use crate::gql;
use crate::gql_items::GqlLibraryRef;
use crate::gql_items::{
    with_shared_fragments, GqlItemEdge, ItemsByIdData, SectionItemsData, ITEMS_BY_ID_QUERY,
    SECTION_ITEMS_QUERY,
};
use crate::items::{
    CharacterDirectory, CharactersContainer, ItemContainer, ItemsContainer, MetadataEntry,
    SearchContainer, SearchEntry,
};
use crate::listing;
use crate::response::{PlexResponse, ResponseFormat};
use crate::PlexState;

pub fn router() -> Router<PlexState> {
    Router::new()
        .route("/library/sections", get(sections))
        // The spec declares only `/all`, while the web client calls the bare path. A real
        // server answers both with byte-identical responses, so they share a handler.
        .route("/library/sections/all", get(sections))
        .route("/library/sections/{sectionId}/all", get(section_items))
        .route("/library/metadata/{ids}", get(items_by_id))
        // Neither is in the spec; both are how a client opens a series.
        .route("/library/metadata/{id}/children", get(children::children))
        .route(
            "/library/metadata/{id}/grandchildren",
            get(children::grandchildren),
        )
        // Singular, and not in the spec in that form: the declared
        // `firstCharacters` 404s on a real server.
        .route(
            "/library/sections/{sectionId}/firstCharacter",
            get(first_character),
        )
        // Also absent from the spec, which declares only `/hubs/search`.
        .route("/search", get(search))
}

/// Page size when a client asks for a listing without saying how much of it it wants.
///
/// A reference server returns the whole library in that case. This one cannot — see
/// [`ItemsContainer`] — and matching the schema's own default keeps the two surfaces
/// consistent.
const DEFAULT_LISTING_SIZE: u32 = 50;

/// Everything `GET /library/sections` needs from the official API.
///
/// Written out as a constant rather than built at runtime: a static query is greppable,
/// reviewable, and cannot be shaped by client input.
const LIBRARIES_QUERY: &str = r"
query PlexSections($first: Int!, $after: String) {
  libraries(first: $first, after: $after) {
    edges {
      node {
        id
        kind
        title
        titleSort
        language
        uuid
        hidden
        createdAt
        updatedAt
        scannedAt
        contentChangedAt
        roots { id path createdAt }
      }
    }
    pageInfo { hasNextPage endCursor }
  }
}
";

/// How many libraries to pull per round trip.
///
/// `/library/sections` has no pagination of its own, so the adapter walks the whole
/// connection. The page size only trades round trips against memory.
const PAGE_SIZE: i32 = 100;

#[derive(Debug, Deserialize)]
struct LibrariesData {
    libraries: LibraryConnection,
}

#[derive(Debug, Deserialize)]
struct LibraryConnection {
    edges: Vec<LibraryEdge>,
    #[serde(rename = "pageInfo")]
    page_info: PageInfo,
}

#[derive(Debug, Deserialize)]
struct LibraryEdge {
    node: GqlLibrary,
}

#[derive(Debug, Deserialize)]
struct PageInfo {
    #[serde(rename = "hasNextPage")]
    has_next_page: bool,
    #[serde(rename = "endCursor")]
    end_cursor: Option<String>,
}

/// The adapter's view of a library, mirroring `LIBRARIES_QUERY`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlLibrary {
    /// The Relay global id. Plex exposes a bare number as `key`, so the adapter decodes it
    /// through the published codec — the official API publishes no numeric field of its own.
    pub id: String,
    pub kind: LibraryKind,
    pub title: String,
    pub title_sort: Option<String>,
    pub language: String,
    pub uuid: String,
    pub hidden: bool,
    pub created_at: i64,
    pub updated_at: i64,
    pub scanned_at: Option<i64>,
    /// A string in the schema because it exceeds GraphQL's 32-bit `Int`.
    pub content_changed_at: String,
    pub roots: Vec<GqlRoot>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlRoot {
    pub id: String,
    pub path: String,
    pub created_at: i64,
}

fn page_variables(after: Option<String>) -> Variables {
    let mut variables = Variables::default();
    variables.insert(
        async_graphql::Name::new("first"),
        async_graphql::Value::Number(PAGE_SIZE.into()),
    );
    variables.insert(
        async_graphql::Name::new("after"),
        after.map_or(async_graphql::Value::Null, async_graphql::Value::String),
    );
    variables
}

/// `GET /library/sections` and its `/all` alias — the entry point of navigation.
async fn sections(
    _caller: Caller,
    State(state): State<PlexState>,
    format: ResponseFormat,
) -> Result<PlexResponse<SectionsContainer>, ApiError> {
    let libraries = libraries(&state).await?;
    let body = SectionsContainer {
        size: u32::try_from(libraries.len()).unwrap_or(u32::MAX),
        allow_sync: false,
        title1: "Plex Library".to_owned(),
        directory: libraries
            .iter()
            .map(SectionDirectory::from_graphql)
            .collect(),
    };
    Ok(PlexResponse::new(body, format))
}

/// Every library, in the order the reference lists them: by media type, then by title.
///
/// Shared by `/library/sections` and `/media/providers`, which must list the same libraries
/// in the same order.
///
/// # Errors
///
/// Fails when the query fails, which is a defect in the schema rather than client input.
pub(crate) async fn libraries(state: &PlexState) -> Result<Vec<GqlLibrary>, ApiError> {
    let mut libraries = Vec::new();
    let mut after: Option<String> = None;

    // Walk the whole connection: capping at one page would silently drop libraries on a
    // server with more than `PAGE_SIZE` of them.
    loop {
        let data: LibrariesData =
            gql::execute(&state.schema, LIBRARIES_QUERY, page_variables(after)).await?;
        libraries.extend(data.libraries.edges.into_iter().map(|edge| edge.node));

        if !data.libraries.page_info.has_next_page {
            break;
        }
        after = data.libraries.page_info.end_cursor;
        if after.is_none() {
            // A connection reporting another page without a cursor would loop forever.
            break;
        }
    }

    // The reference server lists by media type then title, not by id. Ordering is a
    // presentation decision, so it belongs in the adapter rather than in the schema.
    libraries.sort_by(|left, right| {
        kind_rank(left.kind)
            .cmp(&kind_rank(right.kind))
            .then_with(|| left.title.cmp(&right.title))
    });
    Ok(libraries)
}

/// Item type as Plex numbers it on the wire.
///
/// A client sends `type=1`; the official API takes an enum. An unknown number places no
/// filter rather than failing: Plex numbers kinds this server does not hold — artists,
/// albums, photos — and a listing of everything is a better answer than a 400 to a client
/// probing what a library contains.
fn item_type(kind: Option<i32>) -> Option<&'static str> {
    match kind {
        Some(1) => Some("MOVIE"),
        Some(2) => Some("SHOW"),
        Some(3) => Some("SEASON"),
        Some(4) => Some("EPISODE"),
        _ => None,
    }
}

/// Translates Plex's `sort=` into the official API's ordering enum and direction.
///
/// Only the two orderings the schema offers are honored, and anything else falls back to
/// title. That bound is not arbitrary: both supported columns are non-null, which is what
/// lets a cursor locate a page boundary unambiguously. Ordering by a nullable column needs
/// the cursor to encode absence first.
fn ordering(sort: Option<&str>) -> (&'static str, &'static str) {
    let Some(sort) = sort else {
        return ("TITLE", "ASCENDING");
    };
    let (field, suffix) = sort.split_once(':').unwrap_or((sort, "asc"));
    let direction = if suffix.eq_ignore_ascii_case("desc") {
        "DESCENDING"
    } else {
        "ASCENDING"
    };
    let order = match field {
        "addedAt" | "added" => "ADDED_AT",
        _ => "TITLE",
    };
    (order, direction)
}

/// `GET /library/sections/{sectionId}/all` — the listing clients spend their time in.
///
/// Query pairs rather than a typed struct: the filter operator rides on the parameter *name*
/// (`year>=2020`), and the same field may appear twice, neither of which a serde struct can
/// express. See [`listing`].
async fn section_items(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(section): Path<i64>,
    Query(pairs): Query<Vec<(String, String)>>,
    format: ResponseFormat,
) -> Result<PlexResponse<ItemsContainer>, ApiError> {
    let request = listing::parse(&pairs)?;
    let offset = request.start;
    // Capped rather than honored as given: `MAX_ITEMS` is the ceiling every window in this
    // project respects, and a listing is where an uncapped count would hurt most.
    let first = request.size.unwrap_or(DEFAULT_LISTING_SIZE).min(MAX_ITEMS);
    let (order_by, direction) = ordering(request.sort.as_deref());

    let mut variables = Variables::default();
    variables.insert(
        Name::new("id"),
        Value::String(GlobalId::encode(LIBRARY_TYPE, section)),
    );
    variables.insert(Name::new("first"), Value::Number(first.into()));
    variables.insert(Name::new("offset"), Value::Number(offset.into()));
    variables.insert(Name::new("orderBy"), Value::Enum(Name::new(order_by)));
    variables.insert(Name::new("direction"), Value::Enum(Name::new(direction)));
    variables.insert(
        Name::new("filter"),
        request.filters.to_graphql(item_type(request.kind)),
    );

    let data: SectionItemsData =
        gql::execute(&state.schema, &SECTION_ITEMS_QUERY, variables).await?;
    // A null `node` means no library carries that id. 404 rather than an empty listing: a
    // client told "zero items" would cache an empty library.
    let library = data.node.ok_or(ApiError::NotFound)?;

    let items = library
        .items
        .edges
        .iter()
        .filter_map(|edge| MetadataEntry::from_graphql(&edge.node, false))
        .collect();
    let total = library.stats.item_count;
    let body = ItemsContainer::new(&library.as_ref(), items, offset, total);
    Ok(PlexResponse::new(body, format).paged(offset, total))
}

/// `GET /library/metadata/{ids}` — one or more items by identifier.
///
/// The path segment is a comma-separated list, which is how a client fetches a screenful of
/// items it already holds keys for.
async fn items_by_id(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(ids): Path<String>,
    format: ResponseFormat,
) -> Result<PlexResponse<ItemContainer>, ApiError> {
    let keys: Vec<i64> = ids
        .split(',')
        .filter_map(|key| key.trim().parse().ok())
        .collect();
    // An unparseable or oversized list is a client mistake, and 404 is what a reference
    // server answers for a key that names nothing.
    if keys.is_empty() || keys.len() > MAX_ITEMS as usize {
        return Err(ApiError::NotFound);
    }

    let mut variables = Variables::default();
    variables.insert(
        Name::new("ids"),
        Value::List(
            keys.iter()
                .map(|key| Value::String(GlobalId::encode(ITEM_TYPE, *key)))
                .collect(),
        ),
    );

    let data: ItemsByIdData = gql::execute(&state.schema, &ITEMS_BY_ID_QUERY, variables).await?;
    let found: Vec<_> = data.items.into_iter().flatten().collect();
    // Every id missing is a 404; some missing is simply a shorter list, which is what a
    // reference server returns when one of several keys has been removed.
    if found.is_empty() {
        return Err(ApiError::NotFound);
    }

    let library = found.first().and_then(|item| item.library.clone());
    let items = found
        .iter()
        .filter_map(|item| MetadataEntry::from_graphql(item, true))
        .collect();
    let body = ItemContainer::new(items, library.as_ref());
    Ok(PlexResponse::new(body, format))
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    #[test]
    fn maps_plex_type_numbers_to_the_schema_enum() {
        assert_eq!(item_type(Some(1)), Some("MOVIE"));
        assert_eq!(item_type(Some(2)), Some("SHOW"));
        assert_eq!(item_type(Some(3)), Some("SEASON"));
        assert_eq!(item_type(Some(4)), Some("EPISODE"));
        // 8 is an artist, which this server does not hold: no filter rather than an error.
        assert_eq!(item_type(Some(8)), None);
        assert_eq!(item_type(None), None);
    }

    #[test]
    fn reads_the_direction_suffix_off_the_sort_field() {
        assert_eq!(ordering(None), ("TITLE", "ASCENDING"));
        assert_eq!(ordering(Some("titleSort")), ("TITLE", "ASCENDING"));
        assert_eq!(ordering(Some("titleSort:desc")), ("TITLE", "DESCENDING"));
        assert_eq!(ordering(Some("addedAt:desc")), ("ADDED_AT", "DESCENDING"));
        assert_eq!(ordering(Some("addedAt")), ("ADDED_AT", "ASCENDING"));
        // An ordering the schema does not offer falls back rather than failing.
        assert_eq!(ordering(Some("rating:desc")), ("TITLE", "DESCENDING"));
    }
}

/// Everything `GET /library/sections/{id}/firstCharacter` needs from the official API.
const FIRST_CHARACTERS_QUERY: &str = r"
query PlexFirstCharacters($id: ID!, $filter: ItemFilterInput) {
  node(id: $id) {
    ... on Library {
      id
      title
      uuid
      kind
      stats(filter: $filter) { firstCharacters { character count } }
    }
  }
}
";

#[derive(Debug, Deserialize)]
struct FirstCharactersData {
    node: Option<GqlFirstCharacters>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlFirstCharacters {
    id: String,
    title: String,
    uuid: String,
    kind: String,
    stats: GqlCharacterStats,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlCharacterStats {
    first_characters: Vec<GqlCharacter>,
}

#[derive(Debug, Deserialize)]
struct GqlCharacter {
    character: String,
    count: u64,
}

/// `GET /library/sections/{sectionId}/firstCharacter` — the A–Z jump bar.
///
/// Singular, and absent from the spec in that form: the spec declares
/// `firstCharacters`, which 404s on a real server. See `docs/spec-gaps.md`.
///
/// `type=` is required by the reference, which 404s without it. We accept its absence and
/// bucket the library's top level, because answering the obvious question beats refusing it.
async fn first_character(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(section): Path<i64>,
    Query(pairs): Query<Vec<(String, String)>>,
    format: ResponseFormat,
) -> Result<PlexResponse<CharactersContainer>, ApiError> {
    let request = listing::parse(&pairs)?;

    let mut variables = Variables::default();
    variables.insert(
        Name::new("id"),
        Value::String(GlobalId::encode(LIBRARY_TYPE, section)),
    );
    variables.insert(
        Name::new("filter"),
        request.filters.to_graphql(item_type(request.kind)),
    );

    let data: FirstCharactersData =
        gql::execute(&state.schema, FIRST_CHARACTERS_QUERY, variables).await?;
    let library = data.node.ok_or(ApiError::NotFound)?;

    let body = CharactersContainer::new(
        &GqlLibraryRef {
            id: library.id,
            title: library.title,
            uuid: library.uuid,
            kind: library.kind,
        },
        library
            .stats
            .first_characters
            .into_iter()
            .map(|bucket| CharacterDirectory::new(&bucket.character, bucket.count))
            .collect(),
    );
    Ok(PlexResponse::new(body, format))
}

/// Everything `GET /search` needs from the official API.
const SEARCH_QUERY: &str = r"
query PlexSearch($query: String!, $first: Int!) {
  search(query: $query, first: $first) {
    edges {
      node {
        ...Shared
        ... on Movie {
          originalTitle
          year
          releasedOn
          studio
          tagline
          contentRating
          contentRatingAge
          rating
          audienceRating
          durationMs
          versions { ...VersionFields files { id path sizeBytes durationMs } }
        }
        ... on Show {
          number
          originalTitle
          year
          releasedOn
          studio
          tagline
          contentRating
          contentRatingAge
          audienceRating
          durationMs
          showStats: stats { seasonCount episodeCount }
        }
        ... on Season { ...SeasonFields }
        ... on Episode {
          ...EpisodeFields
          versions { ...VersionFields files { id path sizeBytes durationMs } }
        }
      }
    }
  }
}
";

/// The search query with the shared item fragments appended.
static SEARCH_QUERY_WITH_FRAGMENTS: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| with_shared_fragments(SEARCH_QUERY));

/// How many results `GET /search` returns.
///
/// The endpoint takes no container parameters on the reference server; it answers one page and
/// expects the client to narrow the term. 50 is what a search box shows.
const SEARCH_RESULTS: u32 = 50;

#[derive(Debug, Deserialize)]
struct SearchData {
    search: SearchConnection,
}

#[derive(Debug, Deserialize)]
struct SearchConnection {
    edges: Vec<GqlItemEdge>,
}

/// What a client sends to `GET /search`.
#[derive(Debug, Default, Deserialize)]
pub struct SearchParams {
    query: Option<String>,
}

/// `GET /search` — one search box across every library.
///
/// Absent from the spec, which declares only `/hubs/search`; the reference answers it, and it
/// is what a client's search field calls. See `docs/spec-gaps.md`.
async fn search(
    _caller: Caller,
    State(state): State<PlexState>,
    Query(params): Query<SearchParams>,
    format: ResponseFormat,
) -> Result<PlexResponse<SearchContainer>, ApiError> {
    // A missing term is an empty result, not an error: that is what a client gets while the
    // user is still typing, and the schema treats a blank term the same way.
    let term = params.query.unwrap_or_default();

    let mut variables = Variables::default();
    variables.insert(Name::new("query"), Value::String(term));
    variables.insert(
        Name::new("first"),
        Value::Number(SEARCH_RESULTS.min(MAX_ITEMS).into()),
    );

    let data: SearchData =
        gql::execute(&state.schema, &SEARCH_QUERY_WITH_FRAGMENTS, variables).await?;
    let items = data
        .search
        .edges
        .iter()
        .filter_map(|edge| SearchEntry::from_graphql(&edge.node, &state.identity.friendly_name))
        .collect();

    Ok(PlexResponse::new(SearchContainer::new(items), format))
}
