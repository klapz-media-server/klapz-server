//! `GET /hubs/search`: a search answered as shelves, one per kind of result.
//!
//! What Plex Web's search page and most other clients call. The reference always answers the
//! same seventeen shelves, filled or empty, of which this server can fill seven: films,
//! series, episodes, actors, directors, genres and collections. Music, photos, playlists and
//! the rest stay empty, as they would on a server holding none.
//!
//! Matching and ranking are the official API's: `search`, ranked by its score, the score the
//! reference gives, and `searchTags`; a film or series shelf no title fills is filled through
//! the people the term names (`searchCredits`), as the reference fills it. Each shelf is cut
//! to `limit`. `docs/plex/spec-gaps.md` lists the rules and where they stop.

use std::cmp::Reverse;
use std::collections::HashMap;

use async_graphql::{Name, Value, Variables};
use axum::extract::{RawQuery, State};
use axum::routing::get;
use axum::Router;
use klapz_api::library::LIBRARY_TYPE;
use klapz_api::GlobalId;
use klapz_core::page::MAX_ITEMS;
use klapz_core::search::{fold, name_rung, name_score, tag_order, ListedTag, Rung};
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

const MATCHES_QUERY: &str = r"
query PlexHubSearch($query: String!, $items: Int!, $tags: Int!, $withTags: Boolean!) {
  movies: search(query: $query, kinds: [MOVIE], first: $items) { ...FoundItems }
  shows: search(query: $query, kinds: [SHOW], first: $items) { ...FoundItems }
  episodes: search(query: $query, kinds: [EPISODE], episodesBySeries: true, first: $items) {
    ...FoundItems
  }
  actors: searchTags(query: $query, kinds: [ACTOR], first: $tags) @include(if: $withTags) {
    ...FoundTags
  }
  directors: searchTags(query: $query, kinds: [DIRECTOR], first: $tags) @include(if: $withTags) {
    ...FoundTags
  }
  genres: searchTags(query: $query, kinds: [GENRE], first: $tags) @include(if: $withTags) {
    ...FoundTags
  }
  collections(matching: $query, first: $tags) @include(if: $withTags) { ...FoundCollections }
}
";

/// The film and series shelves no title filled, from the people the term names.
const CREDITS_QUERY: &str = r"
query PlexHubSearchCredits($query: String!, $items: Int!, $movies: Boolean!, $shows: Boolean!) {
  movies: searchCredits(query: $query, kind: MOVIE, first: $items) @include(if: $movies) {
    ...CreditedItems
  }
  shows: searchCredits(query: $query, kind: SHOW, first: $items) @include(if: $shows) {
    ...CreditedItems
  }
}

fragment CreditedItems on CreditedItemConnection {
  edges { score credit { tag { id name kind } } node { ...ListedItem } }
}
";

/// What a search finds, as `/library/search` asks for it too.
pub(crate) const SEARCH_FRAGMENTS: &str = r"
fragment FoundItems on ItemSearchConnection {
  edges { score titleMatch node { ...ListedItem } }
}

fragment FoundTags on TagUseConnection {
  edges { node {
    itemCount
    tag { id name person { providerKey thumbUrl } }
    library { id title kind }
  } }
}

fragment FoundCollections on CollectionConnection {
  edges { node {
    tagNumber title guid summary thumbUrl artUrl itemCount
    library { id title kind }
  } }
}
";

static MATCHES_QUERY_WITH_FRAGMENTS: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    with_shared_fragments(&format!("{MATCHES_QUERY}{SEARCH_FRAGMENTS}"))
});

static CREDITS_QUERY_WITH_FRAGMENTS: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| with_shared_fragments(CREDITS_QUERY));

const LIBRARY_QUERY: &str = r"
query PlexHubSearchLibrary($id: ID!) { node(id: $id) { id } }
";

#[derive(Debug, Deserialize)]
struct MatchesData {
    movies: FoundItems,
    shows: FoundItems,
    episodes: FoundItems,
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

/// Items `search` found, ranked, with their scores.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct FoundItems {
    pub(crate) edges: Vec<FoundItem>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FoundItem {
    pub(crate) score: f64,
    pub(crate) title_match: TitleMatch,
    pub(crate) node: GqlItem,
}

/// Mirrors the official API's `TitleMatch`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum TitleMatch {
    Exact,
    LeadingWord,
    LeadingPrefix,
    Anywhere,
}

#[derive(Debug, Default, Deserialize)]
struct CreditsData {
    #[serde(default)]
    movies: CreditedItems,
    #[serde(default)]
    shows: CreditedItems,
}

#[derive(Debug, Default, Deserialize)]
struct CreditedItems {
    edges: Vec<CreditedItem>,
}

#[derive(Debug, Deserialize)]
struct CreditedItem {
    score: f64,
    credit: Credit,
    node: GqlItem,
}

#[derive(Debug, Deserialize)]
struct Credit {
    tag: CreditTag,
}

#[derive(Debug, Deserialize)]
struct CreditTag {
    id: String,
    name: String,
    kind: CreditKind,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum CreditKind {
    Actor,
    Director,
    #[serde(other)]
    Other,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct CollectionMatches {
    pub(crate) edges: Vec<CollectionMatchEdge>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CollectionMatchEdge {
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

#[derive(Debug, Default, Deserialize)]
pub(crate) struct TagMatches {
    pub(crate) edges: Vec<TagMatchEdge>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TagMatchEdge {
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
    /// library, and on what was found through a person, naming them.
    #[serde(flatten)]
    reason: Option<FoundIn>,
    score: Score,
    #[serde(flatten)]
    metadata: MetadataEntry,
}

/// Why a result is listed, Plex's `reason`: the library it comes from, or the person it was
/// found through.
#[derive(Debug, Serialize)]
pub(crate) struct FoundIn {
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

/// A person, a genre or a collection in one library, leading to that library's items
/// filtered on it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TagResult {
    key: String,
    #[serde(rename = "librarySectionID")]
    library_section_id: i64,
    library_section_key: String,
    library_section_title: String,
    library_section_type: u8,
    /// The library again, on a shelf; `/library/search` carries none.
    #[serde(flatten)]
    reason: Option<FoundIn>,
    /// On a shelf; `/library/search` writes it around the entry instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    score: Option<Score>,
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

impl TagResult {
    /// The library kind whose items it leads to.
    pub(crate) const fn library_type(&self) -> u8 {
        self.library_section_type
    }

    /// The entry as `/library/search` writes it, with its score apart and no library
    /// repeated as a reason.
    pub(crate) fn into_bare(mut self) -> (f64, Self) {
        let score = self.score.take().map_or(0.0, |score| score.0);
        self.reason = None;
        (score, self)
    }
}

/// A relevance score, which the reference writes as a string of five decimals.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub(crate) struct Score(f64);

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

/// Plex's `tagType` of a collection.
const COLLECTION_TAG_TYPE: u8 = 2;

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
pub(crate) enum Shelf {
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
        let tags = self.tags.iter().filter_map(|result| result.score);
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

/// A shelf of items, in the order `search` ranked them.
///
/// A single-character term keeps only the titles equal to it: on the reference, `a` finds
/// the episodes titled "A" and `x` the film "X", nothing else.
fn item_hub(
    shelf: Shelf,
    found: Vec<FoundItem>,
    request: &Request,
    language: Language,
) -> SearchHub {
    let found: Vec<FoundItem> = found
        .into_iter()
        .filter(|item| !request.is_single_character() || item.title_match == TitleMatch::Exact)
        .take(request.limit as usize)
        .collect();
    let mut titles: HashMap<&str, usize> = HashMap::new();
    for item in &found {
        *titles
            .entry(item.node.title.as_deref().unwrap_or_default())
            .or_default() += 1;
    }
    let shared_titles: Vec<bool> = found
        .iter()
        .map(|item| titles[item.node.title.as_deref().unwrap_or_default()] > 1)
        .collect();
    let items = found
        .into_iter()
        .zip(shared_titles)
        .filter_map(|(found, shared_title)| {
            let reason = found
                .node
                .library
                .as_ref()
                .filter(|_| shared_title)
                .map(|library| FoundIn::library(library.database_id(), library.title.clone()));
            item_result(&found.node, reason, found.score, language)
        })
        .collect();
    filled(shelf, items, language)
}

/// A film or series shelf from the works of the people the term names.
fn credited_hub(shelf: Shelf, found: CreditedItems, language: Language) -> SearchHub {
    let items = found
        .edges
        .into_iter()
        .filter_map(|found| {
            let reason = match found.credit.tag.kind {
                CreditKind::Actor => "actor",
                CreditKind::Director => "director",
                CreditKind::Other => return None,
            };
            let reason = FoundIn {
                reason,
                reason_id: GlobalId::decode(&found.credit.tag.id).ok()?.key,
                reason_title: found.credit.tag.name,
            };
            item_result(&found.node, Some(reason), found.score, language)
        })
        .collect();
    filled(shelf, items, language)
}

fn item_result(
    item: &GqlItem,
    reason: Option<FoundIn>,
    score: f64,
    language: Language,
) -> Option<ItemResult> {
    let mut metadata = MetadataEntry::from_graphql(item, true, language)?;
    // Empty rather than absent, as `/library/search` measured on films with none.
    if metadata.kind == "movie" {
        metadata.summary.get_or_insert_with(String::new);
    }
    Some(ItemResult {
        reason,
        score: Score(score),
        metadata,
    })
}

fn filled(shelf: Shelf, items: Vec<ItemResult>, language: Language) -> SearchHub {
    SearchHub {
        size: items.len(),
        items,
        ..shelf.empty(language.catalog())
    }
}

/// A matched tag with what ranks it: its rung and numbers, decoded from the global ids.
struct RankedTag {
    rung: Rung,
    id: i64,
    section: i64,
    found: TagMatch,
    library: MatchedLibrary,
}

impl RankedTag {
    fn listed(&self) -> ListedTag<'_> {
        ListedTag {
            name: &self.found.tag.name,
            library_title: &self.library.title,
            count: self.found.item_count,
            id: self.id,
        }
    }
}

/// The people or genres of a shelf, `shelf` naming which, in `tag_order` and cut to
/// `limit`.
pub(crate) fn ranked_tags(
    shelf: Shelf,
    edges: Vec<TagMatchEdge>,
    term: &str,
    limit: u32,
) -> Vec<TagResult> {
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
                rung: name_rung(&found.tag.name, term),
                id: GlobalId::decode(&found.tag.id).ok()?.key,
                section: GlobalId::decode(&library.id).ok()?.key,
                found,
                library,
            })
        })
        .collect();
    ranked.sort_by(|left, right| tag_order(term, &left.listed(), &right.listed()));

    ranked
        .into_iter()
        .take(limit as usize)
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
                    reason: Some(FoundIn::library(section, library.title)),
                    score: Some(Score(name_score(rung))),
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
        .collect()
}

/// A matched collection with what ranks it, as [`RankedTag`] is for a tag.
struct RankedCollection {
    rung: Rung,
    number: i64,
    section: i64,
    found: CollectionMatch,
    library: MatchedLibrary,
}

impl RankedCollection {
    fn listed(&self) -> ListedTag<'_> {
        ListedTag {
            name: &self.found.title,
            library_title: &self.library.title,
            count: self.found.item_count,
            id: self.number,
        }
    }
}

/// The collections, each in its library, ranked as a tag is, since the reference lists a
/// collection as the tag its items carry (`tagType` 2).
pub(crate) fn ranked_collections(
    edges: Vec<CollectionMatchEdge>,
    term: &str,
    limit: u32,
) -> Vec<TagResult> {
    let mut ranked: Vec<RankedCollection> = edges
        .into_iter()
        .filter_map(|edge| {
            let mut found = edge.node;
            let library = found.library.take()?;
            Some(RankedCollection {
                rung: name_rung(&found.title, term),
                number: found.tag_number.parse().ok()?,
                section: GlobalId::decode(&library.id).ok()?.key,
                found,
                library,
            })
        })
        .collect();
    // As people are ranked: library by library in title order, the largest first.
    ranked.sort_by(|left, right| tag_order(term, &left.listed(), &right.listed()));
    ranked
        .into_iter()
        .take(limit as usize)
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
                    reason: Some(FoundIn::library(section, library.title)),
                    score: Some(Score(name_score(rung))),
                    kind: "tag",
                    id: number,
                    filter,
                    tag: found.title,
                    tag_type: COLLECTION_TAG_TYPE,
                    tag_key: None,
                    thumb: found.thumb_url,
                    art: found.art_url,
                    count: found.item_count,
                    guid: Some(found.guid),
                    summary: Some(found.summary).filter(|summary| !summary.is_empty()),
                }
            },
        )
        .collect()
}

fn tag_hub(shelf: Shelf, tags: Vec<TagResult>, language: Language) -> SearchHub {
    SearchHub {
        size: tags.len(),
        tags,
        ..shelf.empty(language.catalog())
    }
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

/// The works of the people the term names, for the film and series shelves no title filled.
async fn credited(
    state: &PlexState,
    request: &Request,
    movies: bool,
    shows: bool,
) -> Result<CreditsData, ApiError> {
    if !(movies || shows) {
        return Ok(CreditsData::default());
    }
    let mut variables = Variables::default();
    variables.insert(Name::new("query"), Value::String(request.term.clone()));
    variables.insert(Name::new("items"), Value::Number(request.limit.into()));
    variables.insert(Name::new("movies"), Value::Boolean(movies));
    variables.insert(Name::new("shows"), Value::Boolean(shows));
    gql::execute(&state.schema, &CREDITS_QUERY_WITH_FRAGMENTS, variables).await
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

    let with_tags = !request.is_single_character();
    let mut variables = Variables::default();
    variables.insert(Name::new("query"), Value::String(request.term.clone()));
    variables.insert(Name::new("items"), Value::Number(request.limit.into()));
    variables.insert(Name::new("tags"), Value::Number(MAX_ITEMS.into()));
    variables.insert(Name::new("withTags"), Value::Boolean(with_tags));
    let matches: MatchesData =
        gql::execute(&state.schema, &MATCHES_QUERY_WITH_FRAGMENTS, variables).await?;
    let credits = credited(
        &state,
        &request,
        with_tags && matches.movies.edges.is_empty(),
        with_tags && matches.shows.edges.is_empty(),
    )
    .await?;

    let language = Language::of(&client);
    let term = request.term.trim();
    let item_shelf = |shelf, found: FoundItems, credited: CreditedItems| {
        if found.edges.is_empty() {
            credited_hub(shelf, credited, language)
        } else {
            item_hub(shelf, found.edges, &request, language)
        }
    };
    let mut filled = [
        item_shelf(Shelf::Movie, matches.movies, credits.movies),
        item_shelf(Shelf::Show, matches.shows, credits.shows),
        item_hub(Shelf::Episode, matches.episodes.edges, &request, language),
        tag_hub(
            Shelf::Actor,
            ranked_tags(Shelf::Actor, matches.actors.edges, term, request.limit),
            language,
        ),
        tag_hub(
            Shelf::Director,
            ranked_tags(
                Shelf::Director,
                matches.directors.edges,
                term,
                request.limit,
            ),
            language,
        ),
        tag_hub(
            Shelf::Genre,
            ranked_tags(Shelf::Genre, matches.genres.edges, term, request.limit),
            language,
        ),
        tag_hub(
            Shelf::Collection,
            ranked_collections(matches.collections.edges, term, request.limit),
            language,
        ),
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

    fn item_json(id: i64, title: &str, section: i64, library: &str) -> serde_json::Value {
        serde_json::json!({
            "__typename": "Movie",
            "id": GlobalId::encode("Item", id),
            "guid": "plex://movie/5d7768273c3c2a001fbcb51a",
            "title": title,
            "addedAt": "1970-01-01T00:00:00Z",
            "updatedAt": "1970-01-01T00:00:00Z",
            "library": {
                "id": GlobalId::encode(LIBRARY_TYPE, section),
                "title": library,
                "uuid": "uuid",
                "kind": "MOVIE",
            },
        })
    }

    fn found(id: i64, title: &str, section: i64, library: &str, title_match: &str) -> FoundItem {
        serde_json::from_value(serde_json::json!({
            "score": 0.93,
            "titleMatch": title_match,
            "node": item_json(id, title, section, library),
        }))
        .unwrap()
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
    fn names_the_library_of_results_sharing_a_title() {
        // As captured for `dune`: the two copies of Dune name their library, the sequel
        // alone on the shelf does not.
        let shelf = vec![
            found(470_473, "Dune", 4, "VF - Films", "EXACT"),
            found(470_476, "Dune", 6, "VO - Movies", "EXACT"),
            found(
                440_230,
                "Dune : Deuxième partie",
                17,
                "VF - Films UHD",
                "LEADING_WORD",
            ),
        ];
        let request = Request::parse("query=dune&limit=10").unwrap();
        let hub = item_hub(Shelf::Movie, shelf, &request, Language::default());
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
    fn cuts_to_the_limit_and_keeps_exact_titles_for_one_character() {
        let request = Request::parse("query=dune&limit=2").unwrap();
        let shelf = vec![
            found(470_473, "Dune", 4, "VF - Films", "EXACT"),
            found(470_476, "Dune", 6, "VO - Movies", "EXACT"),
            found(
                440_230,
                "Dune : Deuxième partie",
                17,
                "VF - Films UHD",
                "LEADING_WORD",
            ),
        ];
        assert_eq!(
            item_hub(Shelf::Movie, shelf, &request, Language::default()).size,
            2
        );
        let request = Request::parse("query=x").unwrap();
        let shelf = vec![
            found(411_328, "X", 4, "VF - Films", "EXACT"),
            found(1, "X-Men", 4, "VF - Films", "LEADING_PREFIX"),
        ];
        let hub = item_hub(Shelf::Movie, shelf, &request, Language::default());
        assert_eq!(hub.items.len(), 1);
        assert_eq!(hub.items[0].metadata.title.as_deref(), Some("X"));
    }

    #[test]
    fn names_the_person_an_item_was_found_through() {
        let credited: CreditedItems = serde_json::from_value(serde_json::json!({ "edges": [{
            "score": 0.33097,
            "credit": { "tag": {
                "id": GlobalId::encode("Tag", 469_791), "name": "Tom Hanks", "kind": "ACTOR",
            } },
            "node": item_json(418_098, "Le Pire voisin au monde", 4, "VF - Films"),
        }] }))
        .unwrap();
        let hub = credited_hub(Shelf::Movie, credited, Language::default());
        let result = &hub.items[0];
        let reason = result.reason.as_ref().unwrap();
        assert_eq!(
            (
                reason.reason,
                reason.reason_id,
                reason.reason_title.as_str()
            ),
            ("actor", 469_791, "Tom Hanks")
        );
        assert_eq!(serde_json::to_value(result.score).unwrap(), "0.33097");
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
        let tags = ranked_tags(Shelf::Actor, edges, "besson", 10);
        let order: Vec<(i64, i64)> = tags
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
