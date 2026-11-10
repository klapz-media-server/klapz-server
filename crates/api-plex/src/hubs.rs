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
//! - **Start watching**, the series not begun; **top unwatched** films and **top rated**
//!   series, rated 7 or more by their audience; **recently watched** films or episodes.
//! - **Recent playlists**, empty, when the client asks for library playlists: there are
//!   none.
//!
//! Left out: the shelves of one genre, one person, and "rediscover", which pick their genre,
//! person or period in a way not measured yet.

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
    ContinueWatchingData, GqlContinueEdge, GqlRecentlyAdded, RecentlyAddedData, ShelfData,
    CONTINUE_WATCHING_QUERY, RECENTLY_ADDED_QUERY, SHELF_QUERY,
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
    for listing in before {
        hubs.push(listing_shelf(&state, section, prefix, listing, &params, language).await?);
    }
    hubs.push(shelf.hub(items, language));
    for listing in after {
        hubs.push(listing_shelf(&state, section, prefix, listing, &params, language).await?);
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

const FILM_SHELVES_BEFORE: &[ListingShelf] = &[ListingShelf {
    name: "recentlyreleased",
    context: "hub.movie.recentlyreleased",
    kind: "movie",
    title: |c| c.recently_released_movies,
    query: "sort=originallyAvailableAt:desc&originallyAvailableAt>=-1y",
    random: false,
}];

const FILM_SHELVES_AFTER: &[ListingShelf] = &[
    ListingShelf {
        name: "topunwatched",
        context: "hub.movie.topunwatched",
        kind: "movie",
        title: |c| c.top_unwatched_movies,
        query: "unwatched=1&audienceRating>=7.0&sort=audienceRating:desc",
        random: true,
    },
    ListingShelf {
        name: "recentlyviewed",
        context: "hub.movie.recentlyviewed",
        kind: "movie",
        title: |c| c.recently_watched_movies,
        query: "sort=lastViewedAt:desc&unwatched=0&viewOffset=0",
        random: false,
    },
];

const SERIES_SHELVES_BEFORE: &[ListingShelf] = &[ListingShelf {
    name: "recentlyaired",
    context: "hub.tv.recentlyaired",
    kind: "episode",
    title: |c| c.recently_released_episodes,
    query: "type=4&sort=originallyAvailableAt:desc",
    random: false,
}];

const SERIES_SHELVES_AFTER: &[ListingShelf] = &[
    ListingShelf {
        name: "startwatching",
        context: "hub.tv.startwatching",
        kind: "show",
        title: |c| c.start_watching,
        query: "type=2&viewedOrInProgressLeafCount=0",
        random: true,
    },
    ListingShelf {
        name: "toprated",
        context: "hub.tv.toprated",
        kind: "show",
        title: |c| c.top_rated_tv,
        query: "type=2&unwatchedLeaves=1&audienceRating>=7.0&sort=audienceRating:desc",
        random: true,
    },
    ListingShelf {
        name: "recentlyviewed",
        context: "hub.tv.recentlyviewed",
        kind: "episode",
        title: |c| c.recently_watched_episodes,
        query: "sort=lastViewedAt:desc&unwatched=0&viewOffset=0&type=4",
        random: false,
    },
];

/// Fills one listing shelf.
///
/// A `random` shelf is the listing shuffled, in the shuffle a random listing keeps for its
/// period: the reference samples one and keeps it for a while too, so a shelf that changes
/// on every visit would be the deviation.
async fn listing_shelf(
    state: &PlexState,
    section: i64,
    prefix: &str,
    shelf: &ListingShelf,
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
    let salt = shelf.name.bytes().fold(0_u32, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(u32::from(byte))
    });
    variables.insert(
        Name::new("seed"),
        Value::Number((crate::library::shuffle_seed() ^ salt).into()),
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
        // A raw `>` is not a legal URI character, and this server's HTTP layer refuses a
        // request line holding one, so the key a client follows carries it encoded.
        key: format!(
            "/library/sections/{section}/all?{}",
            shelf.query.replace('>', "%3E")
        ),
        title: (shelf.title)(language.catalog()).to_owned(),
        kind: shelf.kind,
        hub_identifier: format!("{prefix}.{}.{section}", shelf.name),
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
