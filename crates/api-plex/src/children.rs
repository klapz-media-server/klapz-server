//! `GET /library/metadata/{id}/children` and `/grandchildren` — browsing into a series.
//!
//! Neither is in the spec, and both are how every client opens a series: the seasons of a
//! show, the episodes of a season, and the episodes of a show as one list. See
//! `docs/spec-gaps.md`.
//!
//! The two containers differ more than the items in them. `/children` describes the parent
//! being opened — its title, year, synopsis, and for a season the series above it — because
//! the client renders that header from the same response. `/grandchildren` describes
//! nothing but the library: a reference server sends it as a flat list.

use async_graphql::{Name, Value, Variables};
use axum::extract::{Path, Query, State};
use open_plex_api::item::ITEM_TYPE;
use open_plex_api::GlobalId;
use open_plex_core::page::MAX_ITEMS;
use serde::{Deserialize, Serialize};

use crate::auth::Caller;
use crate::error::ApiError;
use crate::gql;
use crate::gql_items::{
    season_title, with_shared_fragments, GqlItemConnection, GqlLibraryRef, GqlParentRef,
};
use crate::items::{
    MetadataEntry, ViewStyle, LIBRARY_IDENTIFIER, MEDIA_TAG_PREFIX, MEDIA_TAG_VERSION,
};
use crate::listing;
use crate::response::{PlexResponse, ResponseFormat};
use crate::PlexState;

/// Page size when a client opens a series without saying how much of it it wants.
///
/// A reference server returns every child in that case. This one always windows — the
/// deviation `ItemsContainer` documents — and matches the schema's own default.
const DEFAULT_PAGE: u32 = 50;

/// Layout the web client restores for a list of seasons. Opaque to the server, and one more
/// than the grid every other listing reports.
const SEASON_VIEW_MODE: u32 = 65_593;

/// Everything `/children` needs: the parent, its counts, and one page below it.
///
/// Both kinds that can be opened are selected under one alias, `children`, so the response
/// deserializes into one shape whichever the node turns out to be. A film or an episode
/// matches neither fragment and comes back as its type name alone, which is what the
/// handler needs to refuse it.
static CHILDREN_QUERY: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    with_shared_fragments(
        r"
query PlexChildren($id: ID!, $first: Int!, $offset: Int!) {
  node(id: $id) {
    __typename
    ... on Show {
      id
      title
      number
      year
      summary
      library { id title uuid kind }
      stats { seasonCount }
      children: seasons(first: $first, offset: $offset) {
        edges { node { ...Shared ... on Season { ...SeasonFields } } }
      }
    }
    ... on Season {
      id
      title
      number
      year
      show { id title guid studio number year contentRating }
      library { id title uuid kind }
      stats { episodeCount }
      children: episodes(first: $first, offset: $offset) {
        edges {
          node {
            ...Shared
            ... on Episode {
              ...EpisodeFields
              versions { ...VersionFields files { id path sizeBytes durationMs } }
            }
          }
        }
      }
    }
  }
}
",
    )
});

/// Everything `/grandchildren` needs: the library, and the leaves below the node.
///
/// A film and an episode select their library too, because the reference answers them
/// with an empty container rather than an error, and that container still names the
/// library.
static GRANDCHILDREN_QUERY: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    with_shared_fragments(
        r"
query PlexGrandchildren($id: ID!, $first: Int!, $offset: Int!) {
  node(id: $id) {
    __typename
    ... on Show {
      library { id title uuid kind }
      stats { episodeCount }
      children: episodes(first: $first, offset: $offset) {
        edges {
          node {
            ...Shared
            ... on Episode {
              ...EpisodeFields
              versions { ...VersionFields files { id path sizeBytes durationMs } }
            }
          }
        }
      }
    }
    ... on Season {
      library { id title uuid kind }
      stats { episodeCount }
      children: episodes(first: $first, offset: $offset) {
        edges {
          node {
            ...Shared
            ... on Episode {
              ...EpisodeFields
              versions { ...VersionFields files { id path sizeBytes durationMs } }
            }
          }
        }
      }
    }
    ... on Movie { library { id title uuid kind } }
    ... on Episode { library { id title uuid kind } }
  }
}
",
    )
});

#[derive(Debug, Deserialize)]
struct ParentData {
    node: Option<GqlParent>,
}

/// The node being opened, whichever kind it is.
///
/// One struct for all four kinds, defaulted: the fragments select different subsets, and a
/// field no fragment asked for is simply absent.
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct GqlParent {
    #[serde(rename = "__typename")]
    typename: String,
    id: String,
    title: Option<String>,
    number: Option<i32>,
    year: Option<i32>,
    summary: Option<String>,
    show: Option<GqlParentRef>,
    library: Option<GqlLibraryRef>,
    stats: GqlCounts,
    children: Option<GqlItemConnection>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct GqlCounts {
    season_count: Option<u64>,
    episode_count: Option<u64>,
}

impl GqlParent {
    fn database_id(&self) -> i64 {
        GlobalId::decode(&self.id).map_or(0, |decoded| decoded.key)
    }

    /// Whether anything can be listed below this node.
    fn is_branch(&self) -> bool {
        matches!(self.typename.as_str(), "Show" | "Season")
    }

    /// How many rows the page is a window over.
    fn total(&self) -> u64 {
        self.stats
            .season_count
            .or(self.stats.episode_count)
            .unwrap_or(0)
    }

    fn entries(&self) -> Vec<MetadataEntry> {
        self.children
            .as_ref()
            .map(|connection| {
                connection
                    .edges
                    .iter()
                    // A listing: the library is stated once on the container.
                    .filter_map(|edge| MetadataEntry::from_graphql(&edge.node, false))
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Response body for `GET /library/metadata/{id}/children`.
///
/// Attributes are alphabetical after the paging trio, which is how a reference server
/// writes this container. The `grandparent*` group is present only below a season, where
/// the series is two levels above the episodes listed.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildrenContainer {
    pub size: u32,
    pub total_size: u64,
    pub offset: u64,
    pub allow_sync: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grandparent_content_rating: Option<String>,
    /// A number here, where every item's `ratingKey` is a string. The reference's own
    /// inconsistency, reproduced.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grandparent_rating_key: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grandparent_studio: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grandparent_title: Option<String>,
    pub identifier: &'static str,
    /// The parent's own key, bare: no `/library/metadata/` prefix on this one.
    pub key: String,
    #[serde(rename = "librarySectionID")]
    pub library_section_id: i64,
    pub library_section_title: String,
    #[serde(rename = "librarySectionUUID")]
    pub library_section_uuid: String,
    pub media_tag_prefix: &'static str,
    pub media_tag_version: i64,
    /// Always set on this container. Opening a series is the one navigation step the
    /// reference asks clients not to cache, presumably because watch state changes it.
    pub nocache: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_index: Option<i32>,
    /// The parent's *stored* title — empty for a season named at read time, while `title2`
    /// carries the display form. Both as the reference sends them.
    pub parent_title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_year: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub title1: String,
    pub title2: String,
    pub view_group: &'static str,
    pub view_mode: u32,
    #[serde(rename = "Metadata", skip_serializing_if = "Vec::is_empty")]
    pub metadata: Vec<MetadataEntry>,
}

/// Response body for `GET /library/metadata/{id}/grandchildren`.
///
/// A flat list: nothing about the series, only the library. `nocache` appears only where
/// the node can have leaves; a film's grandchildren is an empty container without it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeavesContainer {
    pub size: u32,
    pub total_size: u64,
    pub offset: u64,
    pub art: &'static str,
    pub identifier: &'static str,
    pub media_tag_prefix: &'static str,
    pub media_tag_version: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nocache: Option<bool>,
    pub title1: String,
    pub view_group: &'static str,
    pub view_mode: u32,
    #[serde(rename = "Metadata", skip_serializing_if = "Vec::is_empty")]
    pub metadata: Vec<MetadataEntry>,
}

/// Runs one of the two queries for the node, with the client's window.
async fn open(
    state: &PlexState,
    query: &str,
    id: i64,
    pairs: &[(String, String)],
) -> Result<(GqlParent, u64), ApiError> {
    // Parsed as a listing so the container window is read the same way everywhere, and so
    // a filter the reference would honor here is refused rather than dropped.
    let request = listing::parse(pairs)?;
    let first = request.size.unwrap_or(DEFAULT_PAGE).min(MAX_ITEMS);

    let mut variables = Variables::default();
    variables.insert(
        Name::new("id"),
        Value::String(GlobalId::encode(ITEM_TYPE, id)),
    );
    variables.insert(Name::new("first"), Value::Number(first.into()));
    variables.insert(Name::new("offset"), Value::Number(request.start.into()));

    let data: ParentData = gql::execute(&state.schema, query, variables).await?;
    let parent = data.node.ok_or(ApiError::NotFound)?;
    Ok((parent, request.start))
}

/// `GET /library/metadata/{id}/children` — the seasons of a series, the episodes of a season.
pub(crate) async fn children(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(id): Path<i64>,
    Query(pairs): Query<Vec<(String, String)>>,
    format: ResponseFormat,
) -> Result<PlexResponse<ChildrenContainer>, ApiError> {
    let (parent, offset) = open(&state, &CHILDREN_QUERY, id, &pairs).await?;
    // A film or an episode has nothing below it, and a reference server says so with a
    // 400 rather than an empty list.
    if !parent.is_branch() {
        return Err(ApiError::BadRequest);
    }
    let library = parent.library.as_ref().ok_or(ApiError::NotFound)?;
    let items = parent.entries();
    let total = parent.total();

    let body = match parent.typename.as_str() {
        "Season" => {
            let show = parent.show.as_ref();
            ChildrenContainer {
                size: u32::try_from(items.len()).unwrap_or(u32::MAX),
                total_size: total,
                offset,
                allow_sync: true,
                grandparent_content_rating: show.and_then(|show| show.content_rating.clone()),
                grandparent_rating_key: show.map(GqlParentRef::database_id),
                grandparent_studio: show.and_then(|show| show.studio.clone()),
                grandparent_title: show.and_then(|show| show.title.clone()),
                identifier: LIBRARY_IDENTIFIER,
                key: parent.database_id().to_string(),
                library_section_id: library.database_id(),
                library_section_title: library.title.clone(),
                library_section_uuid: library.uuid.clone(),
                media_tag_prefix: MEDIA_TAG_PREFIX,
                media_tag_version: MEDIA_TAG_VERSION,
                nocache: true,
                parent_index: parent.number,
                parent_title: parent.title.clone().unwrap_or_default(),
                // The season's year, or the series' when the season has none — the same
                // fallback the season reports as an item.
                parent_year: parent.year.or(show.and_then(|show| show.year)),
                summary: None,
                title1: show.and_then(|show| show.title.clone()).unwrap_or_default(),
                title2: season_title(parent.title.as_deref(), parent.number),
                view_group: "episode",
                view_mode: crate::items::GRID_VIEW_MODE,
                metadata: items,
            }
        }
        _ => ChildrenContainer {
            size: u32::try_from(items.len()).unwrap_or(u32::MAX),
            total_size: total,
            offset,
            allow_sync: true,
            grandparent_content_rating: None,
            grandparent_rating_key: None,
            grandparent_studio: None,
            grandparent_title: None,
            identifier: LIBRARY_IDENTIFIER,
            key: parent.database_id().to_string(),
            library_section_id: library.database_id(),
            library_section_title: library.title.clone(),
            library_section_uuid: library.uuid.clone(),
            media_tag_prefix: MEDIA_TAG_PREFIX,
            media_tag_version: MEDIA_TAG_VERSION,
            nocache: true,
            parent_index: parent.number,
            parent_title: parent.title.clone().unwrap_or_default(),
            parent_year: parent.year,
            // Omitted when empty, unlike the season item's own `summary`, which the
            // reference sends as an empty string. Different element, different rule.
            summary: parent.summary.clone().filter(|text| !text.is_empty()),
            title1: library.title.clone(),
            title2: parent.title.clone().unwrap_or_default(),
            view_group: "season",
            view_mode: SEASON_VIEW_MODE,
            metadata: items,
        },
    };
    Ok(PlexResponse::new(body, format).paged(offset, total))
}

/// `GET /library/metadata/{id}/grandchildren` — every episode below a series or a season.
///
/// Below a season this lists the same episodes as `/children`; the reference does the same,
/// reading it as "the leaves" rather than literally two levels down. A film or an episode
/// has no leaves and answers an empty container, not an error — also the reference's call.
pub(crate) async fn grandchildren(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(id): Path<i64>,
    Query(pairs): Query<Vec<(String, String)>>,
    format: ResponseFormat,
) -> Result<PlexResponse<LeavesContainer>, ApiError> {
    let (parent, offset) = open(&state, &GRANDCHILDREN_QUERY, id, &pairs).await?;
    let library = parent.library.as_ref().ok_or(ApiError::NotFound)?;
    let style = ViewStyle::for_kind(&library.kind);
    let items = parent.entries();
    let total = parent.total();

    let body = LeavesContainer {
        size: u32::try_from(items.len()).unwrap_or(u32::MAX),
        total_size: total,
        offset,
        art: style.art,
        identifier: LIBRARY_IDENTIFIER,
        media_tag_prefix: MEDIA_TAG_PREFIX,
        media_tag_version: MEDIA_TAG_VERSION,
        nocache: parent.is_branch().then_some(true),
        title1: library.title.clone(),
        view_group: style.view_group,
        view_mode: crate::items::GRID_VIEW_MODE,
        metadata: items,
    };
    Ok(PlexResponse::new(body, format).paged(offset, total))
}
