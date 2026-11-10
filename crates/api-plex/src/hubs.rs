//! `GET /hubs/sections/{id}`: the shelves of a library's "Recommended" tab.
//!
//! The reference builds seven shelves for a film library and eight for a series library. A
//! shelf is served when it can be answered truthfully, and absent otherwise — an invented
//! shelf would show wrong films under a title that promises something.
//!
//! Served, in the reference's order:
//!
//! - **Continue watching**: the library's films in progress, or its series' next episodes,
//!   by the rules `core::watch` measured.
//! - **Recently released**, films out within a year or episodes by air date.
//! - **Recently added**: a film library's films by date added; a series library's new
//!   episodes grouped into their season or series, by the rule `core::recent` measured.
//! - **Top of a genre** and **top of a person**, whose genre or person is drawn among the
//!   library's.
//! - **Start watching**, the series not begun; **rediscover**, the series watched between
//!   five years and six weeks ago and not finished; **top unwatched** films and **top rated**
//!   series, rated 7 or more by their audience; **recently watched** films or episodes.
//! - **Recent playlists**, empty, when the client asks for library playlists: there are
//!   none.
//!
//! Left out: a series library's "more in" a genre. Its genre is not drawn evenly — four of 45
//! in 120 draws, all genres of series watched lately, Drama never — by a rule not found yet.

use async_graphql::{Name, Value, Variables};
use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::Router;
use open_plex_api::library::LIBRARY_TYPE;
use open_plex_api::GlobalId;
use open_plex_core::page::MAX_ITEMS;
use serde::Serialize;

use crate::auth::Caller;
use crate::client::ClientIdentity;
use crate::error::ApiError;
use crate::gql;
use crate::gql_items::{
    ContinueWatchingData, GqlContinueEdge, GqlRecentEdge, GqlRecentlyAdded, HomeRecentData,
    LibraryRecentData, RecentlyAddedData, ShelfData, CONTINUE_WATCHING_QUERY, HOME_RECENT_QUERY,
    LIBRARY_RECENT_QUERY, RECENTLY_ADDED_QUERY, SHELF_QUERY,
};
use crate::items::{MetadataEntry, LIBRARY_IDENTIFIER};
use crate::language::Language;
use crate::response::{PlexResponse, ResponseFormat};
use crate::translations::Catalog;
use crate::PlexState;

pub fn router() -> Router<PlexState> {
    Router::new()
        .route("/hubs/sections/{sectionId}", get(section_hubs))
        .route("/hubs/continueWatching", get(continue_watching_hub))
        .route("/hubs/continueWatching/items", get(continue_watching_items))
        .route("/hubs/home/continueWatching", get(home_in_progress))
        .route("/hubs/home/onDeck", get(home_up_next))
        .route("/hubs/home/recentlyAdded", get(home_recently_added))
        .route(
            "/hubs/sections/{sectionId}/continueWatching/items",
            get(section_continue_watching_items),
        )
}

/// Items per shelf when the client does not say, or says something that is not a number:
/// `count=abc` gets six on the reference.
const DEFAULT_COUNT: u32 = 6;

/// Response to `GET /hubs/sections/{id}`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HubsContainer {
    pub size: u32,
    pub allow_sync: bool,
    pub identifier: &'static str,
    #[serde(rename = "librarySectionID")]
    pub library_section_id: i64,
    pub library_section_title: String,
    #[serde(rename = "librarySectionUUID")]
    pub library_section_uuid: String,
    #[serde(rename = "Hub", skip_serializing_if = "Vec::is_empty")]
    pub hub: Vec<Hub>,
}

/// One shelf.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hub {
    /// The shelf's items as one lookup, `/library/metadata/1,2,3`. Absent on an empty shelf.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hub_key: Option<String>,
    /// Where the client goes for the whole list behind the shelf.
    pub key: String,
    pub title: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub hub_identifier: String,
    pub context: &'static str,
    pub size: u32,
    /// Whether the list behind the shelf holds more than the shelf shows.
    pub more: bool,
    /// Set on a shelf that samples its list rather than showing its head.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub random: Option<bool>,
    pub style: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub promoted: Option<bool>,
    #[serde(rename = "Metadata", skip_serializing_if = "Vec::is_empty")]
    pub metadata: Vec<MetadataEntry>,
}

/// The parameters this endpoint reads. The many others Plex Web sends —
/// `includeStations`, `includeExternalMetadata` and the like — ask for content this server
/// does not have, and an absent shelf is already the answer.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HubParams {
    count: Option<String>,
    #[serde(rename = "includeLibraryPlaylists")]
    include_library_playlists: Option<String>,
    #[serde(rename = "excludeFields")]
    exclude_fields: Option<String>,
}

impl HubParams {
    fn count(&self) -> u32 {
        self.count_or(DEFAULT_COUNT)
    }

    /// The client's `count`, else `default`, within the ceiling every window respects.
    fn count_or(&self, default: u32) -> u32 {
        self.count
            .as_deref()
            .and_then(|count| count.parse().ok())
            .unwrap_or(default)
            .min(MAX_ITEMS)
    }

    fn includes_playlists(&self) -> bool {
        self.include_library_playlists.as_deref() == Some("1")
    }

    /// `excludeFields=summary` drops the synopsis from every item, which the reference
    /// honors and Plex Web asks for: a shelf shows no synopsis.
    fn excludes_summary(&self) -> bool {
        self.exclude_fields
            .as_deref()
            .is_some_and(|fields| fields.split(',').any(|field| field == "summary"))
    }
}

/// `GET /hubs/sections/{id}`.
async fn section_hubs(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(section): Path<i64>,
    Query(params): Query<HubParams>,
    client: ClientIdentity,
    format: ResponseFormat,
) -> Result<PlexResponse<HubsContainer>, ApiError> {
    let count = params.count();
    let language = Language::of(&client);
    let library = recently_added(&state, section, count).await?;
    let is_film_library = library.kind == "MOVIE";

    let mut items: Vec<MetadataEntry> = library
        .recently_added
        .edges
        .iter()
        .filter_map(|edge| {
            let mut item = MetadataEntry::from_graphql(&edge.node, true, language)?;
            // The entry's time, which for a season gathered from a batch is its newest
            // episode's rather than the season's own.
            item.added_at = edge.added_at;
            Some(item)
        })
        .collect();
    if params.excludes_summary() {
        for item in &mut items {
            item.summary = None;
        }
    }
    let shelf = RecentShelf {
        section,
        library_title: &library.title,
        is_film_library,
        more: library.recently_added.page_info.has_next_page,
    };

    let started = continue_watching(&state, Some(section), count).await?;
    let mut hubs = vec![continue_watching_shelf(
        section,
        is_film_library,
        language,
        &started,
    )];
    let (before, after) = if is_film_library {
        (FILM_SHELVES_BEFORE, FILM_SHELVES_AFTER)
    } else {
        (SERIES_SHELVES_BEFORE, SERIES_SHELVES_AFTER)
    };
    let prefix = if is_film_library { "movie" } else { "tv" };
    for source in before {
        if let Some(spec) = source.spec(&state, section, prefix, language).await? {
            hubs.push(listing_shelf(&state, section, &spec, &params, language).await?);
        }
    }
    hubs.push(shelf.hub(items, language));
    for source in after {
        if let Some(spec) = source.spec(&state, section, prefix, language).await? {
            hubs.push(listing_shelf(&state, section, &spec, &params, language).await?);
        }
    }
    if params.includes_playlists() {
        hubs.push(recent_playlists(section, language));
    }

    Ok(PlexResponse::new(
        HubsContainer {
            size: u32::try_from(hubs.len()).unwrap_or(u32::MAX),
            allow_sync: true,
            identifier: LIBRARY_IDENTIFIER,
            library_section_id: section,
            library_section_title: library.title.clone(),
            library_section_uuid: library.uuid.clone(),
            hub: hubs,
        },
        format,
    ))
}

/// A shelf showing a library listing: the head of the list its `key` opens, or a sample of
/// it for a `random` shelf.
#[derive(Debug)]
struct ListingShelf {
    /// The middle of `hubIdentifier`: `recentlyreleased`.
    name: &'static str,
    context: &'static str,
    kind: &'static str,
    title: fn(&'static Catalog) -> &'static str,
    /// The listing's query, worded as the reference words it. It is also what fills the
    /// shelf, read by the same parser as `/all`, so the shelf cannot show what its list
    /// does not hold.
    query: &'static str,
    random: bool,
}

const RECENTLY_RELEASED_MOVIES: ListingShelf = ListingShelf {
    name: "recentlyreleased",
    context: "hub.movie.recentlyreleased",
    kind: "movie",
    title: |c| c.recently_released_movies,
    query: "sort=originallyAvailableAt:desc&originallyAvailableAt>=-1y",
    random: false,
};

const TOP_UNWATCHED_MOVIES: ListingShelf = ListingShelf {
    name: "topunwatched",
    context: "hub.movie.topunwatched",
    kind: "movie",
    title: |c| c.top_unwatched_movies,
    query: "unwatched=1&audienceRating>=7.0&sort=audienceRating:desc",
    random: true,
};

const RECENTLY_WATCHED_MOVIES: ListingShelf = ListingShelf {
    name: "recentlyviewed",
    context: "hub.movie.recentlyviewed",
    kind: "movie",
    title: |c| c.recently_watched_movies,
    query: "sort=lastViewedAt:desc&unwatched=0&viewOffset=0",
    random: false,
};

const RECENTLY_AIRED: ListingShelf = ListingShelf {
    name: "recentlyaired",
    context: "hub.tv.recentlyaired",
    kind: "episode",
    title: |c| c.recently_released_episodes,
    query: "type=4&sort=originallyAvailableAt:desc",
    random: false,
};

const START_WATCHING: ListingShelf = ListingShelf {
    name: "startwatching",
    context: "hub.tv.startwatching",
    kind: "show",
    title: |c| c.start_watching,
    query: "type=2&viewedOrInProgressLeafCount=0",
    random: true,
};

const REDISCOVER: ListingShelf = ListingShelf {
    name: "rediscover",
    context: "hub.tv.rediscover",
    kind: "show",
    title: |c| c.rediscover,
    query: "type=2&lastViewedAt<=-6w&lastViewedAt>=-5y&unwatchedLeaves=1&viewCount>=1",
    random: true,
};

const TOP_RATED_TV: ListingShelf = ListingShelf {
    name: "toprated",
    context: "hub.tv.toprated",
    kind: "show",
    title: |c| c.top_rated_tv,
    query: "type=2&unwatchedLeaves=1&audienceRating>=7.0&sort=audienceRating:desc",
    random: true,
};

const RECENTLY_WATCHED_EPISODES: ListingShelf = ListingShelf {
    name: "recentlyviewed",
    context: "hub.tv.recentlyviewed",
    kind: "episode",
    title: |c| c.recently_watched_episodes,
    query: "sort=lastViewedAt:desc&unwatched=0&viewOffset=0&type=4",
    random: false,
};

const FILM_SHELVES_BEFORE: &[ShelfSource] = &[ShelfSource::Fixed(&RECENTLY_RELEASED_MOVIES)];

const FILM_SHELVES_AFTER: &[ShelfSource] = &[
    ShelfSource::TopInGenre,
    ShelfSource::TopByPerson,
    ShelfSource::Fixed(&TOP_UNWATCHED_MOVIES),
    ShelfSource::Fixed(&RECENTLY_WATCHED_MOVIES),
];

const SERIES_SHELVES_BEFORE: &[ShelfSource] = &[ShelfSource::Fixed(&RECENTLY_AIRED)];

const SERIES_SHELVES_AFTER: &[ShelfSource] = &[
    ShelfSource::Fixed(&START_WATCHING),
    ShelfSource::Fixed(&REDISCOVER),
    ShelfSource::Fixed(&TOP_RATED_TV),
    ShelfSource::Fixed(&RECENTLY_WATCHED_EPISODES),
];

/// Where a listing shelf's list comes from.
#[derive(Debug)]
enum ShelfSource {
    /// The same list in every library of a kind.
    Fixed(&'static ListingShelf),
    /// A film library's best unwatched films of one of its genres.
    TopInGenre,
    /// A film library's unwatched films by one of its directors, or with one of its actors.
    TopByPerson,
}

/// One listing shelf, worded for one library and one language.
#[derive(Debug)]
struct ShelfSpec {
    identifier: String,
    context: &'static str,
    kind: &'static str,
    title: String,
    /// The listing's query, worded as the reference words it.
    query: String,
    random: bool,
}

impl ShelfSource {
    /// The shelf this source makes in `section`; none when the library has nothing to draw.
    ///
    /// The genre and the person are drawn at random among all the library's, measured on 150
    /// of the reference's draws: 39 of a film library's 40 genres came up, the one-film genres
    /// as often as the large ones, and directors and actors about evenly, at any billing. A
    /// draw can make an empty shelf, and does there too. Ours holds for the shuffle's period,
    /// as the reference keeps its own for a while.
    async fn spec(
        &self,
        state: &PlexState,
        section: i64,
        prefix: &str,
        language: Language,
    ) -> Result<Option<ShelfSpec>, ApiError> {
        let catalog = language.catalog();
        let named = |template: &str, name: &str| template.replace("{1}", name);
        Ok(match self {
            Self::Fixed(fixed) => Some(ShelfSpec {
                identifier: format!("{prefix}.{}.{section}", fixed.name),
                context: fixed.context,
                kind: fixed.kind,
                title: (fixed.title)(catalog).to_owned(),
                query: fixed.query.to_owned(),
                random: fixed.random,
            }),
            Self::TopInGenre => drawn_tag(state, section, &["GENRE"], "movie.genre")
                .await?
                .map(|(id, name, _)| ShelfSpec {
                    identifier: format!("movie.genre.{section}.{id}"),
                    context: "hub.movie.genre",
                    kind: "movie",
                    title: named(catalog.top_movies_in, &name),
                    query: format!("unwatched=1&genre={id}&audienceRating>=7.0"),
                    random: false,
                }),
            Self::TopByPerson => drawn_tag(
                state,
                section,
                &["DIRECTOR", "ACTOR"],
                "movie.by.actor.or.director",
            )
            .await?
            .map(|(id, name, kind)| {
                let (template, field) = if kind == "DIRECTOR" {
                    (catalog.top_movies_by, "director")
                } else {
                    (catalog.top_movies_with, "actor")
                };
                ShelfSpec {
                    identifier: format!("movie.by.actor.or.director.{section}.{id}"),
                    context: "hub.movie.by.actor.or.director",
                    kind: "movie",
                    title: named(template, &name),
                    query: format!("unwatched=1&{field}={id}&sort=audienceRating:desc"),
                    random: true,
                }
            }),
        })
    }
}

/// A number for `purpose`, fixed for the shuffle's period: what each draw and each random
/// shelf derives its choice from, so two of them do not choose alike.
fn draw(purpose: &str) -> u32 {
    let salt = purpose.bytes().fold(0_u32, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(u32::from(byte))
    });
    // Odd multipliers spread the bits, so neighbouring periods and salts draw apart.
    (crate::library::shuffle_seed() ^ salt)
        .wrapping_mul(0x9e37_79b9)
        .rotate_left(13)
        .wrapping_mul(0x85eb_ca6b)
}

/// One of the library's tags, drawn first among `kinds` evenly, then among that kind's tags
/// evenly: `(id, name, kind)`. None when the library has no tag of any of the kinds.
async fn drawn_tag(
    state: &PlexState,
    section: i64,
    kinds: &[&'static str],
    purpose: &str,
) -> Result<Option<(i64, String, &'static str)>, ApiError> {
    let library = Value::String(GlobalId::encode(LIBRARY_TYPE, section));
    let mut counts = Vec::new();
    for kind in kinds {
        let mut variables = Variables::default();
        variables.insert(Name::new("id"), library.clone());
        variables.insert(Name::new("kind"), Value::Enum(Name::new(*kind)));
        let data: TagCountData = gql::execute(&state.schema, TAG_COUNT_QUERY, variables).await?;
        let total = data.node.map_or(0, |library| library.stats.total);
        if total > 0 {
            counts.push((*kind, total));
        }
    }
    let Some(choice) = u64::from(draw(purpose)).checked_rem(counts.len() as u64) else {
        return Ok(None);
    };
    let (kind, total) = counts[usize::try_from(choice).unwrap_or(0)];
    let offset = u64::from(draw(&format!("{purpose}.{kind}"))) % total;

    let mut variables = Variables::default();
    variables.insert(Name::new("id"), library);
    variables.insert(Name::new("kind"), Value::Enum(Name::new(kind)));
    variables.insert(Name::new("offset"), Value::Number(offset.into()));
    let data: TagAtData = gql::execute(&state.schema, TAG_AT_QUERY, variables).await?;
    Ok(data
        .node
        .and_then(|library| library.tags.edges.into_iter().next())
        .map(|edge| {
            (
                crate::container::decode_key(&edge.node.id),
                edge.node.name,
                kind,
            )
        }))
}

const TAG_COUNT_QUERY: &str = r"
query PlexTagCount($id: ID!, $kind: TagType!) {
  node(id: $id) { ... on Library { stats { total: tagCount(kind: $kind) } } }
}
";

const TAG_AT_QUERY: &str = r"
query PlexTagAt($id: ID!, $kind: TagType!, $offset: Int!) {
  node(id: $id) {
    ... on Library { tags(kind: $kind, first: 1, offset: $offset) { edges { node { id name } } } }
  }
}
";

#[derive(Debug, serde::Deserialize)]
struct TagCountData {
    node: Option<TagCountLibrary>,
}

#[derive(Debug, serde::Deserialize)]
struct TagCountLibrary {
    stats: TagCountStats,
}

#[derive(Debug, serde::Deserialize)]
struct TagCountStats {
    total: u64,
}

#[derive(Debug, serde::Deserialize)]
struct TagAtData {
    node: Option<TagAtLibrary>,
}

#[derive(Debug, serde::Deserialize)]
struct TagAtLibrary {
    tags: TagAtConnection,
}

#[derive(Debug, serde::Deserialize)]
struct TagAtConnection {
    edges: Vec<TagAtEdge>,
}

#[derive(Debug, serde::Deserialize)]
struct TagAtEdge {
    node: TagAtNode,
}

#[derive(Debug, serde::Deserialize)]
struct TagAtNode {
    id: String,
    name: String,
}

/// Fills one listing shelf.
///
/// A `random` shelf is the listing shuffled, in the shuffle a random listing keeps for its
/// period: the reference samples one and keeps it for a while too, so a shelf that changes
/// on every visit would be the deviation.
async fn listing_shelf(
    state: &PlexState,
    section: i64,
    shelf: &ShelfSpec,
    params: &HubParams,
    language: Language,
) -> Result<Hub, ApiError> {
    let pairs: Vec<(String, String)> = form_urlencoded::parse(shelf.query.as_bytes())
        .into_owned()
        .collect();
    let request = crate::listing::parse(&pairs)?;
    let (order_by, direction) = if shelf.random {
        ("RANDOM", "ASCENDING")
    } else {
        request.ordering()
    };
    let count = params.count();

    let mut variables = Variables::default();
    variables.insert(
        Name::new("id"),
        Value::String(GlobalId::encode(LIBRARY_TYPE, section)),
    );
    variables.insert(Name::new("first"), Value::Number(count.into()));
    variables.insert(Name::new("orderBy"), Value::Enum(Name::new(order_by)));
    variables.insert(Name::new("direction"), Value::Enum(Name::new(direction)));
    // Each shelf its own shuffle: one seed would sample the same series for two shelves
    // whose lists overlap.
    variables.insert(
        Name::new("seed"),
        Value::Number(draw(&shelf.identifier).into()),
    );
    variables.insert(
        Name::new("filter"),
        request
            .filters
            .to_graphql(crate::library::item_type(request.kind)),
    );
    let data: ShelfData = gql::execute(&state.schema, &SHELF_QUERY, variables).await?;
    let connection = data.node.ok_or(ApiError::NotFound)?.items;

    let mut items: Vec<MetadataEntry> = connection
        .edges
        .iter()
        .filter_map(|edge| MetadataEntry::from_graphql(&edge.node, true, language))
        .collect();
    if params.excludes_summary() {
        for item in &mut items {
            item.summary = None;
        }
    }
    Ok(Hub {
        hub_key: lookup_key(&items),
        // A raw `>` or `<` is not a legal URI character, and this server's HTTP layer refuses
        // a request line holding one, so the key a client follows carries them encoded.
        key: format!(
            "/library/sections/{section}/all?{}",
            shelf.query.replace('>', "%3E").replace('<', "%3C")
        ),
        title: shelf.title.clone(),
        kind: shelf.kind,
        hub_identifier: shelf.identifier.clone(),
        context: shelf.context,
        size: u32::try_from(items.len()).unwrap_or(u32::MAX),
        more: connection.page_info.has_next_page,
        random: shelf.random.then_some(true),
        style: "shelf",
        promoted: None,
        metadata: items,
    })
}

/// The library and its `count` most recently added items, as the official API groups them.
async fn recently_added(
    state: &PlexState,
    section: i64,
    count: u32,
) -> Result<GqlRecentlyAdded, ApiError> {
    let mut variables = Variables::default();
    variables.insert(
        Name::new("id"),
        Value::String(GlobalId::encode(LIBRARY_TYPE, section)),
    );
    variables.insert(Name::new("first"), Value::Number(count.into()));

    let data: RecentlyAddedData =
        gql::execute(&state.schema, &RECENTLY_ADDED_QUERY, variables).await?;
    data.node.ok_or(ApiError::NotFound)
}

fn continue_watching_shelf(
    section: i64,
    is_film_library: bool,
    language: Language,
    started: &Started,
) -> Hub {
    let (kind, prefix) = if is_film_library {
        ("movie", "movie")
    } else {
        ("episode", "tv")
    };
    let items = started.entries(language, false);
    Hub {
        hub_key: lookup_key(&items),
        key: format!("/hubs/sections/{section}/continueWatching/items"),
        title: language.continue_watching().to_owned(),
        kind,
        hub_identifier: format!("{prefix}.inprogress.{section}"),
        context: if is_film_library {
            "hub.movie.inprogress"
        } else {
            "hub.tv.inprogress"
        },
        size: u32::try_from(items.len()).unwrap_or(u32::MAX),
        more: started.more,
        random: None,
        style: "shelf",
        promoted: None,
        metadata: items,
    }
}

/// The shelf's items as one lookup, `/library/metadata/1,2,3`; none for an empty shelf.
fn lookup_key(items: &[MetadataEntry]) -> Option<String> {
    (!items.is_empty()).then(|| {
        let keys: Vec<&str> = items.iter().map(|item| item.rating_key.as_str()).collect();
        format!("/library/metadata/{}", keys.join(","))
    })
}

/// Continue Watching as the official API answers it.
struct Started {
    edges: Vec<GqlContinueEdge>,
    more: bool,
}

impl Started {
    /// The entries, each dated by the work behind it, which for a next episode is its
    /// series': the reference shows that date on an episode never played. `includedAt`
    /// repeats it, on the home shelf only.
    fn entries(&self, language: Language, with_inclusion: bool) -> Vec<MetadataEntry> {
        self.edges
            .iter()
            .filter_map(|edge| {
                let mut item = MetadataEntry::from_graphql(&edge.node, true, language)?;
                item.last_viewed_at = Some(edge.last_viewed_at);
                item.included_at = with_inclusion.then_some(edge.last_viewed_at);
                Some(item)
            })
            .collect()
    }

    /// Only the entries listed for one reason: `IN_PROGRESS` or `UP_NEXT`.
    fn only(mut self, reason: &str) -> Self {
        self.edges.retain(|edge| edge.reason == reason);
        self
    }
}

async fn continue_watching(
    state: &PlexState,
    section: Option<i64>,
    count: u32,
) -> Result<Started, ApiError> {
    let mut variables = Variables::default();
    if let Some(section) = section {
        variables.insert(
            Name::new("library"),
            Value::String(GlobalId::encode(LIBRARY_TYPE, section)),
        );
    }
    variables.insert(Name::new("first"), Value::Number(count.into()));
    let data: ContinueWatchingData =
        gql::execute(&state.schema, &CONTINUE_WATCHING_QUERY, variables).await?;
    Ok(Started {
        more: data.continue_watching.page_info.has_next_page,
        edges: data.continue_watching.edges,
    })
}

/// How many entries a list endpoint serves when the client says nothing: as many as the
/// window holds, within the ceiling every window respects.
const LIST_COUNT: u32 = MAX_ITEMS;

/// `GET /hubs/continueWatching`: the home screen's first shelf, films in progress and next
/// episodes together, newest first.
async fn continue_watching_hub(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    Query(params): Query<HubParams>,
    format: ResponseFormat,
) -> Result<PlexResponse<HomeHubs>, ApiError> {
    let language = Language::of(&client);
    let started = continue_watching(&state, None, params.count_or(LIST_COUNT)).await?;
    let items = started.entries(language, true);
    let hub = Hub {
        hub_key: lookup_key(&items),
        key: "/hubs/continueWatching/items".to_owned(),
        title: language.continue_watching().to_owned(),
        kind: "mixed",
        hub_identifier: "continueWatching".to_owned(),
        context: "hub.continueWatching",
        size: u32::try_from(items.len()).unwrap_or(u32::MAX),
        more: started.more,
        random: None,
        style: "hero",
        promoted: None,
        metadata: items,
    };
    Ok(PlexResponse::new(
        HomeHubs {
            size: 1,
            allow_sync: true,
            identifier: LIBRARY_IDENTIFIER,
            hub: vec![hub],
        },
        format,
    ))
}

async fn continue_watching_items(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    format: ResponseFormat,
) -> Result<PlexResponse<HubItems>, ApiError> {
    let started = continue_watching(&state, None, LIST_COUNT).await?;
    Ok(PlexResponse::new(
        HubItems::new(started.entries(Language::of(&client), true)),
        format,
    ))
}

/// `GET /hubs/home/continueWatching`: what was started, without the next episodes.
async fn home_in_progress(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    format: ResponseFormat,
) -> Result<PlexResponse<HubItems>, ApiError> {
    let started = continue_watching(&state, None, LIST_COUNT)
        .await?
        .only("IN_PROGRESS");
    Ok(PlexResponse::new(
        HubItems::new(started.entries(Language::of(&client), false)),
        format,
    ))
}

/// `GET /hubs/home/onDeck`: the next episodes, without what was started.
async fn home_up_next(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    format: ResponseFormat,
) -> Result<PlexResponse<HubItems>, ApiError> {
    let started = continue_watching(&state, None, LIST_COUNT)
        .await?
        .only("UP_NEXT");
    Ok(PlexResponse::new(
        HubItems::new(started.entries(Language::of(&client), false)),
        format,
    ))
}

async fn section_continue_watching_items(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    Path(section): Path<i64>,
    format: ResponseFormat,
) -> Result<PlexResponse<HubItems>, ApiError> {
    let started = continue_watching(&state, Some(section), LIST_COUNT).await?;
    Ok(PlexResponse::new(
        HubItems::new(started.entries(Language::of(&client), false)),
        format,
    ))
}

/// The home shelves' container: the shelves alone, no library.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HomeHubs {
    size: u32,
    allow_sync: bool,
    identifier: &'static str,
    #[serde(rename = "Hub")]
    hub: Vec<Hub>,
}

/// The items behind a shelf.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HubItems {
    size: u32,
    allow_sync: bool,
    identifier: &'static str,
    #[serde(rename = "Metadata", skip_serializing_if = "Vec::is_empty")]
    metadata: Vec<MetadataEntry>,
}

impl HubItems {
    fn new(metadata: Vec<MetadataEntry>) -> Self {
        Self {
            size: u32::try_from(metadata.len()).unwrap_or(u32::MAX),
            allow_sync: true,
            identifier: LIBRARY_IDENTIFIER,
            metadata,
        }
    }
}

/// How many entries a series' "recently added" list holds, as the reference caps it.
const RECENT_SERIES_ENTRIES: u32 = 50;

/// Response to `GET /hubs/home/recentlyAdded`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentItems {
    size: u32,
    /// Where the window sits, sent when the client asked for one or the list exceeds it.
    #[serde(skip_serializing_if = "Option::is_none")]
    total_size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    offset: Option<u64>,
    allow_sync: bool,
    identifier: &'static str,
    #[serde(rename = "Metadata", skip_serializing_if = "Vec::is_empty")]
    metadata: Vec<MetadataEntry>,
}

/// `GET /hubs/home/recentlyAdded?type=&sectionID=`: what a library, or every library of a
/// kind, added lately. The list behind a series library's "recently added" shelf.
///
/// Measured on the reference: `type=1` lists films, `type=2` a series library's grouped
/// entries, capped at fifty; any other type, or none, an empty container, as does a library
/// of the other kind or an unknown one. A `sectionID` that is not a number is ignored, and
/// every library of the kind is listed.
async fn home_recently_added(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    Query(pairs): Query<Vec<(String, String)>>,
    format: ResponseFormat,
) -> Result<PlexResponse<RecentItems>, ApiError> {
    let value = |name: &str| {
        pairs
            .iter()
            .rev()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };
    let start: Option<u64> = value("X-Plex-Container-Start").and_then(|start| start.parse().ok());
    let size: Option<u32> = value("X-Plex-Container-Size").and_then(|size| size.parse().ok());
    // Both or neither, as everywhere a Plex client pages.
    let window = start.zip(size);
    let kind = match value("type") {
        Some("1") => Some("MOVIE"),
        Some("2") => Some("SHOW"),
        _ => None,
    };
    let section = value("sectionID").and_then(|section| section.parse::<i64>().ok());
    let language = Language::of(&client);

    let (edges, total) = match kind {
        Some(kind) => recent_entries(&state, kind, section, window).await?,
        None => (Vec::new(), 0),
    };
    let offset = window.map_or(0, |(start, _)| start);
    let metadata: Vec<MetadataEntry> = edges
        .iter()
        .filter_map(|edge| {
            let mut item = MetadataEntry::from_graphql(&edge.node, true, language)?;
            item.added_at = edge.added_at;
            Some(item)
        })
        .collect();
    let size = u32::try_from(metadata.len()).unwrap_or(u32::MAX);
    let windowed = window.is_some() || offset + u64::from(size) < total;
    let body = RecentItems {
        size,
        total_size: windowed.then_some(total),
        offset: windowed.then_some(offset),
        allow_sync: true,
        identifier: LIBRARY_IDENTIFIER,
        metadata,
    };
    let response = PlexResponse::new(body, format);
    Ok(if windowed {
        response.paged(offset, total)
    } else {
        response
    })
}

/// One window of the entries, and how many there are in all.
///
/// A series list is read whole, its fifty entries being what the total counts; a film list
/// by window, its total being its libraries' films.
async fn recent_entries(
    state: &PlexState,
    kind: &str,
    section: Option<i64>,
    window: Option<(u64, u32)>,
) -> Result<(Vec<GqlRecentEdge>, u64), ApiError> {
    let series = kind == "SHOW";
    let (offset, first) = match (series, window) {
        (true, _) => (0, RECENT_SERIES_ENTRIES),
        (false, Some((start, size))) => (start, size.min(MAX_ITEMS)),
        (false, None) => (0, MAX_ITEMS),
    };
    let mut variables = Variables::default();
    variables.insert(Name::new("first"), Value::Number(first.into()));
    variables.insert(Name::new("offset"), Value::Number(offset.into()));

    variables.insert(Name::new("kind"), Value::Enum(Name::new(kind)));
    let (mut edges, film_total) = if let Some(section) = section {
        variables.insert(
            Name::new("id"),
            Value::String(GlobalId::encode(LIBRARY_TYPE, section)),
        );
        let data: LibraryRecentData =
            gql::execute(&state.schema, &LIBRARY_RECENT_QUERY, variables).await?;
        match data.node {
            Some(library) if library.kind == kind => {
                (data.recently_added.edges, library.stats.item_count)
            }
            // Unknown, or of the other kind: nothing, as on the reference.
            _ => return Ok((Vec::new(), 0)),
        }
    } else {
        let data: HomeRecentData =
            gql::execute(&state.schema, &HOME_RECENT_QUERY, variables).await?;
        let films = data
            .libraries
            .edges
            .iter()
            .filter(|edge| edge.node.kind == kind)
            .map(|edge| edge.node.stats.item_count)
            .sum();
        (data.recently_added.edges, films)
    };
    if !series {
        return Ok((edges, film_total));
    }
    let total = edges.len() as u64;
    if let Some((start, size)) = window {
        let start = usize::try_from(start)
            .unwrap_or(usize::MAX)
            .min(edges.len());
        edges.drain(..start);
        edges.truncate(usize::try_from(size).unwrap_or(usize::MAX));
    }
    Ok((edges, total))
}

/// The "recently added" shelf, in the form the library's kind calls for.
struct RecentShelf<'a> {
    section: i64,
    library_title: &'a str,
    is_film_library: bool,
    more: bool,
}

impl RecentShelf<'_> {
    /// A film library's shelf holds films; a series library's mixes series, seasons and
    /// episodes, and its full list lives under the home hub rather than the library listing.
    fn hub(&self, items: Vec<MetadataEntry>, language: Language) -> Hub {
        let section = self.section;
        let size = u32::try_from(items.len()).unwrap_or(u32::MAX);
        let keys: Vec<&str> = items.iter().map(|item| item.rating_key.as_str()).collect();
        let (key, kind, prefix) = if self.is_film_library {
            (
                format!("/library/sections/{section}/all?sort=addedAt:desc"),
                "movie",
                "movie",
            )
        } else {
            (
                format!("/hubs/home/recentlyAdded?type=2&sectionID={section}"),
                "mixed",
                "tv",
            )
        };
        Hub {
            hub_key: (!keys.is_empty()).then(|| format!("/library/metadata/{}", keys.join(","))),
            key,
            title: language.recently_added_in(self.library_title),
            kind,
            hub_identifier: format!("{prefix}.recentlyadded.{section}"),
            context: if self.is_film_library {
                "hub.movie.recentlyadded"
            } else {
                "hub.tv.recentlyadded"
            },
            size,
            more: self.more,
            random: None,
            style: "shelf",
            promoted: Some(true),
            metadata: items,
        }
    }
}

fn recent_playlists(section: i64, language: Language) -> Hub {
    Hub {
        hub_key: None,
        key: format!(
            "/playlists/all?type=15&playlistType=video&sort=lastViewedAt:desc&sectionID={section}"
        ),
        title: language.recent_playlists().to_owned(),
        kind: "playlist",
        hub_identifier: format!("recent.library.playlists.{section}"),
        context: "hub.recent.library.playlists",
        size: 0,
        more: false,
        random: None,
        style: "shelf",
        promoted: None,
        metadata: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(count: Option<&str>) -> HubParams {
        HubParams {
            count: count.map(ToOwned::to_owned),
            ..HubParams::default()
        }
    }

    #[test]
    fn a_count_that_is_not_a_number_gets_six() {
        // Measured: `count=abc` and no `count` both give six items per shelf.
        assert_eq!(params(None).count(), 6);
        assert_eq!(params(Some("abc")).count(), 6);
        assert_eq!(params(Some("12")).count(), 12);
        assert_eq!(params(Some("100000")).count(), MAX_ITEMS);
    }
}
