//! `GET /hubs` and `GET /hubs/promoted`: the home screen's shelves, across every library.
//!
//! `/hubs` answers the reference's eight global shelves: what was started, what comes next,
//! what each kind of library added lately, and the playlists played last. Music, photos and
//! personal videos are kinds this server holds none of, so their shelves are listed empty,
//! as the reference lists a kind it has nothing of. `/hubs/promoted` keeps the first two,
//! then each library's own "recently added" shelf, then the playlists.
//!
//! `/hubs/popularInLibraries` is a `404`, as on the reference: see [`popular`].

use async_graphql::Variables;
use axum::extract::{Query, State};
use axum::routing::get;
use axum::Router;
use klapz_core::page::MAX_ITEMS;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::auth::Caller;
use crate::client::ClientIdentity;
use crate::error::ApiError;
use crate::gql;
use crate::hubs::{
    continue_watching, lookup_key, recent_entries, recent_shelf, recently_added, Hub,
};
use crate::items::{MetadataEntry, LIBRARY_IDENTIFIER};
use crate::language::Language;
use crate::response::{PlexResponse, ResponseFormat};
use crate::PlexState;

pub fn router() -> Router<PlexState> {
    Router::new()
        .route("/hubs", get(global))
        .route("/hubs/promoted", get(promoted))
        .route("/hubs/popularInLibraries", get(popular))
}

/// Items per shelf when the client does not say: six, as on the reference.
const DEFAULT_COUNT: u32 = 6;

#[derive(Debug, Default, Deserialize)]
struct Params {
    count: Option<String>,
    /// A comma-separated list of `hubIdentifier`s: only those shelves.
    identifier: Option<String>,
}

impl Params {
    fn count(&self) -> u32 {
        self.count
            .as_deref()
            .and_then(|count| count.parse().ok())
            .unwrap_or(DEFAULT_COUNT)
            .min(MAX_ITEMS)
    }

    fn wants(&self, identifier: &str) -> bool {
        self.identifier
            .as_deref()
            .is_none_or(|wanted| wanted.split(',').any(|wanted| wanted == identifier))
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HomeContainer {
    size: usize,
    allow_sync: bool,
    identifier: &'static str,
    #[serde(rename = "Hub", skip_serializing_if = "Vec::is_empty")]
    hub: Vec<HomeHub>,
}

impl HomeContainer {
    fn new(hub: Vec<HomeHub>) -> Self {
        Self {
            size: hub.len(),
            allow_sync: true,
            identifier: LIBRARY_IDENTIFIER,
            hub,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum HomeHub {
    Items(Hub),
    Playlists(PlaylistHub),
}

/// The playlists shelf, whose entries are playlists rather than library items.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PlaylistHub {
    #[serde(skip_serializing_if = "Option::is_none")]
    hub_key: Option<String>,
    key: &'static str,
    title: &'static str,
    #[serde(rename = "type")]
    kind: &'static str,
    hub_identifier: &'static str,
    context: &'static str,
    size: usize,
    more: bool,
    style: &'static str,
    promoted: bool,
    #[serde(rename = "Metadata", skip_serializing_if = "Vec::is_empty")]
    metadata: Vec<Json>,
}

/// What a home shelf is made of, before its items are read.
struct HomeShelf {
    key: &'static str,
    title: &'static str,
    kind: &'static str,
    identifier: &'static str,
    style: &'static str,
}

impl HomeShelf {
    fn hub(&self, items: Vec<MetadataEntry>, more: bool) -> HomeHub {
        HomeHub::Items(Hub {
            hub_key: lookup_key(&items),
            key: self.key.to_owned(),
            title: self.title.to_owned(),
            kind: self.kind,
            hub_identifier: self.identifier.to_owned(),
            context: context_of(self.identifier),
            size: u32::try_from(items.len()).unwrap_or(u32::MAX),
            more,
            random: None,
            style: self.style,
            promoted: Some(true),
            metadata: items,
        })
    }
}

/// `hub.` and the identifier, for every home shelf.
fn context_of(identifier: &str) -> &'static str {
    match identifier {
        "home.continue" => "hub.home.continue",
        "home.ondeck" => "hub.home.ondeck",
        "home.movies.recent" => "hub.home.movies.recent",
        "home.television.recent" => "hub.home.television.recent",
        "home.music.recent" => "hub.home.music.recent",
        "home.photos.recent" => "hub.home.photos.recent",
        "home.videos.recent" => "hub.home.videos.recent",
        _ => "hub.home.playlists",
    }
}

/// What was started and what comes next, the first two shelves of both lists.
///
/// Measured on the reference: an episode in progress is in both, as its series' next.
async fn started_shelves(
    state: &PlexState,
    params: &Params,
    language: Language,
) -> Result<Vec<HomeHub>, ApiError> {
    let count = params.count() as usize;
    let catalog = language.catalog();
    let started = continue_watching(state, None, MAX_ITEMS).await?;
    let in_progress = started.only("IN_PROGRESS").entries(language, false);
    let next = continue_watching(state, None, MAX_ITEMS)
        .await?
        .episodes()
        .entries(language, false);

    let mut hubs = Vec::new();
    if params.wants("home.continue") {
        let more = in_progress.len() > count;
        let shelf = HomeShelf {
            key: "/hubs/home/continueWatching",
            title: catalog.continue_watching,
            kind: "mixed",
            identifier: "home.continue",
            style: "hero",
        };
        hubs.push(shelf.hub(in_progress.into_iter().take(count).collect(), more));
    }
    if params.wants("home.ondeck") {
        let more = next.len() > count;
        let shelf = HomeShelf {
            key: "/hubs/home/onDeck",
            title: catalog.on_deck,
            kind: "episode",
            identifier: "home.ondeck",
            style: "shelf",
        };
        hubs.push(shelf.hub(next.into_iter().take(count).collect(), more));
    }
    Ok(hubs)
}

/// The playlists the caller played last.
async fn playlists_shelf(
    state: &PlexState,
    params: &Params,
    language: Language,
) -> Result<Option<HomeHub>, ApiError> {
    if !params.wants("home.playlists") {
        return Ok(None);
    }
    let (metadata, more) = crate::playlists::recently_played(state, params.count()).await?;
    let keys: Vec<String> = metadata
        .iter()
        .filter_map(|playlist| playlist.get("ratingKey").and_then(Json::as_str))
        .map(ToOwned::to_owned)
        .collect();
    Ok(Some(HomeHub::Playlists(PlaylistHub {
        hub_key: (!keys.is_empty()).then(|| format!("/playlists/{}", keys.join(","))),
        key: "/playlists/all?type=15&sort=lastViewedAt:desc&playlistType=video,audio",
        title: language.recent_playlists(),
        kind: "playlist",
        hub_identifier: "home.playlists",
        context: "hub.home.playlists",
        size: metadata.len(),
        more,
        style: "shelf",
        promoted: true,
        metadata,
    })))
}

/// `GET /hubs`.
async fn global(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    Query(params): Query<Params>,
    format: ResponseFormat,
) -> Result<PlexResponse<HomeContainer>, ApiError> {
    let language = Language::of(&client);
    let catalog = language.catalog();
    let count = params.count();
    let mut hubs = started_shelves(&state, &params, language).await?;

    for (kind, shelf) in [
        (
            "MOVIE",
            HomeShelf {
                key: "/hubs/home/recentlyAdded?type=1",
                title: catalog.recently_added_movies,
                kind: "movie",
                identifier: "home.movies.recent",
                style: "shelf",
            },
        ),
        (
            "SHOW",
            HomeShelf {
                key: "/hubs/home/recentlyAdded?type=2",
                title: catalog.recently_added_tv,
                kind: "mixed",
                identifier: "home.television.recent",
                style: "shelf",
            },
        ),
    ] {
        if !params.wants(shelf.identifier) {
            continue;
        }
        let (edges, total) = recent_entries(&state, kind, None, Some((0, count))).await?;
        let items = edges
            .iter()
            .filter_map(|edge| {
                let mut item = MetadataEntry::from_graphql(&edge.node, true, language)?;
                item.added_at = edge.added_at;
                Some(item)
            })
            .collect();
        hubs.push(shelf.hub(items, total > u64::from(count)));
    }
    // Kinds held nowhere here: listed, and empty.
    for shelf in [
        HomeShelf {
            key: "/hubs/home/recentlyAdded?type=8",
            title: catalog.recently_added_music,
            kind: "album",
            identifier: "home.music.recent",
            style: "shelf",
        },
        HomeShelf {
            key: "/hubs/home/recentlyAdded?type=13",
            title: catalog.recently_added_photos,
            kind: "photo",
            identifier: "home.photos.recent",
            style: "shelf",
        },
        HomeShelf {
            key: "/hubs/home/recentlyAdded?type=1&personal=1",
            title: catalog.recently_added_videos,
            kind: "clip",
            identifier: "home.videos.recent",
            style: "shelf",
        },
    ] {
        if params.wants(shelf.identifier) {
            hubs.push(shelf.hub(Vec::new(), false));
        }
    }
    hubs.extend(playlists_shelf(&state, &params, language).await?);
    Ok(PlexResponse::new(HomeContainer::new(hubs), format))
}

/// `GET /hubs/popularInLibraries`: `404`, whatever the parameters, as the reference answers.
///
/// Its shelf counts the plays of the server's *other* users over `PopularInLibrariesWindow`
/// weeks. Each user here has a server of its own, whose history nobody else's plays reach,
/// so there are no other users' plays to count: the shelf is empty, and the reference
/// answers an empty one, or one switched off, with a `404` and leaves it out of `/hubs`.
async fn popular(_caller: Caller) -> ApiError {
    ApiError::NotFound
}

const LIBRARIES_QUERY: &str = r"
query PlexHomeLibraries {
  libraries(first: 500) { edges { node { id title kind hidden } } }
}
";

#[derive(Debug, Deserialize)]
struct LibrariesData {
    libraries: LibraryConnection,
}

#[derive(Debug, Deserialize)]
struct LibraryConnection {
    edges: Vec<LibraryEdge>,
}

#[derive(Debug, Deserialize)]
struct LibraryEdge {
    node: HomeLibrary,
}

#[derive(Debug, Deserialize)]
struct HomeLibrary {
    id: String,
    title: String,
    kind: String,
    hidden: bool,
}

/// `GET /hubs/promoted`: what was started and what comes next, then the "recently added"
/// shelf of every library the home screen shows, by title, then the playlists.
///
/// Measured on the reference: its two hidden libraries have none, and the libraries come in
/// the order of their titles, case aside.
async fn promoted(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    Query(params): Query<Params>,
    format: ResponseFormat,
) -> Result<PlexResponse<HomeContainer>, ApiError> {
    let language = Language::of(&client);
    let count = params.count();
    let mut hubs = started_shelves(&state, &params, language).await?;

    let data: LibrariesData =
        gql::execute(&state.schema, LIBRARIES_QUERY, Variables::default()).await?;
    let mut libraries: Vec<HomeLibrary> = data
        .libraries
        .edges
        .into_iter()
        .map(|edge| edge.node)
        .filter(|library| !library.hidden && matches!(library.kind.as_str(), "MOVIE" | "SHOW"))
        .collect();
    libraries.sort_by_cached_key(|library| library.title.to_lowercase());
    for library in libraries {
        let section = crate::gql_items::database_id(&library.id);
        let prefix = if library.kind == "MOVIE" {
            "movie"
        } else {
            "tv"
        };
        if !params.wants(&format!("{prefix}.recentlyadded.{section}")) {
            continue;
        }
        let recent = recently_added(&state, section, count).await?;
        hubs.push(HomeHub::Items(recent_shelf(
            &recent, section, false, language,
        )));
    }
    hubs.extend(playlists_shelf(&state, &params, language).await?);
    Ok(PlexResponse::new(HomeContainer::new(hubs), format))
}
