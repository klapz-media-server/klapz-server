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

fragment SeasonFields on Season {
  number
  year
  show { id title guid studio number year }
  seasonStats: stats { episodeCount }
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

fragment ListedMovieTags on Movie {
  tags {
    genres(first: 2) { edges { node { name } } }
    countries(first: 2) { edges { node { name } } }
    directors(first: 3) { edges { node { name } } }
    writers(first: 2) { edges { node { name } } }
    cast(first: 3) { edges { node { name } } }
  }
}

fragment ListedShowTags on Show {
  tags(includeEpisodes: false) {
    genres(first: 2) { edges { node { name } } }
    countries(first: 2) { edges { node { name } } }
    cast(first: 3) { edges { node { name } } }
  }
}

fragment ListedEpisodeTags on Episode {
  tags {
    directors(first: 3) { edges { node { name } } }
    writers(first: 2) { edges { node { name } } }
    cast(first: 3) { edges { node { name } } }
  }
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
}

fragment ShowTags on Show {
  tags(includeEpisodes: false) {
    genres(first: 500) { edges { node { ...TagFields } } }
    countries(first: 500) { edges { node { ...TagFields } } }
    cast(first: 500) { edges { character node { ...TagFields } } }
  }
}

fragment EpisodeTags on Episode {
  tags {
    directors(first: 500) { edges { node { ...TagFields } } }
    writers(first: 500) { edges { node { ...TagFields } } }
    cast(first: 500) { edges { character node { ...TagFields } } }
    producers(first: 500) { edges { node { ...TagFields } } }
  }
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
              ...ListedMovieTags
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
              ...ListedShowTags
            }
            ... on Season { ...SeasonFields }
            ... on Episode {
              ...EpisodeFields
              versions { ...VersionFields files { id path sizeBytes durationMs } }
              ...ListedEpisodeTags
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
    ...References
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
      ...MovieTags
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
      ...ShowTags
    }
    ... on Season { ...SeasonFields }
    ... on Episode {
      ...EpisodeFields
      ...EpisodeTags
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
    pub tags: GqlItemTags,
    pub external_ids: GqlConnection<GqlExternalId>,
    pub ratings: GqlConnection<GqlRating>,
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
    /// is derived — see [`season_title`].
    pub fn parent_title(&self) -> Option<String> {
        let parent = self.parent()?;
        if self.typename == "Episode" {
            return Some(season_title(parent.title.as_deref(), parent.number));
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
    pub fn display_title(&self) -> Option<String> {
        if self.typename == "Season" {
            return Some(season_title(self.title.as_deref(), self.number));
        }
        self.title.clone()
    }
}

/// Names a season the way the reference does when the source stores no title.
///
/// `Season 1`, in English regardless of the library's language — a French library's seasons
/// read `Season 1` on the reference too — and `Specials` for season zero, the convention
/// scanners use for extras and specials. A stored title wins, as it does for *The Haunting
/// of Bly Manor*, a season titled as its own series.
pub fn season_title(stored: Option<&str>, number: Option<i32>) -> String {
    match (stored, number) {
        (Some(title), _) => title.to_owned(),
        (None, Some(0)) => "Specials".to_owned(),
        (None, Some(number)) => format!("Season {number}"),
        (None, None) => String::new(),
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
        for document in [&*SECTION_ITEMS_QUERY, &*ITEMS_BY_ID_QUERY] {
            for name in [
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
