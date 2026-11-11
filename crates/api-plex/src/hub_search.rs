//! `GET /hubs/search`: a search answered as shelves, one per kind of result.
//!
//! What Plex Web's search page and most other clients call. The reference always answers the
//! same seventeen shelves, filled or empty, of which this server can fill seven: films,
//! series, episodes, actors, directors, genres and collections. Music, photos, playlists and
//! the rest stay empty, as they would on a server holding none.
//!
//! Matching is the official API's (`search` and `searchTags`), a word prefix with accents
//! folded; the reference matches more loosely (`dune` finds `d'une`). Ranking follows the
//! scores the reference gives, measured on a dozen terms: an exact title first, then a title
//! whose first word is the term, then one starting with it, then the rest, each shelf cut to
//! `limit`. `docs/spec-gaps.md` lists the rules and where they stop.

use std::cmp::Reverse;
use std::collections::HashMap;

use async_graphql::{Name, Value, Variables};
use axum::extract::{RawQuery, State};
use axum::routing::get;
use axum::Router;
use open_plex_api::library::LIBRARY_TYPE;
use open_plex_api::GlobalId;
use open_plex_core::page::MAX_ITEMS;
use open_plex_core::search::fold;
use serde::{Deserialize, Serialize};

use crate::auth::Caller;
use crate::client::ClientIdentity;
use crate::container::LibraryKind;
use crate::error::ApiError;
use crate::gql;
use crate::gql_items::{with_shared_fragments, GqlItem};
use crate::items::MetadataEntry;
use crate::language::Language;
use crate::response::{PlexResponse, ResponseFormat};
use crate::translations::Catalog;
use crate::PlexState;

pub fn router() -> Router<PlexState> {
    Router::new().route("/hubs/search", get(search))
}

/// Entries per shelf when the client does not say.
const DEFAULT_LIMIT: u32 = 3;

/// How many matches each shelf ranks before cutting to `limit`. Storage answers items in
/// title order and tags most credited first, so without this margin an exact match would
/// lose its place to whatever happens to come before it.
const RANKED_MATCHES: u32 = 100;

/// The matches, with only what ranking reads: the listing fields of a hundred items a shelf
/// would cost more than the search itself, for the three each shelf keeps.
const MATCHES_QUERY: &str = r"
query PlexHubSearch($query: String!, $items: Int!, $tags: Int!, $withTags: Boolean!) {
  movies: search(query: $query, kinds: [MOVIE], first: $items) { ...MatchedItems }
  shows: search(query: $query, kinds: [SHOW], first: $items) { ...MatchedItems }
  episodes: search(query: $query, kinds: [EPISODE], first: $items) { ...MatchedItems }
  actors: searchTags(query: $query, kinds: [ACTOR], first: $tags) @include(if: $withTags) {
    ...FoundTags
  }
  directors: searchTags(query: $query, kinds: [DIRECTOR], first: $tags) @include(if: $withTags) {
    ...FoundTags
  }
  genres: searchTags(query: $query, kinds: [GENRE], first: $tags) @include(if: $withTags) {
    ...FoundTags
  }
  collections(matching: $query, first: $tags) @include(if: $withTags) {
    edges { node {
      tagNumber title guid summary thumbUrl artUrl itemCount
      library { id title kind }
    } }
  }
}

fragment MatchedItems on ItemConnection {
  edges { node {
    id
    title
    titleSort
    ... on Movie { originalTitle }
    ... on Show { originalTitle }
    ... on Episode { originalTitle }
  } }
}

fragment FoundTags on TagUseConnection {
  edges { node {
    itemCount
    tag { id name person { providerKey thumbUrl } }
    library { id title kind }
  } }
}
";

/// The listing fields of the items each shelf keeps, in the order asked.
const ITEMS_QUERY: &str = r"
query PlexHubSearchItems($movies: [ID!]!, $shows: [ID!]!, $episodes: [ID!]!) {
  movies: nodes(ids: $movies) { ...ListedItem }
  shows: nodes(ids: $shows) { ...ListedItem }
  episodes: nodes(ids: $episodes) { ...ListedItem }
}
";

static ITEMS_QUERY_WITH_FRAGMENTS: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| with_shared_fragments(ITEMS_QUERY));

const LIBRARY_QUERY: &str = r"
query PlexHubSearchLibrary($id: ID!) { node(id: $id) { id } }
";

#[derive(Debug, Deserialize)]
struct MatchesData {
    movies: ItemMatches,
    shows: ItemMatches,
    episodes: ItemMatches,
    /// Absent for a one-character term, which finds no tags.
    #[serde(default)]
    actors: TagMatches,
    #[serde(default)]
    directors: TagMatches,
    #[serde(default)]
    genres: TagMatches,
    #[serde(default)]
    collections: CollectionMatches,
}

#[derive(Debug, Default, Deserialize)]
struct CollectionMatches {
    edges: Vec<CollectionMatchEdge>,
}

#[derive(Debug, Deserialize)]
struct CollectionMatchEdge {
    node: CollectionMatch,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CollectionMatch {
    tag_number: String,
    title: String,
    guid: String,
    summary: String,
    thumb_url: Option<String>,
    art_url: Option<String>,
    item_count: u64,
    library: Option<MatchedLibrary>,
}

#[derive(Debug, Deserialize)]
struct ItemMatches {
    edges: Vec<ItemMatchEdge>,
}

#[derive(Debug, Deserialize)]
struct ItemMatchEdge {
    node: MatchedItem,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MatchedItem {
    id: String,
    title: Option<String>,
    title_sort: Option<String>,
    original_title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ItemsData {
    movies: Vec<Option<GqlItem>>,
    shows: Vec<Option<GqlItem>>,
    episodes: Vec<Option<GqlItem>>,
}

#[derive(Debug, Default, Deserialize)]
struct TagMatches {
    edges: Vec<TagMatchEdge>,
}

#[derive(Debug, Deserialize)]
struct TagMatchEdge {
    node: TagMatch,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TagMatch {
    item_count: u64,
    tag: MatchedTag,
    library: Option<MatchedLibrary>,
}

#[derive(Debug, Deserialize)]
struct MatchedTag {
    id: String,
    name: String,
    person: Option<MatchedPerson>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MatchedPerson {
    provider_key: Option<String>,
    thumb_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MatchedLibrary {
    id: String,
    title: String,
    kind: LibraryKind,
}

#[derive(Debug, Deserialize)]
struct LibraryData {
    node: Option<serde_json::Value>,
}

/// Response to `GET /hubs/search`.
#[derive(Debug, Serialize)]
struct SearchContainer {
    size: usize,
    #[serde(rename = "Hub")]
    hubs: Vec<SearchHub>,
}

/// One shelf of results.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchHub {
    title: &'static str,
    #[serde(rename = "type")]
    kind: &'static str,
    hub_identifier: &'static str,
    context: &'static str,
    size: usize,
    /// False even when `limit` cut the shelf short, as on the reference: a search shelf has
    /// no list behind it to open.
    more: bool,
    style: &'static str,
    #[serde(rename = "Metadata", skip_serializing_if = "Vec::is_empty")]
    items: Vec<ItemResult>,
    #[serde(rename = "Directory", skip_serializing_if = "Vec::is_empty")]
    tags: Vec<TagResult>,
}

/// A film, a series or an episode, as a lookup describes it, with how well it matched.
#[derive(Debug, Serialize)]
struct ItemResult {
    /// Set when the shelf holds another result of the same title, to tell them apart by
    /// library.
    #[serde(flatten)]
    reason: Option<FoundIn>,
    score: Score,
    #[serde(flatten)]
    metadata: MetadataEntry,
}

/// Why a result is listed, Plex's `reason`: here, always the library it comes from.
#[derive(Debug, Serialize)]
struct FoundIn {
    reason: &'static str,
    #[serde(rename = "reasonID")]
    reason_id: i64,
    #[serde(rename = "reasonTitle")]
    reason_title: String,
}

impl FoundIn {
    fn library(id: i64, title: String) -> Self {
        Self {
            reason: "section",
            reason_id: id,
            reason_title: title,
        }
    }
}

/// A person or a genre in one library, leading to that library's items filtered on it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TagResult {
    key: String,
    #[serde(rename = "librarySectionID")]
    library_section_id: i64,
    library_section_key: String,
    library_section_title: String,
    library_section_type: u8,
    #[serde(flatten)]
    reason: FoundIn,
    score: Score,
    #[serde(rename = "type")]
    kind: &'static str,
    id: i64,
    filter: String,
    tag: String,
    tag_type: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    tag_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thumb: Option<String>,
    /// A collection's background, from its provider.
    #[serde(skip_serializing_if = "Option::is_none")]
    art: Option<String>,
    count: u64,
    /// A collection's, as its items name it.
    #[serde(skip_serializing_if = "Option::is_none")]
    guid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
}

/// A relevance score, which the reference writes as a string of five decimals.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
struct Score(f64);

impl Score {
    /// The rung of the ladder, which is what orders the shelves: `0.73` and `0.71` are the
    /// same rung for a film and an episode.
    fn rung(self) -> u8 {
        // Scores are tenths plus a small offset; truncating to the tenth is the rung.
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a score lies in [0, 1)"
        )]
        let tenths = (self.0 * 10.0) as u8;
        tenths
    }
}

impl Serialize for Score {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format!("{:.5}", self.0))
    }
}

/// The scores of an item shelf, from a match anywhere to an exact title.
const ITEM_SCORES: [f64; 4] = [0.33, 0.53, 0.73, 0.93];
/// An episode scores two hundredths below a film or a series matching as well.
const EPISODE_SCORES: [f64; 4] = [0.31, 0.51, 0.71, 0.91];
/// The scores of a person or a genre: a match anywhere, a name starting with the term, the
/// name itself.
const TAG_SCORES: [f64; 3] = [0.32, 0.52, 0.92];

/// The seventeen shelves, in the order the reference falls back on when their scores tie.
const SHELVES: [Shelf; 17] = [
    Shelf::Show,
    Shelf::Artist,
    Shelf::Album,
    Shelf::Movie,
    Shelf::Episode,
    Shelf::Track,
    Shelf::PhotoAlbum,
    Shelf::AutoTag,
    Shelf::Photo,
    Shelf::Tag,
    Shelf::Actor,
    Shelf::Director,
    Shelf::Genre,
    Shelf::Collection,
    Shelf::Playlist,
    Shelf::Shared,
    Shelf::Place,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shelf {
    Movie,
    Show,
    Artist,
    Album,
    Episode,
    Track,
    PhotoAlbum,
    AutoTag,
    Photo,
    Tag,
    Actor,
    Director,
    Genre,
    Collection,
    Playlist,
    Shared,
    Place,
}

impl Shelf {
    /// The shelf's `type` and `hubIdentifier`, which are the same word.
    const fn identifier(self) -> &'static str {
        match self {
            Self::Movie => "movie",
            Self::Show => "show",
            Self::Artist => "artist",
            Self::Album => "album",
            Self::Episode => "episode",
            Self::Track => "track",
            Self::PhotoAlbum => "photoalbum",
            Self::AutoTag => "autotag",
            Self::Photo => "photo",
            Self::Tag => "tag",
            Self::Actor => "actor",
            Self::Director => "director",
            Self::Genre => "genre",
            Self::Collection => "collection",
            Self::Playlist => "playlist",
            Self::Shared => "shared",
            Self::Place => "place",
        }
    }

    const fn title(self, catalog: &Catalog) -> &'static str {
        match self {
            Self::Movie => catalog.movies,
            Self::Show => catalog.shows,
            Self::Artist => catalog.artists,
            Self::Album => catalog.albums,
            Self::Episode => catalog.episodes,
            Self::Track => catalog.tracks,
            Self::PhotoAlbum => catalog.photo_albums,
            Self::AutoTag => catalog.automatic_tags,
            Self::Photo => catalog.photos,
            Self::Tag => catalog.tags,
            Self::Actor => catalog.actors,
            Self::Director => catalog.directors,
            Self::Genre => catalog.genres,
            Self::Collection => catalog.collections,
            Self::Playlist => catalog.playlists,
            Self::Shared => catalog.shared,
            Self::Place => catalog.places,
        }
    }

    fn empty(self, catalog: &Catalog) -> SearchHub {
        SearchHub {
            title: self.title(catalog),
            kind: self.identifier(),
            hub_identifier: self.identifier(),
            context: "",
            size: 0,
            more: false,
            style: "shelf",
            items: Vec::new(),
            tags: Vec::new(),
        }
    }
}

impl SearchHub {
    fn top_score(&self) -> Option<Score> {
        let items = self.items.iter().map(|result| result.score);
        let tags = self.tags.iter().map(|result| result.score);
        items
            .chain(tags)
            .reduce(|best, score| if score > best { score } else { best })
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Request {
    term: String,
    limit: u32,
    /// A library the client named, which the reference requires to exist and then does not
    /// narrow the search to.
    library: Option<i64>,
}

impl Request {
    /// A term of one character finds only the titles equal to it, and no person or genre:
    /// on the reference, `a` finds the episodes titled "A" and `x` the film "X", nothing
    /// else.
    fn is_single_character(&self) -> bool {
        fold(self.term.trim()).chars().count() == 1
    }
}

impl Request {
    /// No term, a blank one, or a `limit` that is not a positive number answer `400`. A
    /// `limit` of zero answers `500` on the reference, a fault rather than a rule.
    fn parse(query: &str) -> Result<Self, ApiError> {
        let params: HashMap<_, _> = form_urlencoded::parse(query.as_bytes()).collect();
        let term = params
            .get("query")
            .filter(|term| !term.trim().is_empty())
            .ok_or(ApiError::BadRequest)?
            .to_string();
        let limit = match params.get("limit") {
            Some(limit) => limit
                .parse()
                .ok()
                .filter(|limit| *limit > 0)
                .ok_or(ApiError::BadRequest)?,
            None => DEFAULT_LIMIT,
        };
        // Not a number, and the reference ignores it.
        let library = params
            .get("sectionId")
            .and_then(|section| section.parse().ok());
        Ok(Self {
            term,
            limit: limit.min(MAX_ITEMS),
            library,
        })
    }
}

/// The rung of a title equal to the term.
const EXACT: usize = 3;

/// Where a title ranks against the term, as an index into [`ITEM_SCORES`].
///
/// Words are split on spaces and apostrophes for this, so that `Amelie's Miracle` starts
/// with the word `amelie` while `Dune: Part Two` starts with `dune:` — a rung below `Dune :
/// Deuxième partie`, as the reference ranks them. The sort title counts for an exact match
/// (*The Batman* is `batman`), and the original title for the others: an episode carries its
/// series' there, and ranks by it.
fn item_rung(item: &MatchedItem, folded_term: &str) -> usize {
    let folded = |text: &Option<String>| text.as_deref().map(fold);
    let (title, title_sort, original) = (
        folded(&item.title),
        folded(&item.title_sort),
        folded(&item.original_title),
    );
    if [&title, &title_sort]
        .into_iter()
        .any(|text| text.as_deref() == Some(folded_term))
    {
        return EXACT;
    }
    let first_word = |text: &str| text.split([' ', '\'', '’']).next().map(ToOwned::to_owned);
    let term_word = first_word(folded_term).unwrap_or_default();
    let titles: Vec<String> = [title, original].into_iter().flatten().collect();
    if titles
        .iter()
        .any(|text| first_word(text).as_deref() == Some(term_word.as_str()))
    {
        2
    } else {
        usize::from(titles.iter().any(|text| text.starts_with(&term_word)))
    }
}

/// Where a name ranks against the term, as an index into [`TAG_SCORES`]. Unlike a title, a
/// name's rung minds its accents: `amelie` puts Amelie Kiefer above Amélie Glenn.
fn tag_rung(name: &str, term: &str) -> usize {
    let (name, term) = (name.to_lowercase(), term.to_lowercase());
    if name == term {
        2
    } else {
        usize::from(name.starts_with(&term))
    }
}

/// A shelf's matches in rank order, cut to `limit`, with the rung each reached.
fn rank_items(edges: Vec<ItemMatchEdge>, request: &Request) -> Vec<(usize, MatchedItem)> {
    let folded_term = fold(request.term.trim());
    let mut ranked: Vec<(usize, MatchedItem)> = edges
        .into_iter()
        .map(|edge| (item_rung(&edge.node, &folded_term), edge.node))
        .collect();
    // Stable, so title order survives within a rung.
    ranked.sort_by_key(|(rung, _)| Reverse(*rung));
    if request.is_single_character() {
        ranked.retain(|(rung, _)| *rung == EXACT);
    }
    ranked.truncate(request.limit as usize);
    ranked
}

/// A shelf of items from its ranked matches and their listing fields, in the same order.
fn item_hub(
    shelf: Shelf,
    ranked: &[(usize, MatchedItem)],
    items: Vec<Option<GqlItem>>,
    language: Language,
) -> SearchHub {
    let scores = if shelf == Shelf::Episode {
        EPISODE_SCORES
    } else {
        ITEM_SCORES
    };
    let mut titles: HashMap<&str, usize> = HashMap::new();
    for (_, matched) in ranked {
        *titles
            .entry(matched.title.as_deref().unwrap_or_default())
            .or_default() += 1;
    }
    let items: Vec<ItemResult> = ranked
        .iter()
        .zip(items)
        .filter_map(|((rung, matched), item)| {
            let item = item?;
            let mut metadata = MetadataEntry::from_graphql(&item, true, language)?;
            // Empty rather than absent, as `/library/search` measured on films with none.
            if metadata.kind == "movie" {
                metadata.summary.get_or_insert_with(String::new);
            }
            let shared_title = titles
                .get(matched.title.as_deref().unwrap_or_default())
                .is_some_and(|count| *count > 1);
            let reason = item
                .library
                .filter(|_| shared_title)
                .map(|library| FoundIn::library(library.database_id(), library.title));
            Some(ItemResult {
                reason,
                score: Score(scores[*rung]),
                metadata,
            })
        })
        .collect();

    SearchHub {
        size: items.len(),
        items,
        ..shelf.empty(language.catalog())
    }
}

/// A matched tag with what ranks it: its rung and numbers, decoded from the global ids.
struct RankedTag {
    rung: usize,
    id: i64,
    section: i64,
    found: TagMatch,
    library: MatchedLibrary,
}

fn tag_hub(
    shelf: Shelf,
    edges: Vec<TagMatchEdge>,
    request: &Request,
    language: Language,
) -> SearchHub {
    let (filter, tag_type) = match shelf {
        Shelf::Actor => ("actor", 6),
        Shelf::Director => ("director", 4),
        _ => ("genre", 1),
    };
    let mut ranked: Vec<RankedTag> = edges
        .into_iter()
        .filter_map(|edge| {
            let mut found = edge.node;
            let library = found.library.take()?;
            Some(RankedTag {
                rung: tag_rung(&found.tag.name, request.term.trim()),
                id: GlobalId::decode(&found.tag.id).ok()?.key,
                section: GlobalId::decode(&library.id).ok()?.key,
                found,
                library,
            })
        })
        .collect();
    // Within a rung, library by library in title order, the most credited first.
    ranked.sort_by(|left, right| {
        right
            .rung
            .cmp(&left.rung)
            .then_with(|| {
                left.library
                    .title
                    .to_lowercase()
                    .cmp(&right.library.title.to_lowercase())
            })
            .then_with(|| right.found.item_count.cmp(&left.found.item_count))
            .then_with(|| right.id.cmp(&left.id))
    });

    let tags: Vec<TagResult> = ranked
        .into_iter()
        .take(request.limit as usize)
        .map(
            |RankedTag {
                 rung,
                 id,
                 section,
                 found,
                 library,
             }| {
                let filter = format!("{filter}={id}");
                let person = found.tag.person;
                TagResult {
                    key: format!("/library/sections/{section}/all?{filter}"),
                    library_section_id: section,
                    library_section_key: format!("/library/sections/{section}"),
                    library_section_title: library.title.clone(),
                    library_section_type: library.kind.top_level_type(),
                    reason: FoundIn::library(section, library.title),
                    score: Score(TAG_SCORES[rung]),
                    kind: "tag",
                    id,
                    filter,
                    tag: found.tag.name,
                    tag_type,
                    tag_key: person
                        .as_ref()
                        .and_then(|person| person.provider_key.clone()),
                    thumb: person.and_then(|person| person.thumb_url),
                    art: None,
                    count: found.item_count,
                    guid: None,
                    summary: None,
                }
            },
        )
        .collect();

    SearchHub {
        size: tags.len(),
        tags,
        ..shelf.empty(language.catalog())
    }
}

/// A matched collection with what ranks it, as [`RankedTag`] is for a tag.
struct RankedCollection {
    rung: usize,
    number: i64,
    section: i64,
    found: CollectionMatch,
    library: MatchedLibrary,
}

/// The collections shelf: each collection in its library, ranked as a tag is, since the
/// reference lists a collection as the tag its items carry (`tagType` 2).
fn collection_hub(
    edges: Vec<CollectionMatchEdge>,
    request: &Request,
    language: Language,
) -> SearchHub {
    let mut ranked: Vec<RankedCollection> = edges
        .into_iter()
        .filter_map(|edge| {
            let mut found = edge.node;
            let library = found.library.take()?;
            Some(RankedCollection {
                rung: tag_rung(&found.title, request.term.trim()),
                number: found.tag_number.parse().ok()?,
                section: GlobalId::decode(&library.id).ok()?.key,
                found,
                library,
            })
        })
        .collect();
    // As people are ranked: library by library in title order, the largest first.
    ranked.sort_by(|left, right| {
        right
            .rung
            .cmp(&left.rung)
            .then_with(|| {
                left.library
                    .title
                    .to_lowercase()
                    .cmp(&right.library.title.to_lowercase())
            })
            .then_with(|| right.found.item_count.cmp(&left.found.item_count))
            .then_with(|| right.number.cmp(&left.number))
    });
    let tags: Vec<TagResult> = ranked
        .into_iter()
        .take(request.limit as usize)
        .map(
            |RankedCollection {
                 rung,
                 number,
                 section,
                 found,
                 library,
             }| {
                let filter = format!("collection={number}");
                TagResult {
                    key: format!("/library/sections/{section}/all?{filter}"),
                    library_section_id: section,
                    library_section_key: format!("/library/sections/{section}"),
                    library_section_title: library.title.clone(),
                    library_section_type: library.kind.top_level_type(),
                    reason: FoundIn::library(section, library.title),
                    score: Score(TAG_SCORES[rung]),
                    kind: "tag",
                    id: number,
                    filter,
                    tag: found.title,
                    tag_type: 2,
                    tag_key: None,
                    thumb: found.thumb_url,
                    art: found.art_url,
                    count: found.item_count,
                    guid: Some(found.guid),
                    summary: Some(found.summary).filter(|summary| !summary.is_empty()),
                }
            },
        )
        .collect();
    SearchHub {
        size: tags.len(),
        tags,
        ..Shelf::Collection.empty(language.catalog())
    }
}

/// The listing fields of the items kept on the film, series and episode shelves.
async fn listing_fields(
    state: &PlexState,
    [movies, shows, episodes]: [&[(usize, MatchedItem)]; 3],
) -> Result<ItemsData, ApiError> {
    if movies.is_empty() && shows.is_empty() && episodes.is_empty() {
        return Ok(ItemsData {
            movies: Vec::new(),
            shows: Vec::new(),
            episodes: Vec::new(),
        });
    }
    let ids = |ranked: &[(usize, MatchedItem)]| {
        Value::List(
            ranked
                .iter()
                .map(|(_, matched)| Value::String(matched.id.clone()))
                .collect(),
        )
    };
    let mut variables = Variables::default();
    variables.insert(Name::new("movies"), ids(movies));
    variables.insert(Name::new("shows"), ids(shows));
    variables.insert(Name::new("episodes"), ids(episodes));
    gql::execute(&state.schema, &ITEMS_QUERY_WITH_FRAGMENTS, variables).await
}

/// The library the client named, among those the caller can see.
async fn library_exists(state: &PlexState, section: i64) -> Result<bool, ApiError> {
    let mut variables = Variables::default();
    variables.insert(
        Name::new("id"),
        Value::String(GlobalId::encode(LIBRARY_TYPE, section)),
    );
    let data: LibraryData = gql::execute(&state.schema, LIBRARY_QUERY, variables).await?;
    Ok(data.node.is_some())
}

async fn search(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    RawQuery(query): RawQuery,
    format: ResponseFormat,
) -> Result<PlexResponse<SearchContainer>, ApiError> {
    let request = Request::parse(query.as_deref().unwrap_or_default())?;
    if let Some(section) = request.library {
        if !library_exists(&state, section).await? {
            return Err(ApiError::BadRequest);
        }
    }

    let mut variables = Variables::default();
    variables.insert(Name::new("query"), Value::String(request.term.clone()));
    // A single character begins thousands of words; the exact titles hide among them, in
    // title order, behind whatever starts with a digit or a sign.
    let pool = if request.is_single_character() {
        MAX_ITEMS
    } else {
        request.limit.clamp(RANKED_MATCHES, MAX_ITEMS)
    };
    variables.insert(Name::new("items"), Value::Number(pool.into()));
    variables.insert(Name::new("tags"), Value::Number(MAX_ITEMS.into()));
    variables.insert(
        Name::new("withTags"),
        Value::Boolean(!request.is_single_character()),
    );
    let matches: MatchesData = gql::execute(&state.schema, MATCHES_QUERY, variables).await?;
    let movies = rank_items(matches.movies.edges, &request);
    let shows = rank_items(matches.shows.edges, &request);
    let episodes = rank_items(matches.episodes.edges, &request);
    let items = listing_fields(&state, [&movies, &shows, &episodes]).await?;

    let language = Language::of(&client);
    let mut filled = [
        item_hub(Shelf::Movie, &movies, items.movies, language),
        item_hub(Shelf::Show, &shows, items.shows, language),
        item_hub(Shelf::Episode, &episodes, items.episodes, language),
        tag_hub(Shelf::Actor, matches.actors.edges, &request, language),
        tag_hub(Shelf::Director, matches.directors.edges, &request, language),
        tag_hub(Shelf::Genre, matches.genres.edges, &request, language),
        collection_hub(matches.collections.edges, &request, language),
    ]
    .map(Some);
    let mut hubs: Vec<SearchHub> = SHELVES
        .iter()
        .map(|shelf| {
            filled
                .iter_mut()
                .find(|hub| {
                    hub.as_ref()
                        .is_some_and(|hub| hub.hub_identifier == shelf.identifier())
                })
                .and_then(Option::take)
                .unwrap_or_else(|| shelf.empty(language.catalog()))
        })
        .collect();
    // Stable: shelves on the same rung keep the reference's order, and empty ones sink.
    hubs.sort_by_key(|hub| Reverse(hub.top_score().map_or(0, Score::rung)));

    Ok(PlexResponse::new(
        SearchContainer {
            size: hubs.len(),
            hubs,
        },
        format,
    ))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn matched(
        id: i64,
        title: &str,
        title_sort: Option<&str>,
        original: Option<&str>,
    ) -> MatchedItem {
        MatchedItem {
            id: GlobalId::encode("Item", id),
            title: Some(title.to_owned()),
            title_sort: title_sort.map(ToOwned::to_owned),
            original_title: original.map(ToOwned::to_owned),
        }
    }

    #[test]
    fn a_search_needs_a_term_and_a_usable_limit() {
        for query in [
            "limit=3",
            "query=",
            "query=%20",
            "query=dune&limit=abc",
            "query=dune&limit=0",
        ] {
            assert!(
                matches!(Request::parse(query), Err(ApiError::BadRequest)),
                "{query}"
            );
        }
        let request = Request::parse("query=dune&sectionId=abc").unwrap();
        assert_eq!((request.limit, request.library), (3, None));
        let request = Request::parse("query=dune&limit=9999&sectionId=6").unwrap();
        assert_eq!((request.limit, request.library), (MAX_ITEMS, Some(6)));
    }

    #[test]
    fn ranks_titles_on_the_references_ladder() {
        let rung = |title, sort, original, term: &str| {
            item_rung(&matched(1, title, sort, original), &fold(term))
        };
        // Scores measured for `dune`, `batman` and `amélie`.
        assert_eq!(rung("Dune", None, None, "dune"), 3);
        assert_eq!(rung("The Batman", Some("Batman"), None, "batman"), 3);
        assert_eq!(
            rung(
                "Dune : Deuxième partie",
                None,
                Some("Dune: Part Two"),
                "dune"
            ),
            2
        );
        assert_eq!(
            rung("Amelie's Miracle", None, Some("Amelie rennt"), "amélie"),
            2
        );
        assert_eq!(rung("Le Fléau", None, Some("Batman Beyond"), "batman"), 2);
        assert_eq!(rung("Dune: Part Two", None, None, "dune"), 1);
        assert_eq!(rung("Breaking Bad", None, None, "breaking b"), 2);
        assert_eq!(
            rung("Les Nouvelles aventures de Batman", None, None, "batman"),
            0
        );
        assert_eq!(rung("Legend", None, None, "le"), 1);
    }

    #[test]
    fn ranks_names_minding_their_accents() {
        assert_eq!(tag_rung("Tom Hanks", "tom hanks"), 2);
        assert_eq!(tag_rung("Tom Hanks", "tom"), 1);
        assert_eq!(tag_rung("Tommy Lee Jones", "tom"), 1);
        assert_eq!(tag_rung("Cécile de France", "france"), 0);
        assert_eq!(tag_rung("Amelie Kiefer", "amelie"), 1);
        assert_eq!(tag_rung("Amélie Glenn", "amelie"), 0);
    }

    #[test]
    fn names_the_library_of_results_sharing_a_title() {
        let listed = |id: i64, title: &str, section: i64, library: &str| -> Option<GqlItem> {
            serde_json::from_value(serde_json::json!({
                "__typename": "Movie",
                "id": GlobalId::encode("Item", id),
                "guid": "plex://movie/5d7768273c3c2a001fbcb51a",
                "title": title,
                "addedAt": 0,
                "updatedAt": 0,
                "library": {
                    "id": GlobalId::encode(LIBRARY_TYPE, section),
                    "title": library,
                    "uuid": "uuid",
                    "kind": "MOVIE",
                },
            }))
            .ok()
        };
        // As captured for `dune`: the two copies of Dune name their library, the sequel
        // alone on the shelf does not.
        let copies = [
            (470_473, "Dune", 4, "VF - Films"),
            (470_476, "Dune", 6, "VO - Movies"),
            (440_230, "Dune : Deuxième partie", 17, "VF - Films UHD"),
        ];
        let ranked: Vec<(usize, MatchedItem)> = copies
            .iter()
            .map(|&(id, title, _, _)| (EXACT, matched(id, title, None, None)))
            .collect();
        let items = copies
            .iter()
            .map(|&(id, title, section, library)| listed(id, title, section, library))
            .collect();
        let hub = item_hub(Shelf::Movie, &ranked, items, Language::default());
        let reasons: Vec<Option<(i64, &str)>> = hub
            .items
            .iter()
            .map(|result| {
                result
                    .reason
                    .as_ref()
                    .map(|found| (found.reason_id, found.reason_title.as_str()))
            })
            .collect();
        assert_eq!(
            reasons,
            [Some((4, "VF - Films")), Some((6, "VO - Movies")), None]
        );
    }

    #[test]
    fn ranks_then_cuts_to_the_limit() {
        let edges = || {
            vec![
                ItemMatchEdge {
                    node: matched(440_230, "Dune : Deuxième partie", None, None),
                },
                ItemMatchEdge {
                    node: matched(470_473, "Dune", None, None),
                },
                ItemMatchEdge {
                    node: matched(470_476, "Dune", None, None),
                },
            ]
        };
        let ids = |ranked: Vec<(usize, MatchedItem)>| -> Vec<(usize, String)> {
            ranked
                .into_iter()
                .map(|(rung, item)| (rung, item.id))
                .collect()
        };
        let request = Request::parse("query=dune&limit=2").unwrap();
        assert_eq!(
            ids(rank_items(edges(), &request)),
            [
                (EXACT, GlobalId::encode("Item", 470_473)),
                (EXACT, GlobalId::encode("Item", 470_476))
            ]
        );
        // One character keeps the exact titles only.
        let request = Request::parse("query=x").unwrap();
        let edges = vec![
            ItemMatchEdge {
                node: matched(411_328, "X", None, None),
            },
            ItemMatchEdge {
                node: matched(1, "X-Men", None, None),
            },
        ];
        assert_eq!(
            ids(rank_items(edges, &request)),
            [(EXACT, GlobalId::encode("Item", 411_328))]
        );
    }

    #[test]
    fn orders_people_by_rung_library_count_then_newest_tag() {
        let found = |id: i64, name: &str, section: i64, library: &str, count: u64| {
            serde_json::from_value::<TagMatchEdge>(serde_json::json!({ "node": {
                "itemCount": count,
                "tag": { "id": GlobalId::encode("Tag", id), "name": name, "person": null },
                "library": {
                    "id": GlobalId::encode(LIBRARY_TYPE, section),
                    "title": library,
                    "kind": "MOVIE",
                },
            } }))
            .unwrap()
        };
        // As the reference lists `besson`: Luc first for his count, then the ones credited
        // once, newest tag first, and the other libraries after, in title order.
        let edges = vec![
            found(473_528, "Luc Besson", 6, "VO - Movies", 5),
            found(586_784, "Thalia Besson", 4, "VF - Films", 1),
            found(907_718, "Emma Besson", 4, "VF - Films", 1),
            found(555_365, "Luc Besson", 4, "VF - Films", 3),
        ];
        let request = Request::parse("query=besson&limit=10").unwrap();
        let hub = tag_hub(Shelf::Actor, edges, &request, Language::default());
        let order: Vec<(i64, i64)> = hub
            .tags
            .iter()
            .map(|tag| (tag.id, tag.library_section_id))
            .collect();
        assert_eq!(
            order,
            [(555_365, 4), (907_718, 4), (586_784, 4), (473_528, 6)]
        );
    }

    #[test]
    fn writes_a_score_with_five_decimals_and_reads_its_rung() {
        assert_eq!(serde_json::to_string(&Score(0.93)).unwrap(), "\"0.93000\"");
        assert_eq!(Score(0.93).rung(), 9);
        assert_eq!(Score(0.71).rung(), 7);
        assert_eq!(Score(0.31).rung(), 3);
    }
}
