//! The official-API queries behind library listings, and the shapes they return.
//!
//! Both queries live here as constants. A static query string is greppable against the
//! schema, reviewable as one piece, and — unlike a query assembled from request parameters —
//! cannot be shaped by client input. What the client controls arrives as variables, which
//! are values and never syntax.

use std::sync::LazyLock;

use serde::Deserialize;

use crate::language::Language;

/// The item fields both endpoints read.
///
/// One fragment set rather than one per endpoint: the two differ in which *container*
/// attributes they carry, not in what they know about an item. `__typename` is what tells
/// the adapter which kind it is holding, and therefore the `type` attribute and the XML
/// element name.
///
/// Field availability is not uniform across the kinds, and that is deliberate on the schema
/// side: no show carries a critic rating, no episode a studio or tagline, and no season a
/// duration — so those fields do not exist on those types rather than resolving to null.
///
/// Tags come in two forms, one fragment per kind and form. A lookup carries every tag with
/// its identity; a listing carries names only, and only the first few of each kind:
///
/// | Element | Listing keeps |
/// | --- | --- |
/// | `Genre`, `Country`, `Writer` | 2 |
/// | `Director`, `Role` | 3 |
/// | `Producer` | none |
///
/// A series' own credits only: the official API appends those of its episodes unless told
/// not to, and the reference shows a series' regular cast alone. Neither form carries
/// a season's or an episode's genres, which the official API inherits from the series and
/// the reference does not publish. `References` — external identifiers and ratings — appear
/// in a lookup only.
///
/// Measured over every film, series and episode of the reference library by comparing the
/// listing against the item's full tag set: the listing is the first *n* in billing order,
/// with these *n* and no others. The reference reads them from denormalized columns that a
/// metadata refresh rewrites, and on 0.5% of items those columns are stale — a renamed
/// person, a dropped country. We derive from the tags themselves and differ on those.
///
/// `Markers` is selected only when a request carries `includeMarkers=1`, through the
/// `$markers` variable each query spreading it declares, false by default: the reference
/// publishes them on request alone.
///
/// `VersionFields` stops short of `files` so each query can select its own depth: a listing
/// takes the files, a lookup takes the tracks inside them. The alternative — one fragment per
/// depth — duplicates the twelve scalars above and lets them drift. `SeasonFields` and
/// `EpisodeFields` are the scalars and parents of those two kinds, shared by every endpoint
/// that returns either; `EpisodeFields` likewise stops short of `versions`, for the same
/// reason.
const ITEM_FRAGMENTS: &str = r"
fragment Shared on Item {
  __typename
  id
  guid
  title
  titleSort
  summary
  addedAt
  updatedAt
  library { id title uuid kind }
  watchState { viewCount offsetMs lastViewedAt }
}

fragment VersionFields on MediaVersion {
  id
  width
  height
  aspectRatio
  audioChannels
  audioCodec
  videoCodec
  bitrate
  container
  durationMs
  frameRate
  optimizedForStreaming
  primaryVideo { profile }
  primaryAudio { profile }
}

fragment FileFields on MediaFile {
  id
  path
  sizeBytes
  durationMs
  hasThumbnail
  has64BitOffsets
  optimizedForStreaming
  packetLength
  hasChapterTextStream
  selectedAudio { id }
  selectedSubtitle { id }
}

fragment TrackFields on MediaStream {
  id
  kind
  position
  codec
  language
  channels
  bitrate
  isDefault
  isForced
  isExternal
  title
  streamIdentifier
  profile
  bitDepth
  traits { isOriginal isDub isCommentary isHearingImpaired isVisualImpaired hasHeaderCompression }
  video {
    width
    height
    codedWidth
    codedHeight
    frameRate
    level
    refFrames
    scan
    chromaSubsampling
    chromaLocation
    color { range space primaries transfer }
    hasScalingMatrix
    isAnamorphic
    pixelAspectRatio
    fourcc
    hasClosedCaptions
    hasSeparateFields
    hasHdr10Plus
    dolbyVision {
      version
      profile
      level
      baseLayerCompatibility
      hasBaseLayer
      hasEnhancementLayer
      hasRpu
    }
  }
  audio {
    samplingRate
    channelLayout
    loudness { analysisVersion integrated range truePeak threshold gainOffset }
  }
  subtitle { format isEmbeddedInVideo }
}

fragment SeasonFields on Season {
  number
  year
  show { id title guid studio number year }
  seasonStats: stats { episodeCount watchedEpisodeCount }
}

fragment EpisodeFields on Episode {
  number
  originalTitle
  year
  releasedOn
  contentRating
  audienceRating
  durationMs
  season { id number title year guid }
  show { id title guid }
}

fragment ListedItem on Item {
  ...Shared
  ... on Movie { ...MovieListing }
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
    showStats: stats { seasonCount episodeCount watchedEpisodeCount }
    ...ListedShowTags
  }
  ... on Season { ...SeasonFields }
  ... on Episode {
    ...EpisodeFields
    versions { ...VersionFields files { ...FileFields } }
    ...ListedEpisodeTags
  }
}

fragment MovieListing on Movie {
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
  versions { ...VersionFields files { ...FileFields } }
  ...ListedMovieTags
}

fragment StreamedVersion on MediaVersion {
  ...VersionFields
  files {
    ...FileFields
    streams { ...TrackFields }
  }
}

fragment ShowLookup on Show {
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
  showStats: stats { seasonCount episodeCount watchedEpisodeCount }
  ...ShowTags
}

fragment EpisodeLookup on Episode {
  ...EpisodeFields
  ...EpisodeTags
  versions { ...StreamedVersion }
}

fragment RecentItem on Item {
  ...Shared
  ... on Movie { ...MovieListing }
  ... on Show { ...ShowLookup ...References }
  ... on Season { ...SeasonFields ...References }
  ... on Episode { ...EpisodeLookup ...References }
}

fragment ShelfItem on Item {
  ...Shared
  ... on Movie { ...MovieListing }
  ... on Show { ...ShowLookup }
  ... on Season { ...SeasonFields }
  ... on Episode { ...EpisodeLookup }
}

fragment ListedMovieTags on Movie {
  tags {
    genres(first: 2) { edges { node { name } } }
    countries(first: 2) { edges { node { name } } }
    directors(first: 3) { edges { node { name } } }
    writers(first: 2) { edges { node { name } } }
    cast(first: 3) { edges { node { name } } }
  }
  collections(first: 1) { edges { node { title } } }
}

fragment ListedShowTags on Show {
  tags(includeEpisodes: false) {
    genres(first: 2) { edges { node { name } } }
    countries(first: 2) { edges { node { name } } }
    cast(first: 3) { edges { node { name } } }
  }
  collections(first: 1) { edges { node { title } } }
}

fragment ListedEpisodeTags on Episode {
  tags {
    directors(first: 3) { edges { node { name } } }
    writers(first: 2) { edges { node { name } } }
    cast(first: 3) { edges { node { name } } }
  }
  collections(first: 1) { edges { node { title } } }
}

fragment Markers on Item {
  ... on Movie {
    markers(first: 500) @include(if: $markers) { edges { node { ...MarkerFields } } }
  }
  ... on Episode {
    markers(first: 500) @include(if: $markers) { edges { node { ...MarkerFields } } }
  }
}

fragment MarkerFields on Marker {
  id
  kind
  startMs
  endMs
  final
  version
  title
  color
}

fragment TagFields on Tag {
  id
  name
  person { providerKey thumbUrl }
}

fragment References on Item {
  ... on Movie {
    externalIds(first: 50) { edges { node { source value } } }
    ratings(first: 50) { edges { node { source kind value image } } }
  }
  ... on Show {
    externalIds(first: 50) { edges { node { source value } } }
    ratings(first: 50) { edges { node { source kind value image } } }
  }
  ... on Season {
    externalIds(first: 50) { edges { node { source value } } }
    ratings(first: 50) { edges { node { source kind value image } } }
  }
  ... on Episode {
    externalIds(first: 50) { edges { node { source value } } }
    ratings(first: 50) { edges { node { source kind value image } } }
  }
}

fragment MovieTags on Movie {
  tags {
    genres(first: 500) { edges { node { ...TagFields } } }
    countries(first: 500) { edges { node { ...TagFields } } }
    directors(first: 500) { edges { node { ...TagFields } } }
    writers(first: 500) { edges { node { ...TagFields } } }
    cast(first: 500) { edges { character node { ...TagFields } } }
    producers(first: 500) { edges { node { ...TagFields } } }
  }
  collections(first: 500) { edges { node { tagNumber title guid summary } } }
}

fragment ShowTags on Show {
  tags(includeEpisodes: false) {
    genres(first: 500) { edges { node { ...TagFields } } }
    countries(first: 500) { edges { node { ...TagFields } } }
    cast(first: 500) { edges { character node { ...TagFields } } }
  }
  collections(first: 500) { edges { node { tagNumber title guid summary } } }
}

fragment EpisodeTags on Episode {
  tags {
    directors(first: 500) { edges { node { ...TagFields } } }
    writers(first: 500) { edges { node { ...TagFields } } }
    cast(first: 500) { edges { character node { ...TagFields } } }
    producers(first: 500) { edges { node { ...TagFields } } }
  }
  collections(first: 500) { edges { node { tagNumber title guid summary } } }
}
";

/// `GET /library/sections/{id}/all`.
///
/// Reaches the library through `node`, which is Relay's single refetch field, so the
/// adapter needs no second entry point for "a library by its number". `stats` and `items`
/// take the same filter, which is what makes `totalSize` describe the listing actually
/// returned rather than the whole library.
///
/// Streams are left out: a reference server omits them from a listing and includes them in
/// a lookup, and a page of 50 films would otherwise carry several hundred tracks nobody
/// asked for.
pub static SECTION_ITEMS_QUERY: LazyLock<String> = LazyLock::new(|| {
    with_shared_fragments(
        r"
query PlexSectionItems(
  $markers: Boolean = false
  $id: ID!
  $first: Int!
  $offset: Int!
  $filter: ItemFilterInput
  $orderBy: ItemOrder!
  $direction: SortDirection!
  $seed: Int
) {
  node(id: $id) {
    ... on Library {
      id
      title
      uuid
      kind
      stats(filter: $filter) { itemCount }
      items(
        first: $first
        offset: $offset
        filter: $filter
        orderBy: $orderBy
        direction: $direction
        seed: $seed
      ) {
        edges {
          node {
            ...ListedItem
            ...Markers
          }
        }
      }
    }
  }
}
",
    )
});

/// A library's recently added items, for its shelves.
///
/// In the form the reference shows each kind in on that shelf, which is not the same: a
/// film as a listing shows it, while a series, a season or an episode comes as a lookup
/// does — every tag with its id, the external identifiers and ratings, the streams.
/// Measured on both shelves; a film library holds films only and a series library none, so
/// the type alone decides.
pub static RECENTLY_ADDED_QUERY: LazyLock<String> = LazyLock::new(|| {
    with_shared_fragments(
        r"
query PlexRecentlyAdded($id: ID!, $first: Int!) {
  node(id: $id) {
    ... on Library {
      id
      title
      uuid
      kind
      recentlyAdded(first: $first) {
        pageInfo { hasNextPage }
        edges { addedAt node { ...RecentItem } }
      }
    }
  }
}
",
    )
});

/// One shelf: the head of a library listing, its items in a shelf's form — a lookup's,
/// without the external references only the recently added shelf carries.
pub static SHELF_QUERY: LazyLock<String> = LazyLock::new(|| {
    with_shared_fragments(
        r"
query PlexShelf(
  $id: ID!
  $first: Int!
  $filter: ItemFilterInput
  $orderBy: ItemOrder!
  $direction: SortDirection!
  $seed: Int
) {
  node(id: $id) {
    ... on Library {
      items(
        first: $first
        filter: $filter
        orderBy: $orderBy
        direction: $direction
        seed: $seed
      ) {
        pageInfo { hasNextPage }
        edges { node { ...ShelfItem } }
      }
    }
  }
}
",
    )
});

#[derive(Debug, Deserialize)]
pub struct ShelfData {
    pub node: Option<GqlShelfLibrary>,
}

#[derive(Debug, Deserialize)]
pub struct GqlShelfLibrary {
    pub items: GqlShelfConnection,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlShelfConnection {
    pub page_info: GqlForwardPageInfo,
    pub edges: Vec<GqlShelfEdge>,
}

#[derive(Debug, Deserialize)]
pub struct GqlShelfEdge {
    pub node: GqlItem,
}

/// What one library, or every library of a kind, added most recently, and how many films
/// that covers.
pub static HOME_RECENT_QUERY: LazyLock<String> = LazyLock::new(|| {
    with_shared_fragments(
        r"
query PlexHomeRecent($kind: LibraryKind!, $first: Int!, $offset: Int!) {
  recentlyAdded(kind: $kind, first: $first, offset: $offset) {
    edges { addedAt node { ...RecentItem } }
  }
  libraries(first: 500) {
    edges { node { kind stats { itemCount } } }
  }
}
",
    )
});

/// [`HOME_RECENT_QUERY`] for one library.
pub static LIBRARY_RECENT_QUERY: LazyLock<String> = LazyLock::new(|| {
    with_shared_fragments(
        r"
query PlexLibraryRecent($id: ID!, $kind: LibraryKind!, $first: Int!, $offset: Int!) {
  node(id: $id) {
    ... on Library {
      kind
      stats { itemCount }
    }
  }
  recentlyAdded(kind: $kind, library: $id, first: $first, offset: $offset) {
    edges { addedAt node { ...RecentItem } }
  }
}
",
    )
});

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HomeRecentData {
    pub recently_added: GqlRecentEdges,
    pub libraries: GqlKindCounts,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryRecentData {
    pub node: Option<GqlKindCount>,
    pub recently_added: GqlRecentEdges,
}

#[derive(Debug, Deserialize)]
pub struct GqlRecentEdges {
    pub edges: Vec<GqlRecentEdge>,
}

#[derive(Debug, Deserialize)]
pub struct GqlKindCounts {
    pub edges: Vec<GqlKindCountEdge>,
}

#[derive(Debug, Deserialize)]
pub struct GqlKindCountEdge {
    pub node: GqlKindCount,
}

#[derive(Debug, Deserialize)]
pub struct GqlKindCount {
    pub kind: String,
    pub stats: GqlLibraryStats,
}

/// Everything Continue Watching needs from the official API.
pub static CONTINUE_WATCHING_QUERY: LazyLock<String> = LazyLock::new(|| {
    with_shared_fragments(
        r"
query PlexContinueWatching($library: ID, $first: Int!) {
  continueWatching(library: $library, first: $first) {
    pageInfo { hasNextPage }
    edges { reason lastViewedAt node { ...RecentItem } }
  }
}
",
    )
});

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContinueWatchingData {
    pub continue_watching: GqlContinueConnection,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlContinueConnection {
    pub page_info: GqlForwardPageInfo,
    pub edges: Vec<GqlContinueEdge>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlContinueEdge {
    /// `IN_PROGRESS` or `UP_NEXT`.
    pub reason: String,
    #[serde(deserialize_with = "crate::gql::date_time::seconds")]
    pub last_viewed_at: i64,
    pub node: GqlItem,
}

#[derive(Debug, Deserialize)]
pub struct RecentlyAddedData {
    pub node: Option<GqlRecentlyAdded>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlRecentlyAdded {
    pub id: String,
    pub title: String,
    pub uuid: String,
    pub kind: String,
    pub recently_added: GqlRecentConnection,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlRecentConnection {
    pub page_info: GqlForwardPageInfo,
    pub edges: Vec<GqlRecentEdge>,
}

/// An entry, with when it was added as far as the shelf is concerned.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlRecentEdge {
    #[serde(deserialize_with = "crate::gql::date_time::seconds")]
    pub added_at: i64,
    pub node: GqlItem,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlForwardPageInfo {
    pub has_next_page: bool,
}

/// `GET /library/metadata/{ids}`.
///
/// Takes a list of global ids and refetches each through `node`, which is what Relay's
/// object identification is for. Streams are included here, unlike in a listing.
pub static ITEMS_BY_ID_QUERY: LazyLock<String> = LazyLock::new(|| {
    with_shared_fragments(
        r"
query PlexItems($ids: [ID!]!, $markers: Boolean = false) {
  items: nodes(ids: $ids) {
    ...Shared
    ...References
    ...Markers
    ... on Item { lockedFields }
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
      versions { ...StreamedVersion }
      ...MovieTags
    }
    ... on Show { ...ShowLookup }
    ... on Season { ...SeasonFields }
    ... on Episode { ...EpisodeLookup }
  }
}
",
    )
});

/// Appends the shared fragments a query body spreads, and only those.
///
/// Concatenated at first use rather than written out per query: a fragment set that drifts
/// between copies is a bug nothing would catch until a field went missing from one
/// endpoint. Filtered because GraphQL rejects a document carrying a fragment it never
/// spreads, and not every query lists every kind — one that never lists seasons must not
/// carry `SeasonFields`.
pub fn with_shared_fragments(query: &str) -> String {
    let fragments: Vec<(&str, &str)> = ITEM_FRAGMENTS
        .split("\n\n")
        .filter_map(|fragment| Some((fragment_name(fragment)?, fragment)))
        .collect();

    // Fragments spread other fragments, so the set closes over spreads until it stops
    // growing.
    let mut document = query.to_owned();
    let mut included: Vec<&str> = Vec::new();
    loop {
        let next = fragments
            .iter()
            .copied()
            .find(|(name, _)| !included.contains(name) && document.contains(&format!("...{name}")));
        let Some((name, fragment)) = next else {
            break;
        };
        included.push(name);
        document.push('\n');
        document.push_str(fragment.trim_end());
        document.push('\n');
    }
    document
}

fn fragment_name(fragment: &str) -> Option<&str> {
    fragment
        .trim_start()
        .strip_prefix("fragment ")?
        .split_whitespace()
        .next()
}

#[derive(Debug, Deserialize)]
pub struct SectionItemsData {
    pub node: Option<GqlLibraryItems>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlLibraryItems {
    pub id: String,
    pub title: String,
    pub uuid: String,
    pub kind: String,
    pub stats: GqlLibraryStats,
    pub items: GqlItemConnection,
}

impl GqlLibraryItems {
    /// The library as the container attributes need it.
    pub fn as_ref(&self) -> GqlLibraryRef {
        GqlLibraryRef {
            id: self.id.clone(),
            title: self.title.clone(),
            uuid: self.uuid.clone(),
            kind: self.kind.clone(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlLibraryStats {
    pub item_count: u64,
}

#[derive(Debug, Deserialize)]
pub struct GqlItemConnection {
    pub edges: Vec<GqlItemEdge>,
}

#[derive(Debug, Deserialize)]
pub struct GqlItemEdge {
    pub node: GqlItem,
}

#[derive(Debug, Deserialize)]
pub struct ItemsByIdData {
    pub items: Vec<Option<GqlItem>>,
}

/// The library an item belongs to, as much of it as a container attribute needs.
#[derive(Debug, Clone, Deserialize)]
pub struct GqlLibraryRef {
    pub id: String,
    pub title: String,
    pub uuid: String,
    pub kind: String,
}

impl GqlLibraryRef {
    /// The server-local number, which is what Plex publishes as `librarySectionID`.
    pub fn database_id(&self) -> i64 {
        database_id(&self.id)
    }
}

/// An item of any kind, flattened.
///
/// One struct rather than four, because the fragments above select a different subset per
/// kind and the Plex wire format is itself a flat union: whichever attributes apply are
/// emitted and the rest omitted. `#[serde(default)]` is what lets one struct absorb all four
/// selection sets — a field the fragment did not ask for is simply absent.
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GqlItem {
    #[serde(rename = "__typename")]
    pub typename: String,
    pub id: String,
    pub guid: String,
    pub title: Option<String>,
    pub title_sort: Option<String>,
    pub original_title: Option<String>,
    pub summary: Option<String>,
    pub tagline: Option<String>,
    pub studio: Option<String>,
    pub content_rating: Option<String>,
    pub content_rating_age: Option<i32>,
    pub number: Option<i32>,
    pub year: Option<i32>,
    #[serde(
        default,
        deserialize_with = "crate::gql::date_time::optional_calendar_date"
    )]
    pub released_on: Option<String>,
    pub duration_ms: Option<i64>,
    pub rating: Option<f64>,
    pub audience_rating: Option<f64>,
    #[serde(deserialize_with = "crate::gql::date_time::seconds")]
    pub added_at: i64,
    #[serde(deserialize_with = "crate::gql::date_time::seconds")]
    pub updated_at: i64,
    pub library: Option<GqlLibraryRef>,
    pub season: Option<GqlParentRef>,
    pub show: Option<GqlParentRef>,
    pub show_stats: Option<GqlShowStats>,
    pub season_stats: Option<GqlSeasonStats>,
    /// The caller's: null for a caller with none.
    pub watch_state: Option<GqlWatchState>,
    pub versions: Vec<GqlVersion>,
    pub tags: GqlItemTags,
    /// The collections it is in: the first one's title in a listing, all of them in a
    /// lookup.
    pub collections: GqlConnection<GqlCollectionTag>,
    pub external_ids: GqlConnection<GqlExternalId>,
    pub ratings: GqlConnection<GqlRating>,
    /// `ItemField` names. A lookup asks for them, a listing does not.
    pub locked_fields: Vec<String>,
    /// Asked for on `includeMarkers=1` only.
    pub markers: GqlConnection<GqlMarker>,
}

/// A marker, as `MarkerFields` selects it.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlMarker {
    pub id: String,
    /// `INTRO`, `CREDITS`, `COMMERCIAL` or `BOOKMARK`.
    pub kind: String,
    pub start_ms: i64,
    pub end_ms: Option<i64>,
    #[serde(rename = "final")]
    pub is_final: bool,
    pub version: Option<i32>,
    pub title: Option<String>,
    pub color: Option<String>,
}

impl GqlItem {
    /// The server-local number, which Plex publishes as `ratingKey`.
    ///
    /// `None` when the global id does not decode, which would be a schema defect rather than
    /// client input — the item is then dropped from the response instead of appearing with a
    /// key no client could follow.
    pub fn database_id(&self) -> Option<i64> {
        open_plex_api::GlobalId::decode(&self.id)
            .ok()
            .map(|decoded| decoded.key)
    }

    /// The `type` attribute, which drives both the client's layout and the XML element name.
    pub fn plex_type(&self) -> &'static str {
        match self.typename.as_str() {
            "Show" => "show",
            "Season" => "season",
            "Episode" => "episode",
            _ => "movie",
        }
    }

    /// Whether a client browses into this item rather than playing it.
    pub fn has_children(&self) -> bool {
        matches!(self.typename.as_str(), "Show" | "Season")
    }

    pub fn child_count(&self) -> Option<u64> {
        self.show_stats.as_ref().map(|stats| stats.season_count)
    }

    pub fn leaf_count(&self) -> Option<u64> {
        self.show_stats
            .as_ref()
            .map(|stats| stats.episode_count)
            .or_else(|| self.season_stats.as_ref().map(|stats| stats.episode_count))
    }

    /// The caller's watched episodes below a series or a season.
    pub fn viewed_leaf_count(&self) -> Option<u64> {
        self.show_stats
            .as_ref()
            .map(|stats| stats.watched_episode_count)
            .or_else(|| {
                self.season_stats
                    .as_ref()
                    .map(|stats| stats.watched_episode_count)
            })
    }

    /// The item one level up: the season of an episode, the show of a season.
    pub fn parent(&self) -> Option<&GqlParentRef> {
        match self.typename.as_str() {
            "Episode" => self.season.as_ref(),
            "Season" => self.show.as_ref(),
            _ => None,
        }
    }

    /// The show of an episode, two levels up. Clients read it to link an episode straight to
    /// its series without walking the season.
    pub fn grandparent(&self) -> Option<&GqlParentRef> {
        if self.typename != "Episode" {
            return None;
        }
        self.show.as_ref()
    }

    pub fn parent_rating_key(&self) -> Option<String> {
        Some(self.parent()?.database_id().to_string())
    }

    pub fn grandparent_rating_key(&self) -> Option<String> {
        Some(self.grandparent()?.database_id().to_string())
    }

    /// The parent's number: the season of an episode, the series' own index for a season.
    pub fn parent_index(&self) -> Option<i32> {
        self.parent()?.number
    }

    /// The parent's title as a client shows it. For an episode that is the season's, which
    /// is derived — see [`Language::season_title`].
    pub fn parent_title(&self, language: Language) -> Option<String> {
        let parent = self.parent()?;
        if self.typename == "Episode" {
            return Some(language.season_title(parent.title.as_deref(), parent.number));
        }
        parent.title.clone()
    }

    pub fn grandparent_title(&self) -> Option<String> {
        self.grandparent()?.title.clone()
    }

    /// The title a client shows for this item.
    ///
    /// Only a season differs from its stored title: the source leaves 9,732 of 9,736 seasons
    /// untitled, and the reference names them at read time.
    pub fn display_title(&self, language: Language) -> Option<String> {
        if self.typename == "Season" {
            return Some(language.season_title(self.title.as_deref(), self.number));
        }
        self.title.clone()
    }
}

/// A parent item, as much of it as the child's attributes need.
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GqlParentRef {
    pub id: String,
    pub title: Option<String>,
    pub number: Option<i32>,
    pub guid: Option<String>,
    pub studio: Option<String>,
    pub year: Option<i32>,
    pub content_rating: Option<String>,
}

impl GqlParentRef {
    pub fn database_id(&self) -> i64 {
        database_id(&self.id)
    }
}

/// A connection, reduced to what the adapter reads: its edges.
#[derive(Debug, Deserialize)]
#[serde(bound(deserialize = "T: Deserialize<'de>"))]
pub struct GqlConnection<T> {
    pub edges: Vec<GqlEdge<T>>,
}

impl<T> Default for GqlConnection<T> {
    fn default() -> Self {
        Self { edges: Vec::new() }
    }
}

impl<T> GqlConnection<T> {
    pub fn nodes(&self) -> impl Iterator<Item = &T> {
        self.edges.iter().map(|edge| &edge.node)
    }
}

#[derive(Debug, Deserialize)]
pub struct GqlEdge<T> {
    pub node: T,
    /// Set on a cast edge only.
    #[serde(default)]
    pub character: Option<String>,
}

/// An item's tags, by kind. A kind the query did not select stays empty.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct GqlItemTags {
    pub genres: GqlConnection<GqlTag>,
    pub countries: GqlConnection<GqlTag>,
    pub directors: GqlConnection<GqlTag>,
    pub writers: GqlConnection<GqlTag>,
    pub cast: GqlConnection<GqlTag>,
    pub producers: GqlConnection<GqlTag>,
}

/// A tag, with as much of it as the query selected: a listing asks for the name alone.
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GqlTag {
    pub id: Option<String>,
    pub name: String,
    pub person: Option<GqlPerson>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GqlPerson {
    pub provider_key: Option<String>,
    pub thumb_url: Option<String>,
}

/// A collection as an item names it, as much of it as the query selected.
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GqlCollectionTag {
    pub tag_number: Option<String>,
    pub title: String,
    pub guid: Option<String>,
    pub summary: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct GqlExternalId {
    pub source: String,
    pub value: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct GqlRating {
    pub source: String,
    /// `CRITIC` or `AUDIENCE`.
    pub kind: String,
    pub value: f64,
    pub image: String,
}

impl GqlTag {
    /// The server-local number, which Plex publishes as the tag's `id` and in its `filter`.
    /// `None` when the query did not select the id, as a listing does not.
    pub fn database_id(&self) -> Option<i64> {
        self.id.as_deref().map(database_id)
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlShowStats {
    pub season_count: u64,
    pub episode_count: u64,
    #[serde(default)]
    pub watched_episode_count: u64,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GqlWatchState {
    pub view_count: u32,
    pub offset_ms: Option<i64>,
    #[serde(default, deserialize_with = "crate::gql::date_time::optional_seconds")]
    pub last_viewed_at: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlSeasonStats {
    pub episode_count: u64,
    #[serde(default)]
    pub watched_episode_count: u64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GqlVersion {
    pub id: String,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub aspect_ratio: Option<f64>,
    pub audio_channels: Option<i32>,
    pub audio_codec: Option<String>,
    pub video_codec: Option<String>,
    pub bitrate: Option<i32>,
    pub container: Option<String>,
    pub duration_ms: Option<i64>,
    pub frame_rate: Option<f64>,
    pub optimized_for_streaming: bool,
    pub primary_video: Option<GqlProfiled>,
    pub primary_audio: Option<GqlProfiled>,
    pub files: Vec<GqlPart>,
}

/// A track read only for its profile.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct GqlProfiled {
    pub profile: Option<String>,
}

impl GqlVersion {
    pub fn database_id(&self) -> i64 {
        database_id(&self.id)
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GqlPart {
    pub id: String,
    pub path: String,
    pub size_bytes: Option<i64>,
    pub duration_ms: Option<i64>,
    pub has_thumbnail: bool,
    #[serde(rename = "has64BitOffsets")]
    pub has_64bit_offsets: Option<bool>,
    pub optimized_for_streaming: Option<bool>,
    pub packet_length: Option<i32>,
    pub has_chapter_text_stream: bool,
    pub streams: Vec<GqlStream>,
    pub selected_audio: Option<GqlTrackRef>,
    pub selected_subtitle: Option<GqlTrackRef>,
}

impl GqlPart {
    pub fn database_id(&self) -> i64 {
        database_id(&self.id)
    }

    /// The tracks the caller plays this part with, by database id.
    pub fn selected(&self) -> (Option<i64>, Option<i64>) {
        let id = |track: &Option<GqlTrackRef>| track.as_ref().map(|track| database_id(&track.id));
        (id(&self.selected_audio), id(&self.selected_subtitle))
    }
}

#[derive(Debug, Deserialize)]
pub struct GqlTrackRef {
    pub id: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GqlStream {
    pub id: String,
    pub kind: String,
    pub position: i32,
    pub codec: Option<String>,
    pub language: Option<String>,
    pub channels: Option<i32>,
    pub bitrate: Option<i32>,
    pub is_default: bool,
    pub is_forced: bool,
    pub is_external: bool,
    pub title: Option<String>,
    pub stream_identifier: Option<i32>,
    pub profile: Option<String>,
    pub bit_depth: Option<i32>,
    pub traits: GqlTraits,
    pub video: Option<GqlVideo>,
    pub audio: Option<GqlAudio>,
    pub subtitle: Option<GqlSubtitle>,
}

#[allow(
    clippy::struct_excessive_bools,
    reason = "independent flags the source records one by one, not states to fold into an enum"
)]
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GqlTraits {
    pub is_original: bool,
    pub is_dub: bool,
    pub is_commentary: bool,
    pub is_hearing_impaired: bool,
    pub is_visual_impaired: bool,
    pub has_header_compression: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GqlVideo {
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub coded_width: Option<i32>,
    pub coded_height: Option<i32>,
    pub frame_rate: Option<f64>,
    pub level: Option<i32>,
    pub ref_frames: Option<i32>,
    /// `PROGRESSIVE` or `INTERLACED`.
    pub scan: Option<String>,
    pub chroma_subsampling: Option<String>,
    pub chroma_location: Option<String>,
    pub color: GqlColor,
    pub has_scaling_matrix: Option<bool>,
    pub is_anamorphic: Option<bool>,
    pub pixel_aspect_ratio: Option<String>,
    pub fourcc: Option<String>,
    pub has_closed_captions: bool,
    pub has_separate_fields: bool,
    #[serde(rename = "hasHdr10Plus")]
    pub has_hdr10_plus: bool,
    pub dolby_vision: Option<GqlDolbyVision>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct GqlColor {
    pub range: Option<String>,
    pub space: Option<String>,
    pub primaries: Option<String>,
    pub transfer: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GqlDolbyVision {
    pub version: String,
    pub profile: i32,
    pub level: i32,
    pub base_layer_compatibility: Option<i32>,
    pub has_base_layer: bool,
    pub has_enhancement_layer: bool,
    pub has_rpu: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GqlAudio {
    pub sampling_rate: Option<i32>,
    pub channel_layout: Option<String>,
    pub loudness: Option<GqlLoudness>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GqlLoudness {
    pub analysis_version: String,
    pub integrated: f64,
    pub range: f64,
    pub true_peak: f64,
    pub threshold: f64,
    pub gain_offset: f64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GqlSubtitle {
    pub format: Option<String>,
    pub is_embedded_in_video: bool,
}

impl GqlStream {
    pub fn database_id(&self) -> i64 {
        database_id(&self.id)
    }
}

/// Reads the server-local number out of a global id.
///
/// Zero when the id does not decode. That is a schema defect rather than client input, and a
/// visibly wrong key in the response is easier to diagnose than a dropped field.
pub(crate) fn database_id(global_id: &str) -> i64 {
    open_plex_api::GlobalId::decode(global_id).map_or(0, |decoded| decoded.key)
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    #[test]
    fn appends_only_the_fragments_a_query_spreads() {
        let document = with_shared_fragments("query { node { ...Shared ...SeasonFields } }");
        assert!(document.contains("fragment Shared on Item"));
        assert!(document.contains("fragment SeasonFields on Season"));
        // Never spread, so never appended: GraphQL would reject the document otherwise.
        assert!(!document.contains("fragment EpisodeFields"));
        assert!(!document.contains("fragment VersionFields"));
    }

    #[test]
    fn every_listing_query_is_a_closed_document() {
        // Each fragment the query spreads is defined, and each defined fragment is spread.
        for document in [
            &*SECTION_ITEMS_QUERY,
            &*ITEMS_BY_ID_QUERY,
            &*RECENTLY_ADDED_QUERY,
            &*SHELF_QUERY,
            &*HOME_RECENT_QUERY,
            &*LIBRARY_RECENT_QUERY,
        ] {
            for name in [
                "ListedItem",
                "MovieListing",
                "StreamedVersion",
                "ShowLookup",
                "EpisodeLookup",
                "RecentItem",
                "ShelfItem",
                "Shared",
                "VersionFields",
                "SeasonFields",
                "EpisodeFields",
                "ListedMovieTags",
                "ListedShowTags",
                "ListedEpisodeTags",
                "TagFields",
                "References",
                "MovieTags",
                "ShowTags",
                "EpisodeTags",
            ] {
                assert_eq!(
                    document.contains(&format!("...{name}")),
                    document.contains(&format!("fragment {name} on")),
                    "{name}"
                );
            }
        }
    }
}
