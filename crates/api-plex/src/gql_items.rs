//! The official-API queries behind library listings, and the shapes they return.
//!
//! Both queries live here as constants. A static query string is greppable against the
//! schema, reviewable as one piece, and — unlike a query assembled from request parameters —
//! cannot be shaped by client input. What the client controls arrives as variables, which
//! are values and never syntax.

use std::sync::LazyLock;

use serde::Deserialize;

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
/// `VersionFields` stops short of `files` so each query can select its own depth: a listing
/// takes the files, a lookup takes the tracks inside them. The alternative — one fragment per
/// depth — duplicates the twelve scalars above and lets them drift.
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
  $id: ID!
  $first: Int!
  $offset: Int!
  $filter: ItemFilterInput
  $orderBy: ItemOrder!
  $direction: SortDirection!
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
      ) {
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
            ... on Season {
              number
              year
              show { id title }
              seasonStats: stats { episodeCount }
            }
            ... on Episode {
              number
              originalTitle
              year
              releasedOn
              contentRating
              audienceRating
              durationMs
              season { id number }
              show { id title }
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

/// `GET /library/metadata/{ids}`.
///
/// Takes a list of global ids and refetches each through `node`, which is what Relay's
/// object identification is for. Streams are included here, unlike in a listing.
pub static ITEMS_BY_ID_QUERY: LazyLock<String> = LazyLock::new(|| {
    with_shared_fragments(
        r"
query PlexItems($ids: [ID!]!) {
  items: nodes(ids: $ids) {
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
      versions {
        ...VersionFields
        files {
          id
          path
          sizeBytes
          durationMs
          streams {
            id
            kind
            position
            codec
            language
            channels
            bitrate
            isDefault
            isForced
          }
        }
      }
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
    ... on Season {
      number
      year
      show { id title }
      seasonStats: stats { episodeCount }
    }
    ... on Episode {
      number
      originalTitle
      year
      releasedOn
      contentRating
      audienceRating
      durationMs
      season { id number }
      show { id title }
      versions {
        ...VersionFields
        files {
          id
          path
          sizeBytes
          durationMs
          streams {
            id
            kind
            position
            codec
            language
            channels
            bitrate
            isDefault
            isForced
          }
        }
      }
    }
  }
}
",
    )
});

/// Appends the shared fragments to a query body.
///
/// Concatenated at first use rather than written out twice: a fragment set that drifts
/// between two copies is a bug nothing would catch until a field went missing from one
/// endpoint.
pub fn with_shared_fragments(query: &str) -> String {
    format!("{query}{ITEM_FRAGMENTS}")
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
    pub released_on: Option<String>,
    pub duration_ms: Option<i64>,
    pub rating: Option<f64>,
    pub audience_rating: Option<f64>,
    pub added_at: i64,
    pub updated_at: i64,
    pub library: Option<GqlLibraryRef>,
    pub season: Option<GqlParentRef>,
    pub show: Option<GqlParentRef>,
    pub show_stats: Option<GqlShowStats>,
    pub season_stats: Option<GqlSeasonStats>,
    pub versions: Vec<GqlVersion>,
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

    /// The item one level up: the season of an episode, the show of a season.
    pub fn parent_rating_key(&self) -> Option<String> {
        let parent = match self.typename.as_str() {
            "Episode" => self.season.as_ref(),
            "Season" => self.show.as_ref(),
            _ => None,
        }?;
        Some(database_id(&parent.id).to_string())
    }

    /// The show of an episode, two levels up. Clients read it to link an episode straight to
    /// its series without walking the season.
    pub fn grandparent_rating_key(&self) -> Option<String> {
        if self.typename != "Episode" {
            return None;
        }
        Some(database_id(&self.show.as_ref()?.id).to_string())
    }

    /// The season number of an episode.
    pub fn parent_index(&self) -> Option<i32> {
        if self.typename != "Episode" {
            return None;
        }
        self.season.as_ref()?.number
    }

    pub fn grandparent_title(&self) -> Option<String> {
        if self.typename != "Episode" {
            return None;
        }
        self.show.as_ref()?.title.clone()
    }
}

/// A parent item, as much of it as the child's attributes need.
#[derive(Debug, Default, Deserialize)]
pub struct GqlParentRef {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub number: Option<i32>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlShowStats {
    pub season_count: u64,
    pub episode_count: u64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlSeasonStats {
    pub episode_count: u64,
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
    pub files: Vec<GqlPart>,
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
    pub streams: Vec<GqlStream>,
}

impl GqlPart {
    pub fn database_id(&self) -> i64 {
        database_id(&self.id)
    }
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
fn database_id(global_id: &str) -> i64 {
    open_plex_api::GlobalId::decode(global_id).map_or(0, |decoded| decoded.key)
}
