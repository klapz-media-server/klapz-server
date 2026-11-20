//! Library navigation, served by translating GraphQL results into `MediaContainer`s.

use async_graphql::{Name, Value, Variables};
use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::Router;
use klapz_api::item::ITEM_TYPE;
use klapz_api::library::LIBRARY_TYPE;
use klapz_api::GlobalId;
use klapz_core::page::MAX_ITEMS;
use serde::{Deserialize, Serialize};

use crate::auth::Caller;
use crate::children;
use crate::client::ClientIdentity;
use crate::container::{kind_rank, LibraryKind, SectionDirectory, SectionsContainer};
use crate::error::ApiError;
use crate::gql;
use crate::gql_items::{
    with_shared_fragments, GqlItemEdge, GqlLibraryItems, ItemsByIdData, SectionItemsData,
    ITEMS_BY_ID_QUERY, SECTION_ITEMS_QUERY,
};
use crate::gql_items::{GqlItem, GqlLibraryRef};
use crate::items::{
    CharacterDirectory, CharactersContainer, ItemsContainer, MetadataEntry, SearchContainer,
    SearchEntry, ViewStyle, GRID_VIEW_MODE, LIBRARY_IDENTIFIER, MEDIA_TAG_PREFIX,
    MEDIA_TAG_VERSION,
};
use crate::language::Language;
use crate::listing::{self, ListingRequest};
use crate::response::{PlexResponse, ResponseFormat};
use crate::PlexState;

pub fn router() -> Router<PlexState> {
    Router::new()
        .route("/library/sections", get(sections))
        // The spec declares only `/all`, while the web client calls the bare path. A real
        // server answers both with byte-identical responses, so they share a handler.
        .route("/library/sections/all", get(sections))
        .route(
            "/library/sections/{sectionId}/all",
            get(section_items).put(crate::edit::edit_items),
        )
        .route(
            "/library/metadata/{ids}",
            get(items_by_id)
                .put(crate::edit::edit_metadata)
                .delete(crate::collections::delete_metadata),
        )
        // Neither is in the spec; both are how a client opens a series.
        .route("/library/metadata/{id}/children", get(children::children))
        .route(
            "/library/metadata/{id}/allLeaves",
            get(children::all_leaves),
        )
        .route(
            "/library/metadata/{id}/grandchildren",
            get(children::grandchildren),
        )
        // Singular, and not in the spec in that form: the declared
        // `firstCharacters` 404s on a real server.
        .route(
            "/library/sections/{sectionId}/allLeaves",
            get(section_leaves),
        )
        .route(
            "/library/sections/{sectionId}/firstCharacter",
            get(first_character),
        )
        .route(
            "/library/sections/{sectionId}/filters",
            get(crate::menus::filters),
        )
        .route(
            "/library/sections/{sectionId}/sorts",
            get(crate::menus::sorts),
        )
        .route(
            "/library/sections/{sectionId}/categories",
            get(crate::categories::categories),
        )
        .route(
            "/library/sections/{sectionId}/common",
            get(crate::common::common),
        )
        // The choices of each filter `includeMeta` advertises. Static segments such as
        // `/all` take precedence over the parameter, so this only sees the rest.
        .route(
            "/library/sections/{sectionId}/{filter}",
            get(crate::filter_values::filter_values),
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
    #[serde(deserialize_with = "crate::gql::date_time::seconds")]
    pub created_at: i64,
    #[serde(deserialize_with = "crate::gql::date_time::seconds")]
    pub updated_at: i64,
    #[serde(default, deserialize_with = "crate::gql::date_time::optional_seconds")]
    pub scanned_at: Option<i64>,
    #[serde(deserialize_with = "klapz_api::int64::deserialize")]
    pub content_changed_at: i64,
    pub roots: Vec<GqlRoot>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlRoot {
    pub id: String,
    pub path: String,
    #[serde(deserialize_with = "crate::gql::date_time::seconds")]
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
/// A client sends `type=1`; the official API takes an enum. The music kinds go through
/// too, and the official API answers them with nothing, as the reference answers a library
/// that holds none: an empty container, not every item. Any other number places no filter
/// rather than failing: a listing of everything is a better answer than a 400 to a client
/// probing what a library contains.
pub(crate) fn item_type(kind: Option<i32>) -> Option<&'static str> {
    match kind {
        Some(1) => Some("MOVIE"),
        Some(2) => Some("SHOW"),
        Some(3) => Some("SEASON"),
        Some(4) => Some("EPISODE"),
        Some(8) => Some("ARTIST"),
        Some(9) => Some("ALBUM"),
        Some(10) => Some("TRACK"),
        _ => None,
    }
}

/// How long a shuffle lasts, in seconds.
///
/// The reference reshuffles on every request, so a client paging through a random listing
/// meets some items twice and never sees others. One seed per period keeps the pages of one
/// browse in step, and a later visit still finds a new order.
const SHUFFLE_PERIOD_SECONDS: i64 = 600;

/// The seed of the shuffle in effect.
pub(crate) fn shuffle_seed() -> u32 {
    u32::try_from(klapz_core::now_epoch() / SHUFFLE_PERIOD_SECONDS).unwrap_or(0)
}

/// `GET /library/sections/{sectionId}/all` — the listing clients spend their time in.
///
/// Query pairs rather than a typed struct: the filter operator rides on the parameter *name*
/// (`year>=2020`), and the same field may appear twice, neither of which a serde struct can
/// express. See [`listing`].
async fn section_items(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    Path(section): Path<i64>,
    Query(pairs): Query<Vec<(String, String)>>,
    format: ResponseFormat,
) -> Result<axum::response::Response, ApiError> {
    use axum::response::IntoResponse as _;
    // A collection is a type of its own on the reference, listed in the same container.
    if pairs
        .iter()
        .any(|(key, value)| key == "type" && value == "18")
    {
        return Ok(crate::collections::listing(&state, section, &pairs, format)
            .await?
            .into_response());
    }
    let language = Language::of(&client);
    let (library, request) = section_page(&state, section, &pairs, Depth::Library).await?;
    let offset = request.start;
    let items = entries(&library, language);
    let total = library.stats.item_count;
    let mut body = ItemsContainer::new(&library.as_ref(), items, offset, total);
    if pairs
        .iter()
        .any(|(key, value)| key == "includeMeta" && value == "1")
    {
        body.meta =
            crate::meta::build(section, &library.kind, request.kind, request.sort, language);
    }
    Ok(PlexResponse::new(body, format)
        .paged(offset, total)
        .into_response())
}

/// `GET /library/sections/{sectionId}/allLeaves` — everything in a library that has nothing
/// below it: its films, or its episodes.
///
/// The reference answers it as `/all?type=` the leaves' type, measured identical in items,
/// order and totals on 92 requests over four libraries, filters and sorts included; a `type`
/// naming anything else finds nothing, a collection's 18 included. Only the container differs:
/// no `librarySection*` trio, no `title2`, `allowSync` off, and no `Meta` whatever
/// `includeMeta` says.
async fn section_leaves(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    Path(section): Path<i64>,
    Query(pairs): Query<Vec<(String, String)>>,
    format: ResponseFormat,
) -> Result<PlexResponse<LeavesContainer>, ApiError> {
    let (library, request) = section_page(&state, section, &pairs, Depth::Leaves).await?;
    let offset = request.start;
    // A type number naming no kind the official API has names none of the leaves: the
    // reference lists nothing for 0, 13 or 99, where `/all` here lists everything.
    let names_nothing = request.kind.is_some() && item_type(request.kind).is_none();
    let (items, total) = if names_nothing {
        (Vec::new(), 0)
    } else {
        (
            entries(&library, Language::of(&client)),
            library.stats.item_count,
        )
    };
    let style = ViewStyle::for_kind(&library.kind);
    let body = LeavesContainer {
        size: u32::try_from(items.len()).unwrap_or(u32::MAX),
        total_size: total,
        offset,
        allow_sync: false,
        art: style.art,
        identifier: LIBRARY_IDENTIFIER,
        media_tag_prefix: MEDIA_TAG_PREFIX,
        media_tag_version: MEDIA_TAG_VERSION,
        nocache: (library.kind == "SHOW").then_some(true),
        thumb: style.thumb,
        title1: library.title,
        view_group: style.view_group,
        view_mode: GRID_VIEW_MODE,
        metadata: items,
    };
    Ok(PlexResponse::new(body, format).paged(offset, total))
}

/// Response body for `GET /library/sections/{id}/allLeaves`: `/all`'s container without the
/// library's identity or the view's title. `nocache` is a series library's only, as on `/all`
/// there. Windowed always, as every listing here is.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeavesContainer {
    pub size: u32,
    pub total_size: u64,
    pub offset: u64,
    pub allow_sync: bool,
    pub art: &'static str,
    pub identifier: &'static str,
    pub media_tag_prefix: &'static str,
    pub media_tag_version: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nocache: Option<bool>,
    pub thumb: &'static str,
    pub title1: String,
    pub view_group: &'static str,
    pub view_mode: u32,
    #[serde(rename = "Metadata", skip_serializing_if = "Vec::is_empty")]
    pub metadata: Vec<MetadataEntry>,
}

/// Which part of a library a listing reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Depth {
    /// `/all`: the top level, or the kind `type` names.
    Library,
    /// `/allLeaves`: the films or the episodes, narrowed further by `type`.
    Leaves,
}

/// One window of a library's items, as the request's container parameters, filters and sort
/// ask for it.
async fn section_page(
    state: &PlexState,
    section: i64,
    pairs: &[(String, String)],
    depth: Depth,
) -> Result<(GqlLibraryItems, ListingRequest), ApiError> {
    let request = listing::parse(pairs)?;
    // Capped rather than honored as given: `MAX_ITEMS` is the ceiling every window in this
    // project respects, and a listing is where an uncapped count would hurt most.
    let first = request.size.unwrap_or(DEFAULT_LISTING_SIZE).min(MAX_ITEMS);
    let (order_by, direction) = match depth {
        Depth::Library => request.ordering(),
        Depth::Leaves => request.leaves_ordering(),
    };
    let mut filter = request.filters.to_graphql(item_type(request.kind));
    if depth == Depth::Leaves {
        let mut fields = match filter {
            Value::Object(fields) => fields,
            _ => async_graphql::indexmap::IndexMap::new(),
        };
        fields.insert(Name::new("depth"), Value::Enum(Name::new("LEAVES")));
        filter = Value::Object(fields);
    }

    let mut variables = Variables::default();
    variables.insert(
        Name::new("id"),
        Value::String(GlobalId::encode(LIBRARY_TYPE, section)),
    );
    variables.insert(Name::new("first"), Value::Number(first.into()));
    variables.insert(Name::new("offset"), Value::Number(request.start.into()));
    variables.insert(Name::new("orderBy"), Value::Enum(Name::new(order_by)));
    variables.insert(Name::new("direction"), Value::Enum(Name::new(direction)));
    variables.insert(Name::new("seed"), Value::Number(shuffle_seed().into()));
    variables.insert(Name::new("filter"), filter);
    variables.insert(
        Name::new("markers"),
        Value::Boolean(crate::markers::requested(pairs)),
    );

    let data: SectionItemsData =
        gql::execute(&state.schema, &SECTION_ITEMS_QUERY, variables).await?;
    // A null `node` means no library carries that id. 404 rather than an empty listing: a
    // client told "zero items" would cache an empty library.
    let library = data.node.ok_or(ApiError::NotFound)?;
    Ok((library, request))
}

/// The page's items as a listing publishes them: the library is stated once, on the
/// container.
fn entries(library: &GqlLibraryItems, language: Language) -> Vec<MetadataEntry> {
    library
        .items
        .edges
        .iter()
        .filter_map(|edge| MetadataEntry::from_graphql(&edge.node, false, language))
        .collect()
}

/// `GET /library/metadata/{ids}` — one or more items by identifier.
///
/// The path segment is a comma-separated list, which is how a client fetches a screenful of
/// items it already holds keys for.
async fn items_by_id(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    Path(ids): Path<String>,
    Query(pairs): Query<Vec<(String, String)>>,
    format: ResponseFormat,
) -> Result<PlexResponse<serde_json::Value>, ApiError> {
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
    variables.insert(
        Name::new("markers"),
        Value::Boolean(crate::markers::requested(&pairs)),
    );
    variables.insert(
        Name::new("chapters"),
        Value::Boolean(
            pairs
                .iter()
                .any(|(key, value)| key == "includeChapters" && value == "1"),
        ),
    );
    variables.insert(
        Name::new("extras"),
        Value::Boolean(crate::extras::requested(&pairs)),
    );

    let data: ItemsByIdData = gql::execute(&state.schema, &ITEMS_BY_ID_QUERY, variables).await?;
    // A number no item carries may be a collection's: the two share the numbering.
    let missing: Vec<i64> = keys
        .iter()
        .zip(&data.items)
        .filter_map(|(key, item)| item.is_none().then_some(*key))
        .collect();
    let collections = if missing.is_empty() {
        Vec::new()
    } else {
        crate::collections::lookups(&state, &missing).await?
    };
    let found: Vec<(i64, GqlItem)> = keys
        .iter()
        .copied()
        .zip(data.items)
        .filter_map(|(key, item)| Some((key, item?)))
        .collect();
    // Every id missing is a 404; some missing is simply a shorter list, which is what a
    // reference server returns when one of several keys has been removed.
    if found.is_empty() && collections.is_empty() {
        return Err(ApiError::NotFound);
    }

    let library = found
        .first()
        .and_then(|(_, item)| item.library.clone())
        .or_else(|| {
            collections
                .first()
                .and_then(|(_, _, library)| library.clone())
        });
    let mut entries: Vec<(i64, serde_json::Value)> = found
        .iter()
        .filter_map(|(key, item)| {
            let entry = MetadataEntry::from_graphql(item, true, Language::of(&client))?;
            Some((*key, serde_json::to_value(entry).ok()?))
        })
        .chain(collections.into_iter().map(|(key, entry, _)| (key, entry)))
        .collect();
    // In the order asked, as the reference answers a list mixing the two.
    entries.sort_by_key(|(key, _)| keys.iter().position(|asked| asked == key));
    let body = crate::collections::lookup_container(
        entries.into_iter().map(|(_, entry)| entry).collect(),
        library.as_ref(),
    )?;
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
        assert_eq!(item_type(Some(10)), Some("TRACK"));
        // 13 is a photo, which the official API has no kind for: no filter, not an error.
        assert_eq!(item_type(Some(13)), None);
        assert_eq!(item_type(None), None);
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
query PlexSearch($query: String!, $kinds: [ItemType!], $first: Int!) {
  search(query: $query, kinds: $kinds, order: TITLE, first: $first) {
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

/// How many results of each kind `GET /search` returns: films, then series, then episodes,
/// each in title order.
///
/// Measured on 53 terms: never more than 15 of a kind, and the 15 a title-ordered match
/// begins with. The reference takes no container parameters here; it answers one page and
/// expects the client to narrow the term.
const SEARCH_RESULTS_PER_KIND: u32 = 15;

/// The kinds `GET /search` lists with no `type`, in the order it lists them.
const SEARCH_KINDS: [&str; 3] = ["MOVIE", "SHOW", "EPISODE"];

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
    #[serde(rename = "type")]
    kind: Option<String>,
}

/// `GET /search` — one search box across every library.
///
/// Absent from the spec, which declares only `/hubs/search`; the reference answers it, and it
/// is what a client's search field calls. See `docs/spec-gaps.md`.
async fn search(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    Query(params): Query<SearchParams>,
    format: ResponseFormat,
) -> Result<PlexResponse<SearchContainer>, ApiError> {
    let language = Language::of(&client);
    // A missing term is an empty result, not an error: that is what a client gets while the
    // user is still typing, and the schema treats a blank term the same way.
    let term = params.query.unwrap_or_default();

    let kinds: Vec<&str> = match &params.kind {
        // A type the official API has no kind for finds nothing, as on the reference
        // (`type=99`); a music type finds nothing there too, since none is held.
        Some(kind) => match item_type(kind.parse().ok()) {
            Some(kind) => vec![kind],
            None => {
                return Ok(PlexResponse::new(
                    SearchContainer::of_one_kind(Vec::new()),
                    format,
                ))
            }
        },
        None => SEARCH_KINDS.to_vec(),
    };

    let mut items = Vec::new();
    for kind in kinds {
        let mut variables = Variables::default();
        variables.insert(Name::new("query"), Value::String(term.clone()));
        variables.insert(
            Name::new("kinds"),
            Value::List(vec![Value::Enum(Name::new(kind))]),
        );
        variables.insert(
            Name::new("first"),
            Value::Number(SEARCH_RESULTS_PER_KIND.into()),
        );
        let data: SearchData =
            gql::execute(&state.schema, &SEARCH_QUERY_WITH_FRAGMENTS, variables).await?;
        items.extend(data.search.edges.iter().filter_map(|edge| {
            SearchEntry::from_graphql(&edge.node, &state.identity.friendly_name, language)
        }));
    }

    let body = if params.kind.is_some() {
        SearchContainer::of_one_kind(items)
    } else {
        SearchContainer::new(items)
    };
    Ok(PlexResponse::new(body, format))
}
