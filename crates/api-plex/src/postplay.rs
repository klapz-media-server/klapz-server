//! `GET /hubs/metadata/{id}/postplay`, the shelves a player shows once an item ends, and
//! `GET /hubs/metadata/{id}`, an item's music shelves.
//!
//! After an episode, its series' next episode, else the library's Continue Watching. After a
//! film, the next film of its collection, every collection it is in, and the films like it.
//! A series or a season has none. Measured on the reference; see `docs/spec-gaps.md`.

use async_graphql::{Name, Value, Variables};
use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::Router;
use open_plex_api::item::ITEM_TYPE;
use open_plex_api::library::LIBRARY_TYPE;
use open_plex_api::GlobalId;
use open_plex_core::page::MAX_ITEMS;
use serde::{Deserialize, Serialize};

use crate::auth::Caller;
use crate::client::ClientIdentity;
use crate::error::ApiError;
use crate::gql;
use crate::gql_items::{
    with_shared_fragments, GqlConnection, GqlItem, GqlItemEdge, GqlLibraryRef, GqlWatchState,
    SectionItemsData, SECTION_ITEMS_QUERY,
};
use crate::hubs::{continue_watching, lookup_key, Hub};
use crate::items::{MetadataEntry, LIBRARY_IDENTIFIER};
use crate::language::Language;
use crate::response::{PlexResponse, ResponseFormat};
use crate::PlexState;

pub fn router() -> Router<PlexState> {
    Router::new()
        .route("/hubs/metadata/{metadataId}/postplay", get(postplay))
        .route("/hubs/metadata/{metadataId}", get(music_shelves))
}

/// Items per shelf, the reference's default; `count` changes it.
const DEFAULT_COUNT: u32 = 6;

/// How many collections of a film get a shelf: more than any film of the reference library
/// is in.
const COLLECTION_SHELVES: u32 = 50;

#[derive(Debug, Default, Deserialize)]
struct Params {
    count: Option<String>,
}

impl Params {
    fn count(&self) -> u32 {
        self.count
            .as_deref()
            .and_then(|count| count.parse().ok())
            .unwrap_or(DEFAULT_COUNT)
            .min(MAX_ITEMS)
    }
}

static POSTPLAY_QUERY: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    with_shared_fragments(
        r"
query PlexPostPlay($id: ID!, $first: Int!, $shown: Int!, $collections: Int!) {
  node(id: $id) {
    ... on Item {
      __typename
      library { id title uuid kind }
    }
    ... on Episode {
      episodeNext: upNext {
        ...RecentItem
        series: show { watchState { lastViewedAt } }
      }
    }
    ... on Movie {
      filmNext: upNext(within: $shown) {
        movie { ...ListedItem }
        collection { title }
      }
      collections(first: $collections) { edges { node { tagNumber title } } }
      similar(first: $first) { edges { node { ...ListedItem } } }
    }
  }
}
",
    )
});

#[derive(Debug, Deserialize)]
struct PostPlayData {
    node: Option<GqlPostPlay>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlPostPlay {
    #[serde(rename = "__typename")]
    typename: String,
    library: GqlLibraryRef,
    episode_next: Option<GqlEpisodeNext>,
    film_next: Option<GqlFilmNext>,
    collections: Option<GqlConnection<GqlShelfCollection>>,
    similar: Option<GqlSimilar>,
}

#[derive(Debug, Deserialize)]
struct GqlEpisodeNext {
    #[serde(flatten)]
    episode: GqlItem,
    series: Option<GqlSeries>,
}

#[derive(Debug, Deserialize)]
struct GqlFilmNext {
    movie: GqlItem,
    collection: GqlNamed,
}

#[derive(Debug, Deserialize)]
struct GqlNamed {
    title: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlSeries {
    watch_state: Option<GqlWatchState>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlShelfCollection {
    tag_number: Option<String>,
    title: String,
}

#[derive(Debug, Deserialize)]
struct GqlSimilar {
    edges: Vec<GqlItemEdge>,
}

/// Bare: no library, and `allowSync` off, as the reference sends it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PostPlayContainer {
    size: usize,
    allow_sync: bool,
    identifier: &'static str,
    #[serde(rename = "Hub", skip_serializing_if = "Vec::is_empty")]
    hub: Vec<PostPlayHub>,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum PostPlayHub {
    UpNext(UpNextHub),
    Shelf(Hub),
}

/// The one item to play next. Unlike a shelf it leads nowhere: no `key`, and `upNext`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpNextHub {
    hub_key: String,
    up_next: bool,
    title: String,
    #[serde(rename = "type")]
    kind: &'static str,
    hub_identifier: &'static str,
    context: &'static str,
    size: u32,
    more: bool,
    style: &'static str,
    #[serde(rename = "Metadata")]
    metadata: Vec<MetadataEntry>,
}

impl UpNextHub {
    fn new(item: MetadataEntry, title: String, film: bool) -> Self {
        Self {
            hub_key: format!("/library/metadata/{}", item.rating_key),
            up_next: true,
            title,
            kind: if film { "movie" } else { "episode" },
            hub_identifier: if film { "movie.upnext" } else { "tv.upnext" },
            context: if film {
                "hub.movie.upnext"
            } else {
                "hub.tv.upnext"
            },
            size: 1,
            more: false,
            style: "shelf",
            metadata: vec![item],
        }
    }
}

async fn postplay(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    Path(id): Path<String>,
    Query(params): Query<Params>,
    format: ResponseFormat,
) -> Result<PlexResponse<PostPlayContainer>, ApiError> {
    let item = crate::related::rating_key(&id)?;
    let count = params.count();
    let language = Language::of(&client);

    let mut variables = Variables::default();
    variables.insert(
        Name::new("id"),
        Value::String(GlobalId::encode(ITEM_TYPE, item)),
    );
    variables.insert(Name::new("first"), Value::Number((count + 1).into()));
    // The film played has to be on a collection's shelf for that collection to offer what
    // follows it, as on the reference: one past the sixth is followed by nothing there.
    variables.insert(Name::new("shown"), Value::Number(count.into()));
    variables.insert(
        Name::new("collections"),
        Value::Number(COLLECTION_SHELVES.into()),
    );
    let data: PostPlayData = gql::execute(&state.schema, &POSTPLAY_QUERY, variables).await?;
    let node = data.node.ok_or(ApiError::NotFound)?;

    let hubs = match node.typename.as_str() {
        "Episode" => after_episode(&state, item, node, count, language).await?,
        "Movie" => after_film(&state, item, node, count, language).await?,
        _ => Vec::new(),
    };
    Ok(PlexResponse::new(
        PostPlayContainer {
            size: hubs.len(),
            allow_sync: false,
            identifier: LIBRARY_IDENTIFIER,
            hub: hubs,
        },
        format,
    ))
}

/// The series' next episode; else what the library has under way, the episode just played
/// left out, on a shelf leading to the library's On Deck.
async fn after_episode(
    state: &PlexState,
    item: i64,
    node: GqlPostPlay,
    count: u32,
    language: Language,
) -> Result<Vec<PostPlayHub>, ApiError> {
    if let Some(GqlEpisodeNext { episode, series }) = node.episode_next {
        let Some(mut entry) = MetadataEntry::from_graphql(&episode, true, language) else {
            return Ok(Vec::new());
        };
        // Dated by its series, as in Continue Watching: an episode never played shows when
        // the series last was.
        entry.last_viewed_at = series
            .and_then(|series| series.watch_state)
            .and_then(|state| state.last_viewed_at)
            .or(entry.last_viewed_at);
        let title = language.catalog().up_next.to_owned();
        return Ok(vec![PostPlayHub::UpNext(UpNextHub::new(
            entry, title, false,
        ))]);
    }

    let section = node.library.database_id();
    let mut started = continue_watching(state, Some(section), count + 1).await?;
    let played = GlobalId::encode(ITEM_TYPE, item);
    started.edges.retain(|edge| edge.node.id != played);
    if started.edges.is_empty() {
        return Ok(Vec::new());
    }
    let more = started.more || started.edges.len() > count as usize;
    started.edges.truncate(count as usize);
    let items = started.entries(language, false);
    Ok(vec![PostPlayHub::Shelf(Hub {
        hub_key: lookup_key(&items),
        key: format!("/library/sections/{section}/onDeck"),
        title: language.catalog().on_deck.to_owned(),
        kind: "episode",
        hub_identifier: "tv.ondeck".to_owned(),
        context: "hub.tv.ondeck",
        size: u32::try_from(items.len()).unwrap_or(u32::MAX),
        more,
        random: None,
        style: "shelf",
        promoted: None,
        metadata: items,
    })])
}

/// The next film of its collection, a shelf per collection in the order the film joined
/// them, and the films like it.
async fn after_film(
    state: &PlexState,
    item: i64,
    node: GqlPostPlay,
    count: u32,
    language: Language,
) -> Result<Vec<PostPlayHub>, ApiError> {
    let section = node.library.database_id();
    let catalog = language.catalog();
    let mut hubs = Vec::new();
    if let Some(GqlFilmNext { movie, collection }) = node.film_next {
        if let Some(entry) = MetadataEntry::from_graphql(&movie, true, language) {
            let title = catalog.up_next_in.replace("{1}", &collection.title);
            hubs.push(PostPlayHub::UpNext(UpNextHub::new(entry, title, true)));
        }
    }
    for collection in node.collections.into_iter().flat_map(|c| c.edges) {
        let Some(number) = collection.node.tag_number else {
            continue;
        };
        let title = catalog
            .collection_shelf
            .replace("{1}", &collection.node.title);
        hubs.push(PostPlayHub::Shelf(
            collection_shelf(state, section, &number, title, count, language).await?,
        ));
    }
    if let Some(similar) = node.similar {
        hubs.extend(
            crate::related::similar_shelf(item, true, &similar.edges, count, language)
                .map(PostPlayHub::Shelf),
        );
    }
    Ok(hubs)
}

/// A collection's films by release date, as the listing its `key` opens lists them.
async fn collection_shelf(
    state: &PlexState,
    section: i64,
    number: &str,
    title: String,
    count: u32,
    language: Language,
) -> Result<Hub, ApiError> {
    let query = format!("collection={number}&sort=originallyAvailableAt");
    let pairs: Vec<(String, String)> = form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect();
    let request = crate::listing::parse(&pairs)?;
    let (order_by, direction) = request.ordering();

    let mut variables = Variables::default();
    variables.insert(
        Name::new("id"),
        Value::String(GlobalId::encode(LIBRARY_TYPE, section)),
    );
    variables.insert(Name::new("first"), Value::Number((count + 1).into()));
    variables.insert(Name::new("offset"), Value::Number(0.into()));
    variables.insert(Name::new("orderBy"), Value::Enum(Name::new(order_by)));
    variables.insert(Name::new("direction"), Value::Enum(Name::new(direction)));
    variables.insert(
        Name::new("filter"),
        request.filters.to_graphql(Some("MOVIE")),
    );
    let data: SectionItemsData =
        gql::execute(&state.schema, &SECTION_ITEMS_QUERY, variables).await?;
    let edges = data.node.ok_or(ApiError::NotFound)?.items.edges;
    let more = edges.len() > count as usize;
    let items: Vec<MetadataEntry> = edges
        .iter()
        .take(count as usize)
        .filter_map(|edge| MetadataEntry::from_graphql(&edge.node, true, language))
        .collect();
    Ok(Hub {
        hub_key: lookup_key(&items),
        key: format!("/library/sections/{section}/all?{query}"),
        title,
        kind: "movie",
        hub_identifier: format!("movie.collection.{number}"),
        context: "hub.movie.collection",
        size: u32::try_from(items.len()).unwrap_or(u32::MAX),
        more,
        random: None,
        style: "shelf",
        promoted: None,
        metadata: items,
    })
}

static ITEM_LIBRARY_QUERY: &str = r"
query PlexItemLibrary($id: ID!) {
  node(id: $id) { ... on Item { library { id } } }
}
";

#[derive(Debug, Deserialize)]
struct ItemLibraryData {
    node: Option<ItemLibrary>,
}

#[derive(Debug, Deserialize)]
struct ItemLibrary {
    library: Option<LibraryId>,
}

#[derive(Debug, Deserialize)]
struct LibraryId {
    id: String,
}

/// Shelves with nothing in them: no `hubKey`, no items.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EmptyShelf {
    key: String,
    title: &'static str,
    #[serde(rename = "type")]
    kind: &'static str,
    hub_identifier: &'static str,
    context: &'static str,
    size: u32,
    more: bool,
    style: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MusicShelvesContainer {
    size: usize,
    allow_sync: bool,
    identifier: &'static str,
    #[serde(rename = "Hub")]
    hub: Vec<EmptyShelf>,
}

/// `GET /hubs/metadata/{id}`, which the spec reserves for music: the reference answers an
/// artist's four shelves for any item, keyed by its number in its library, and with no music
/// they are empty. A raw `>` in a key is encoded, as on every shelf this server writes.
async fn music_shelves(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    Path(id): Path<String>,
    format: ResponseFormat,
) -> Result<PlexResponse<MusicShelvesContainer>, ApiError> {
    let item = crate::related::rating_key(&id)?;
    let mut variables = Variables::default();
    variables.insert(
        Name::new("id"),
        Value::String(GlobalId::encode(ITEM_TYPE, item)),
    );
    let data: ItemLibraryData = gql::execute(&state.schema, ITEM_LIBRARY_QUERY, variables).await?;
    let section = data
        .node
        .and_then(|node| node.library)
        .map(|library| crate::gql_items::database_id(&library.id))
        .ok_or(ApiError::NotFound)?;
    let catalog = Language::of(&client).catalog();
    let shelf = |key: String, title, kind, identifier| EmptyShelf {
        key,
        title,
        kind,
        hub_identifier: identifier,
        context: match identifier {
            "artist.mostplayedtracks" => "hub.artist.mostplayedtracks",
            "artist.mostpopulartracks" => "hub.artist.mostpopulartracks",
            "artist.albums" => "hub.artist.albums",
            _ => "hub.music.videos.popular.new",
        },
        size: 0,
        more: false,
        style: "shelf",
    };
    let all = format!("/library/sections/{section}/all?artist.id={item}");
    let hubs = vec![
        shelf(
            format!("{all}&type=10&sort=viewCount:desc&viewCount%3E=10"),
            catalog.most_played,
            "track",
            "artist.mostplayedtracks",
        ),
        shelf(
            format!("{all}&type=10&sort=ratingCount:desc&group=title"),
            catalog.most_popular_tracks,
            "track",
            "artist.mostpopulartracks",
        ),
        shelf(
            format!("{all}&type=9&sort=year:desc"),
            catalog.albums,
            "album",
            "artist.albums",
        ),
        shelf(
            format!("/library/sections/{section}/extras/all?extraType=4&type=8&artist.id={item}"),
            catalog.music_videos,
            "clip",
            "music.videos.popular.new",
        ),
    ];
    Ok(PlexResponse::new(
        MusicShelvesContainer {
            size: hubs.len(),
            allow_sync: true,
            identifier: LIBRARY_IDENTIFIER,
            hub: hubs,
        },
        format,
    ))
}
