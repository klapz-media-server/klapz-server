//! `/playQueues`: what a player will play next.
//!
//! What the client names is expanded here, through the official API, into the list of items
//! a queue holds; storing it, editing it and reading it back is the official API's too. A
//! playlist, named by `playlistID` or by its items' path, is played as it lists its items, and
//! the queue names it.
//!
//! Two answers are copied as the reference gives them although they look stale: removing an
//! entry answers with the container as it was before, and the entries around the entry that
//! was playing; a reset answers with the container as it was, and no entries.

use std::collections::HashMap;

use async_graphql::{Name, Value, Variables};
use axum::extract::{Path, RawQuery, State};
use axum::routing::{delete, get, post, put};
use axum::Router;
use open_plex_api::item::ITEM_TYPE;
use open_plex_api::library::LIBRARY_TYPE;
use open_plex_api::play_queue::{PLAY_QUEUE_ENTRY_TYPE, PLAY_QUEUE_TYPE};
use open_plex_api::playlist::PLAYLIST_TYPE;
use open_plex_api::GlobalId;
use open_plex_core::page::MAX_ITEMS;
use serde::Deserialize;
use serde_json::{json, Map, Value as Json};

use crate::auth::Caller;
use crate::client::ClientIdentity;
use crate::error::ApiError;
use crate::gql;
use crate::gql_items::{
    database_id, GqlItem, ItemsByIdData, SectionItemsData, ITEMS_BY_ID_QUERY, SECTION_ITEMS_QUERY,
};
use crate::items::{MetadataEntry, LIBRARY_IDENTIFIER, MEDIA_TAG_PREFIX, MEDIA_TAG_VERSION};
use crate::language::Language;
use crate::library::{item_type, shuffle_seed};
use crate::listing;
use crate::response::{PlexResponse, ResponseFormat};
use crate::PlexState;

pub fn router() -> Router<PlexState> {
    Router::new()
        .route("/playQueues", post(create))
        .route("/playQueues/{playQueueId}", get(read).put(add))
        .route("/playQueues/{playQueueId}/items", delete(clear))
        .route(
            "/playQueues/{playQueueId}/items/{playQueueItemId}",
            delete(remove),
        )
        .route(
            "/playQueues/{playQueueId}/items/{playQueueItemId}/move",
            put(move_entry),
        )
        .route("/playQueues/{playQueueId}/reset", put(reset))
        .route("/playQueues/{playQueueId}/shuffle", put(shuffle))
        .route("/playQueues/{playQueueId}/unshuffle", put(unshuffle))
}

/// What the reference shows on each side of the centre when not told otherwise.
const DEFAULT_WINDOW: i64 = 20;

const LIBRARY_AGENT: &str = "com.plexapp.plugins.library";

const EXPAND_QUERY: &str = r"
query PlexExpand($id: ID!, $first: Int!) {
  node(id: $id) {
    __typename
    ... on Item { id }
    ... on Show { episodes(first: $first) { edges { node { id } } } }
    ... on Season { episodes(first: $first) { edges { node { id } } } }
    ... on Episode { show { episodes(first: $first) { edges { node { id } } } } }
  }
}
";

#[derive(Debug, Deserialize)]
struct ExpandData {
    node: Option<GqlExpanded>,
}

#[derive(Debug, Deserialize)]
struct GqlExpanded {
    #[serde(rename = "__typename")]
    typename: String,
    id: Option<String>,
    episodes: Option<GqlEpisodes>,
    show: Option<GqlShowEpisodes>,
}

#[derive(Debug, Deserialize)]
struct GqlShowEpisodes {
    episodes: GqlEpisodes,
}

#[derive(Debug, Deserialize)]
struct GqlEpisodes {
    edges: Vec<GqlEpisodeEdge>,
}

impl GqlEpisodes {
    fn ids(self) -> Vec<String> {
        self.edges.into_iter().map(|edge| edge.node.id).collect()
    }
}

#[derive(Debug, Deserialize)]
struct GqlEpisodeEdge {
    node: GqlIdentified,
}

#[derive(Debug, Deserialize)]
struct GqlIdentified {
    id: String,
}

/// The path inside `server://{machine}/com.plexapp.plugins.library{path}`, or `None` when it
/// names another server or is not such a URI at all.
pub(crate) fn library_path<'a>(uri: &'a str, machine: &str) -> Option<&'a str> {
    let rest = uri.strip_prefix("server://")?;
    let (host, rest) = rest.split_once('/')?;
    if host != machine {
        return None;
    }
    rest.strip_prefix(LIBRARY_AGENT)
}

/// The reference restates what the queue was built from in a form of its own: one item, or a
/// directory of them.
pub(crate) fn source_uri(path: &str) -> String {
    let single = path
        .strip_prefix("/library/metadata/")
        .is_some_and(|key| key.bytes().all(|byte| byte.is_ascii_digit()));
    let kind = if single { "item" } else { "directory" };
    let encoded: String = form_urlencoded::byte_serialize(path.as_bytes()).collect();
    format!("library://x/{kind}/{encoded}")
}

/// The global ids of what a path names, in play order. Empty for anything that names nothing,
/// which is a valid, empty queue on the reference. `continuous` plays an episode on to the end
/// of its series, as the reference does; on anything else it changes nothing.
pub(crate) async fn expand(
    state: &PlexState,
    path: &str,
    continuous: bool,
) -> Result<Vec<String>, ApiError> {
    if let Some(keys) = path.strip_prefix("/library/metadata/") {
        let keys: Vec<i64> = keys.split(',').filter_map(|key| key.parse().ok()).collect();
        if let [key] = keys[..] {
            return expand_item(state, key, continuous).await;
        }
        return existing(state, keys).await;
    }
    if let Some(rest) = path.strip_prefix("/library/sections/") {
        return expand_section(state, rest).await;
    }
    Ok(Vec::new())
}

async fn expand_item(
    state: &PlexState,
    key: i64,
    continuous: bool,
) -> Result<Vec<String>, ApiError> {
    let mut variables = Variables::default();
    variables.insert(
        Name::new("id"),
        Value::String(GlobalId::encode(ITEM_TYPE, key)),
    );
    variables.insert(Name::new("first"), Value::Number(MAX_ITEMS.into()));
    let data: ExpandData = gql::execute(&state.schema, EXPAND_QUERY, variables).await?;
    let Some(node) = data.node else {
        return Ok(Vec::new());
    };
    Ok(match node.typename.as_str() {
        "Show" | "Season" => node.episodes.map(GqlEpisodes::ids).unwrap_or_default(),
        "Episode" if continuous => {
            let own = node.id.unwrap_or_default();
            let series = node
                .show
                .map(|show| show.episodes.ids())
                .unwrap_or_default();
            match series.iter().position(|id| *id == own) {
                Some(start) => series[start..].to_vec(),
                None => vec![own],
            }
        }
        _ => node.id.into_iter().collect(),
    })
}

async fn existing(state: &PlexState, keys: Vec<i64>) -> Result<Vec<String>, ApiError> {
    let found = fetch_items(state, &keys).await?;
    Ok(found.into_iter().flatten().map(|item| item.id).collect())
}

/// `4/all?type=1&sort=addedAt:desc`, read as the listing would read it.
async fn expand_section(state: &PlexState, rest: &str) -> Result<Vec<String>, ApiError> {
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let Some(section) = path
        .strip_suffix("/all")
        .and_then(|id| id.parse::<i64>().ok())
    else {
        return Ok(Vec::new());
    };
    let pairs: Vec<(String, String)> = form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect();
    let request = listing::parse(&pairs)?;
    let (order_by, direction) = request.ordering();

    let mut variables = Variables::default();
    variables.insert(
        Name::new("id"),
        Value::String(GlobalId::encode(LIBRARY_TYPE, section)),
    );
    variables.insert(Name::new("first"), Value::Number(MAX_ITEMS.into()));
    variables.insert(Name::new("offset"), Value::Number(0.into()));
    variables.insert(Name::new("orderBy"), Value::Enum(Name::new(order_by)));
    variables.insert(Name::new("direction"), Value::Enum(Name::new(direction)));
    variables.insert(Name::new("seed"), Value::Number(shuffle_seed().into()));
    variables.insert(
        Name::new("filter"),
        request.filters.to_graphql(item_type(request.kind)),
    );
    let data: SectionItemsData =
        gql::execute(&state.schema, &SECTION_ITEMS_QUERY, variables).await?;
    Ok(data
        .node
        .map(|library| {
            library
                .items
                .edges
                .into_iter()
                .map(|edge| edge.node.id)
                .collect()
        })
        .unwrap_or_default())
}

async fn fetch_items(state: &PlexState, keys: &[i64]) -> Result<Vec<Option<GqlItem>>, ApiError> {
    let mut variables = Variables::default();
    variables.insert(
        Name::new("ids"),
        Value::List(
            keys.iter()
                .map(|key| Value::String(GlobalId::encode(ITEM_TYPE, *key)))
                .collect(),
        ),
    );
    let data: ItemsByIdData = gql::execute(&state.schema, &ITEMS_BY_ID_QUERY, variables).await?;
    Ok(data.items)
}

/// The playlist a library path names: `/playlists/{id}/items`.
fn playlist_of(path: &str) -> Option<i64> {
    path.strip_prefix("/playlists/")?
        .strip_suffix("/items")?
        .parse()
        .ok()
}

const PLAYLIST_ITEMS_QUERY: &str = r"
query PlexPlaylistQueue($id: ID!, $first: Int!) {
  node(id: $id) { ... on Playlist { items(first: $first) { edges { node { id } } } } }
}
";

#[derive(Debug, Deserialize)]
struct PlaylistItemsData {
    node: Option<GqlPlaylistItems>,
}

#[derive(Debug, Deserialize)]
struct GqlPlaylistItems {
    items: GqlEpisodes,
}

/// What a playlist of the caller's lists, in order, or `None` for any other.
async fn playlist_items(state: &PlexState, key: i64) -> Result<Option<Vec<String>>, ApiError> {
    let mut variables = Variables::default();
    variables.insert(
        Name::new("id"),
        Value::String(GlobalId::encode(PLAYLIST_TYPE, key)),
    );
    variables.insert(Name::new("first"), Value::Number(MAX_ITEMS.into()));
    let data: PlaylistItemsData =
        gql::execute(&state.schema, PLAYLIST_ITEMS_QUERY, variables).await?;
    Ok(data.node.map(|playlist| playlist.items.ids()))
}

fn parameters(query: Option<&str>) -> HashMap<String, String> {
    form_urlencoded::parse(query.unwrap_or_default().as_bytes())
        .into_owned()
        .collect()
}

fn flag(params: &HashMap<String, String>, name: &str) -> bool {
    params.get(name).is_some_and(|value| value == "1")
}

/// What a request names to play: a playlist by `playlistID` or by its path, or a `uri`.
enum Content<'a> {
    Playlist(i64),
    Library(Option<&'a str>),
}

fn content<'a>(
    params: &'a HashMap<String, String>,
    machine: &str,
) -> Result<Content<'a>, ApiError> {
    if let Some(key) = params.get("playlistID") {
        return Ok(Content::Playlist(
            key.parse().map_err(|_| ApiError::BadRequest)?,
        ));
    }
    let uri = params.get("uri").ok_or(ApiError::BadRequest)?;
    let path = library_path(uri, machine);
    Ok(match path.and_then(playlist_of) {
        Some(key) => Content::Playlist(key),
        None => Content::Library(path),
    })
}

const CREATE_MUTATION: &str = r"
mutation PlexCreatePlayQueue($input: CreatePlayQueueInput!) {
  createPlayQueue(input: $input) { id }
}
";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreatedData {
    create_play_queue: GqlIdentified,
}

fn input_variables(fields: Vec<(&str, Value)>) -> Variables {
    let input = fields
        .into_iter()
        .map(|(name, value)| (Name::new(name), value))
        .collect();
    let mut variables = Variables::default();
    variables.insert(Name::new("input"), Value::Object(input));
    variables
}

fn id_list(ids: Vec<String>) -> Value {
    Value::List(ids.into_iter().map(Value::String).collect())
}

async fn create(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    RawQuery(query): RawQuery,
    format: ResponseFormat,
) -> Result<PlexResponse<Json>, ApiError> {
    let params = parameters(query.as_deref());
    // Any `type` is a video queue on the reference; it still keeps one queue per type named.
    let kind = match params.get("type").ok_or(ApiError::BadRequest)?.as_str() {
        "audio" => "AUDIO",
        "photo" => "PHOTO",
        _ => "VIDEO",
    };
    let range = Range::parse(query.as_deref().unwrap_or_default())?;
    let machine = crate::gql::machine_identifier(&state.identity.machine_identifier);
    let (items, source, playlist) = match content(&params, &machine)? {
        Content::Playlist(key) => (
            playlist_items(&state, key)
                .await?
                .ok_or(ApiError::NotFound)?,
            String::new(),
            Value::String(GlobalId::encode(PLAYLIST_TYPE, key)),
        ),
        Content::Library(path) => {
            let items = match path {
                Some(path) => expand(&state, path, flag(&params, "continuous")).await?,
                None => Vec::new(),
            };
            let source = match path {
                Some(path) => source_uri(path),
                None => params.get("uri").cloned().unwrap_or_default(),
            };
            (items, source, Value::Null)
        }
    };
    let selected = params
        .get("key")
        .and_then(|key| key.strip_prefix("/library/metadata/"))
        .and_then(|key| key.parse::<i64>().ok())
        .and_then(|key| items.iter().position(|id| database_id(id) == key))
        .unwrap_or(0);
    let client_identifier = client
        .identifier
        .as_ref()
        .map_or(Value::Null, |id| Value::String(id.as_str().to_owned()));

    let variables = input_variables(vec![
        ("sourceUri", Value::String(source)),
        ("playlistId", playlist),
        ("itemIds", id_list(items)),
        (
            "selectedPosition",
            Value::Number(u32::try_from(selected).unwrap_or(0).into()),
        ),
        ("shuffle", Value::Boolean(flag(&params, "shuffle"))),
        ("clientIdentifier", client_identifier),
        ("kind", Value::Enum(Name::new(kind))),
    ]);
    let data: CreatedData = gql::execute(&state.schema, CREATE_MUTATION, variables).await?;
    let queue = load(&state, &data.create_play_queue.id).await?;
    let body = render(&state, &queue, &queue, &range, Language::of(&client)).await?;
    Ok(PlexResponse::new(body, format))
}

/// Which entries to show, around which one.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Range {
    /// `None` is the queue's selected entry.
    center: Option<i64>,
    window: i64,
    before: bool,
    after: bool,
    /// Wraps around the queue's ends to fill the window, when the queue is long enough.
    repeat: bool,
}

impl Default for Range {
    fn default() -> Self {
        Self {
            center: None,
            window: DEFAULT_WINDOW,
            before: true,
            after: true,
            repeat: false,
        }
    }
}

impl Range {
    fn parse(query: &str) -> Result<Self, ApiError> {
        let params: HashMap<_, _> = form_urlencoded::parse(query.as_bytes()).collect();
        let number = |name: &str| -> Result<Option<i64>, ApiError> {
            params
                .get(name)
                .map(|value| value.parse().map_err(|_| ApiError::BadRequest))
                .transpose()
        };
        let range = Self {
            center: number("center")?,
            window: number("window")?.unwrap_or(DEFAULT_WINDOW),
            before: number("includeBefore")? != Some(0),
            after: number("includeAfter")? != Some(0),
            repeat: number("repeat")? == Some(1),
        };
        if !range.before && !range.after {
            return Err(ApiError::BadRequest);
        }
        Ok(range)
    }

    /// The runs of positions to show, as `(first position, how many)`, in display order. The
    /// centre itself is shown only when both sides are: with either refused, the reference
    /// returns just the other side, and wraps nothing.
    ///
    /// With `repeat`, a window clipped by one end of the queue takes what it lacks from the
    /// other end, never showing an entry twice: on a queue of 5, a window of 1 around the last
    /// entry shows the fourth, the fifth and the first.
    fn segments(&self, center: i64, total: i64) -> Vec<(i64, i64)> {
        if self.window <= 0 || total <= 0 {
            return Vec::new();
        }
        let start = if self.before {
            (center - self.window).max(0)
        } else {
            center + 1
        };
        let end = if self.after {
            (center + self.window).min(total - 1)
        } else {
            center - 1
        };
        let shown = (end - start + 1).max(0);
        let mut segments = vec![(start, shown)];
        if self.repeat && self.before && self.after {
            let wanted = (2 * self.window + 1).min(total);
            let missing = wanted - shown;
            if missing > 0 && center - self.window < 0 {
                segments.insert(0, (total - missing, missing));
            } else if missing > 0 {
                segments.push((0, missing));
            }
        }
        segments.retain(|(_, count)| *count > 0);
        segments
    }
}

const QUEUE_QUERY: &str = r"
query PlexPlayQueue($id: ID!) {
  node(id: $id) {
    ... on PlayQueue {
      id
      sourceUri
      shuffled
      version
      totalCount
      selectedEntry { id position item { id } }
      lastAddedEntry { id }
      playlist { id title }
    }
  }
}
";

const CENTER_QUERY: &str = r"
query PlexPlayQueueCenter($id: ID!) {
  node(id: $id) { ... on PlayQueueEntry { queueId position } }
}
";

const ENTRIES_QUERY: &str = r"
query PlexPlayQueueEntries($id: ID!, $first: Int!, $offset: Int!) {
  node(id: $id) {
    ... on PlayQueue {
      entries(first: $first, offset: $offset) {
        edges { node { id item { id } } }
      }
    }
  }
}
";

#[derive(Debug, Deserialize)]
struct QueueData {
    node: Option<GqlQueue>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlQueue {
    id: String,
    source_uri: String,
    shuffled: bool,
    version: i32,
    total_count: i64,
    selected_entry: Option<GqlEntry>,
    last_added_entry: Option<GqlIdentified>,
    playlist: Option<GqlQueuePlaylist>,
}

#[derive(Debug, Deserialize)]
struct GqlQueuePlaylist {
    id: String,
    title: String,
}

#[derive(Debug, Deserialize)]
struct GqlEntry {
    id: String,
    #[serde(default)]
    position: i64,
    item: Option<GqlIdentified>,
}

#[derive(Debug, Deserialize)]
struct CenterData {
    node: Option<GqlCenter>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlCenter {
    queue_id: Option<String>,
    position: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct EntriesData {
    node: Option<GqlEntries>,
}

#[derive(Debug, Deserialize)]
struct GqlEntries {
    entries: GqlEntryConnection,
}

#[derive(Debug, Deserialize)]
struct GqlEntryConnection {
    edges: Vec<GqlEntryEdge>,
}

#[derive(Debug, Deserialize)]
struct GqlEntryEdge {
    node: GqlEntry,
}

fn id_variable(id: String) -> Variables {
    let mut variables = Variables::default();
    variables.insert(Name::new("id"), Value::String(id));
    variables
}

/// One of the caller's queues, or `404`.
async fn load(state: &PlexState, queue_id: &str) -> Result<GqlQueue, ApiError> {
    let data: QueueData =
        gql::execute(&state.schema, QUEUE_QUERY, id_variable(queue_id.to_owned())).await?;
    data.node.ok_or(ApiError::NotFound)
}

/// Where an entry stands in a queue. `None` when it is not in this one.
async fn entry_position(
    state: &PlexState,
    queue_id: &str,
    entry: i64,
) -> Result<Option<i64>, ApiError> {
    let variables = id_variable(GlobalId::encode(PLAY_QUEUE_ENTRY_TYPE, entry));
    let data: CenterData = gql::execute(&state.schema, CENTER_QUERY, variables).await?;
    Ok(data
        .node
        .filter(|entry| entry.queue_id.as_deref() == Some(queue_id))
        .and_then(|entry| entry.position))
}

async fn window_entries(
    state: &PlexState,
    queue_id: &str,
    start: i64,
    count: i64,
) -> Result<Vec<GqlEntry>, ApiError> {
    if count <= 0 {
        return Ok(Vec::new());
    }
    let mut variables = id_variable(queue_id.to_owned());
    variables.insert(
        Name::new("first"),
        Value::Number(
            u32::try_from(count)
                .unwrap_or(MAX_ITEMS)
                .min(MAX_ITEMS)
                .into(),
        ),
    );
    variables.insert(
        Name::new("offset"),
        Value::Number(u32::try_from(start).unwrap_or(0).into()),
    );
    let data: EntriesData = gql::execute(&state.schema, ENTRIES_QUERY, variables).await?;
    Ok(data
        .node
        .map(|node| {
            node.entries
                .edges
                .into_iter()
                .map(|edge| edge.node)
                .collect()
        })
        .unwrap_or_default())
}

/// An item as the reference shows it in a queue: a lookup, with its queue entry first.
fn queue_item(entry_id: i64, item: &GqlItem, language: Language) -> Option<Json> {
    let metadata = MetadataEntry::from_graphql(item, true, language)?;
    let Json::Object(fields) = serde_json::to_value(metadata).ok()? else {
        return None;
    };
    let mut ordered = Map::with_capacity(fields.len() + 1);
    ordered.insert("playQueueItemID".to_owned(), json!(entry_id));
    ordered.extend(fields);
    Some(Json::Object(ordered))
}

/// The entries `range` shows of `current`, as the reference lists them.
async fn metadata(
    state: &PlexState,
    current: &GqlQueue,
    center: Option<i64>,
    range: &Range,
    language: Language,
) -> Result<Vec<Json>, ApiError> {
    let Some(center) = center else {
        return Ok(Vec::new());
    };
    let mut entries = Vec::new();
    for (start, count) in range.segments(center, current.total_count) {
        entries.extend(window_entries(state, &current.id, start, count).await?);
    }
    let keys: Vec<i64> = entries
        .iter()
        .filter_map(|entry| entry.item.as_ref().map(|item| database_id(&item.id)))
        .collect();
    let items = if keys.is_empty() {
        Vec::new()
    } else {
        fetch_items(state, &keys).await?
    };
    let shown = entries.iter().filter(|entry| entry.item.is_some());
    Ok(shown
        .zip(items.iter())
        .filter_map(|(entry, item)| queue_item(database_id(&entry.id), item.as_ref()?, language))
        .collect())
}

/// The container's own attributes, from `header`, around `metadata`.
fn container(header: &GqlQueue, metadata: Vec<Json>) -> Json {
    let mut body = Map::new();
    body.insert("size".into(), json!(metadata.len()));
    body.insert("identifier".into(), json!(LIBRARY_IDENTIFIER));
    body.insert("mediaTagPrefix".into(), json!(MEDIA_TAG_PREFIX));
    body.insert("mediaTagVersion".into(), json!(MEDIA_TAG_VERSION));
    body.insert("playQueueID".into(), json!(database_id(&header.id)));
    if let Some(added) = &header.last_added_entry {
        body.insert(
            "playQueueLastAddedItemID".into(),
            json!(database_id(&added.id)),
        );
    }
    if let Some(playlist) = &header.playlist {
        body.insert(
            "playQueuePlaylistID".into(),
            json!(database_id(&playlist.id)),
        );
        body.insert("playQueuePlaylistTitle".into(), json!(playlist.title));
    }
    if let Some(selected) = &header.selected_entry {
        body.insert(
            "playQueueSelectedItemID".into(),
            json!(database_id(&selected.id)),
        );
        body.insert(
            "playQueueSelectedItemOffset".into(),
            json!(selected.position),
        );
        if let Some(item) = &selected.item {
            body.insert(
                "playQueueSelectedMetadataItemID".into(),
                json!(database_id(&item.id).to_string()),
            );
        }
    }
    body.insert("playQueueShuffled".into(), json!(header.shuffled));
    // A playlist's queue names the playlist instead.
    if header.playlist.is_none() {
        body.insert("playQueueSourceURI".into(), json!(header.source_uri));
    }
    if header.total_count > 0 {
        body.insert("playQueueTotalCount".into(), json!(header.total_count));
    }
    body.insert("playQueueVersion".into(), json!(header.version));
    if !metadata.is_empty() {
        body.insert("Metadata".into(), Json::Array(metadata));
    }
    Json::Object(body)
}

/// The container of `header`, around the entries of `current` that `range` shows.
async fn render(
    state: &PlexState,
    header: &GqlQueue,
    current: &GqlQueue,
    range: &Range,
    language: Language,
) -> Result<Json, ApiError> {
    let center = match range.center {
        Some(entry) => entry_position(state, &current.id, entry).await?,
        None => current.selected_entry.as_ref().map(|entry| entry.position),
    };
    let metadata = metadata(state, current, center, range, language).await?;
    Ok(container(header, metadata))
}

async fn read(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    Path(queue): Path<i64>,
    RawQuery(query): RawQuery,
    format: ResponseFormat,
) -> Result<PlexResponse<Json>, ApiError> {
    let range = Range::parse(query.as_deref().unwrap_or_default())?;
    let queue = load(&state, &GlobalId::encode(PLAY_QUEUE_TYPE, queue)).await?;
    let body = render(&state, &queue, &queue, &range, Language::of(&client)).await?;
    Ok(PlexResponse::new(body, format))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlEditPayload {
    user_errors: Vec<GqlUserError>,
}

#[derive(Debug, Deserialize)]
struct GqlUserError {
    code: String,
}

/// Runs one of the official API's queue edits. A refusal is `404`, as each one is on the
/// reference: the queue, the entry or the reordering does not exist for this caller.
async fn edit(state: &PlexState, mutation: &str, variables: Variables) -> Result<(), ApiError> {
    let data: HashMap<String, GqlEditPayload> =
        gql::execute(&state.schema, mutation, variables).await?;
    let refused = data
        .values()
        .flat_map(|payload| &payload.user_errors)
        .next();
    match refused {
        Some(error) => {
            tracing::debug!(code = %error.code, "a play queue edit was refused");
            Err(ApiError::NotFound)
        }
        None => Ok(()),
    }
}

const ADD_MUTATION: &str = r"
mutation PlexAddToPlayQueue($input: AddToPlayQueueInput!) {
  addToPlayQueue(input: $input) { userErrors { code } }
}
";

async fn add(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    Path(queue): Path<i64>,
    RawQuery(query): RawQuery,
    format: ResponseFormat,
) -> Result<PlexResponse<Json>, ApiError> {
    let params = parameters(query.as_deref());
    let queue_id = GlobalId::encode(PLAY_QUEUE_TYPE, queue);
    let machine = crate::gql::machine_identifier(&state.identity.machine_identifier);
    let content = content(&params, &machine)?;
    load(&state, &queue_id).await?;
    // What names nothing — an unknown playlist included — adds nothing, with a `200`.
    let items = match content {
        Content::Playlist(key) => playlist_items(&state, key).await?.unwrap_or_default(),
        Content::Library(Some(path)) => expand(&state, path, flag(&params, "continuous")).await?,
        Content::Library(None) => Vec::new(),
    };
    let variables = input_variables(vec![
        ("playQueueId", Value::String(queue_id.clone())),
        ("itemIds", id_list(items)),
        ("next", Value::Boolean(flag(&params, "next"))),
    ]);
    edit(&state, ADD_MUTATION, variables).await?;
    let queue = load(&state, &queue_id).await?;
    let body = render(
        &state,
        &queue,
        &queue,
        &Range::default(),
        Language::of(&client),
    )
    .await?;
    Ok(PlexResponse::new(body, format))
}

const CLEAR_MUTATION: &str = r"
mutation PlexClearPlayQueue($input: PlayQueueInput!) {
  clearPlayQueue(input: $input) { userErrors { code } }
}
";

const RESET_MUTATION: &str = r"
mutation PlexResetPlayQueue($input: PlayQueueInput!) {
  resetPlayQueue(input: $input) { userErrors { code } }
}
";

const SHUFFLE_MUTATION: &str = r"
mutation PlexShufflePlayQueue($input: PlayQueueInput!) {
  shufflePlayQueue(input: $input) { userErrors { code } }
}
";

const UNSHUFFLE_MUTATION: &str = r"
mutation PlexUnshufflePlayQueue($input: PlayQueueInput!) {
  unshufflePlayQueue(input: $input) { userErrors { code } }
}
";

/// Edits a whole queue, and answers with it as it now stands.
async fn edit_whole(
    state: &PlexState,
    client: &ClientIdentity,
    queue: i64,
    mutation: &str,
    format: ResponseFormat,
) -> Result<PlexResponse<Json>, ApiError> {
    let queue_id = GlobalId::encode(PLAY_QUEUE_TYPE, queue);
    let variables = input_variables(vec![("playQueueId", Value::String(queue_id.clone()))]);
    edit(state, mutation, variables).await?;
    let queue = load(state, &queue_id).await?;
    let body = render(
        state,
        &queue,
        &queue,
        &Range::default(),
        Language::of(client),
    )
    .await?;
    Ok(PlexResponse::new(body, format))
}

async fn clear(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    Path(queue): Path<i64>,
    format: ResponseFormat,
) -> Result<PlexResponse<Json>, ApiError> {
    edit_whole(&state, &client, queue, CLEAR_MUTATION, format).await
}

async fn shuffle(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    Path(queue): Path<i64>,
    format: ResponseFormat,
) -> Result<PlexResponse<Json>, ApiError> {
    edit_whole(&state, &client, queue, SHUFFLE_MUTATION, format).await
}

async fn unshuffle(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    Path(queue): Path<i64>,
    format: ResponseFormat,
) -> Result<PlexResponse<Json>, ApiError> {
    edit_whole(&state, &client, queue, UNSHUFFLE_MUTATION, format).await
}

async fn reset(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(queue): Path<i64>,
    format: ResponseFormat,
) -> Result<PlexResponse<Json>, ApiError> {
    let queue_id = GlobalId::encode(PLAY_QUEUE_TYPE, queue);
    let before = load(&state, &queue_id).await?;
    let variables = input_variables(vec![("playQueueId", Value::String(queue_id))]);
    edit(&state, RESET_MUTATION, variables).await?;
    Ok(PlexResponse::new(container(&before, Vec::new()), format))
}

const REMOVE_MUTATION: &str = r"
mutation PlexRemoveFromPlayQueue($input: RemoveFromPlayQueueInput!) {
  removeFromPlayQueue(input: $input) { userErrors { code } }
}
";

async fn remove(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    Path((queue, entry)): Path<(i64, i64)>,
    format: ResponseFormat,
) -> Result<PlexResponse<Json>, ApiError> {
    let queue_id = GlobalId::encode(PLAY_QUEUE_TYPE, queue);
    let before = load(&state, &queue_id).await?;
    entry_position(&state, &queue_id, entry)
        .await?
        .ok_or(ApiError::NotFound)?;
    let entry_id = GlobalId::encode(PLAY_QUEUE_ENTRY_TYPE, entry);
    let variables = input_variables(vec![("entryId", Value::String(entry_id))]);
    edit(&state, REMOVE_MUTATION, variables).await?;
    let after = load(&state, &queue_id).await?;
    let mut header = before;
    let center = header
        .selected_entry
        .as_ref()
        .map(|selected| database_id(&selected.id));
    // The entry that was playing is looked up again: gone, it stands at 0 with no item.
    if let (Some(selected), Some(id)) = (header.selected_entry.as_mut(), center) {
        let position = entry_position(&state, &queue_id, id).await?;
        selected.position = position.unwrap_or(0);
        if position.is_none() {
            selected.item = None;
        }
    }
    let range = Range {
        center,
        ..Range::default()
    };
    let body = render(&state, &header, &after, &range, Language::of(&client)).await?;
    Ok(PlexResponse::new(body, format))
}

const MOVE_MUTATION: &str = r"
mutation PlexMovePlayQueueEntry($input: MovePlayQueueEntryInput!) {
  movePlayQueueEntry(input: $input) { userErrors { code } }
}
";

const FIRST_ENTRIES_QUERY: &str = r"
query PlexPlayQueueHead($id: ID!) {
  node(id: $id) { ... on PlayQueue { entries(first: 2) { edges { node { id } } } } }
}
";

#[derive(Debug, Deserialize)]
struct HeadData {
    node: Option<GqlHead>,
}

#[derive(Debug, Deserialize)]
struct GqlHead {
    entries: GqlEpisodes,
}

/// The entry to move after, as the reference reads `after`: absent or `-1` is first, an entry
/// of this queue is that entry, and anything else is the first entry other than the one moved.
async fn move_anchor(
    state: &PlexState,
    queue_id: &str,
    entry: i64,
    after: Option<i64>,
) -> Result<Option<String>, ApiError> {
    let Some(after) = after.filter(|after| *after != -1) else {
        return Ok(None);
    };
    if after == entry || entry_position(state, queue_id, after).await?.is_some() {
        return Ok(Some(GlobalId::encode(PLAY_QUEUE_ENTRY_TYPE, after)));
    }
    let data: HeadData = gql::execute(
        &state.schema,
        FIRST_ENTRIES_QUERY,
        id_variable(queue_id.to_owned()),
    )
    .await?;
    let moved = GlobalId::encode(PLAY_QUEUE_ENTRY_TYPE, entry);
    Ok(data
        .node
        .map(|head| head.entries.ids())
        .unwrap_or_default()
        .into_iter()
        .find(|id| *id != moved))
}

async fn move_entry(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    Path((queue, entry)): Path<(i64, i64)>,
    RawQuery(query): RawQuery,
    format: ResponseFormat,
) -> Result<PlexResponse<Json>, ApiError> {
    let params = parameters(query.as_deref());
    let after = params
        .get("after")
        .map(|after| after.parse::<i64>().map_err(|_| ApiError::BadRequest))
        .transpose()?;
    let queue_id = GlobalId::encode(PLAY_QUEUE_TYPE, queue);
    load(&state, &queue_id).await?;
    entry_position(&state, &queue_id, entry)
        .await?
        .ok_or(ApiError::NotFound)?;
    let anchor = move_anchor(&state, &queue_id, entry, after).await?;
    let variables = input_variables(vec![
        (
            "entryId",
            Value::String(GlobalId::encode(PLAY_QUEUE_ENTRY_TYPE, entry)),
        ),
        ("afterEntryId", anchor.map_or(Value::Null, Value::String)),
    ]);
    edit(&state, MOVE_MUTATION, variables).await?;
    let queue = load(&state, &queue_id).await?;
    let body = render(
        &state,
        &queue,
        &queue,
        &Range::default(),
        Language::of(&client),
    )
    .await?;
    Ok(PlexResponse::new(body, format))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    const MACHINE: &str = "abc";

    #[test]
    fn reads_the_path_out_of_a_server_uri() {
        assert_eq!(
            library_path(
                "server://abc/com.plexapp.plugins.library/library/metadata/1",
                MACHINE
            ),
            Some("/library/metadata/1")
        );
    }

    #[test]
    fn another_server_or_a_stranger_names_nothing() {
        assert_eq!(library_path("nonsense", MACHINE), None);
        assert_eq!(
            library_path(
                "server://deadbeef/com.plexapp.plugins.library/library/metadata/1",
                MACHINE
            ),
            None
        );
    }

    #[test]
    fn reads_the_playlist_a_path_names() {
        assert_eq!(playlist_of("/playlists/578387/items"), Some(578_387));
        assert_eq!(playlist_of("/playlists/578387"), None);
        assert_eq!(playlist_of("/library/metadata/1"), None);
    }

    #[test]
    fn restates_the_source_as_the_reference_does() {
        assert_eq!(
            source_uri("/library/metadata/577307"),
            "library://x/item/%2Flibrary%2Fmetadata%2F577307"
        );
        assert_eq!(
            source_uri("/library/sections/4/all?type=1&sort=addedAt:desc"),
            "library://x/directory/%2Flibrary%2Fsections%2F4%2Fall%3Ftype%3D1%26sort%3DaddedAt%3Adesc"
        );
        assert_eq!(
            source_uri("/library/metadata/577307,362768"),
            "library://x/directory/%2Flibrary%2Fmetadata%2F577307%2C362768"
        );
    }

    fn range(query: &str) -> Range {
        Range::parse(query).unwrap()
    }

    // Each span below is what the reference answered, on a queue of 22 centred on position 4.
    #[test]
    fn shows_the_window_on_both_sides() {
        assert_eq!(Range::default().segments(4, 22), [(0, 22)]);
        assert_eq!(range("window=4").segments(4, 22), [(0, 9)]);
        assert_eq!(range("window=5").segments(4, 22), [(0, 10)]);
        assert_eq!(range("window=4").segments(10, 22), [(6, 9)]);
    }

    #[test]
    fn a_new_queue_starting_at_its_first_entry_shows_twenty_one() {
        assert_eq!(Range::default().segments(0, 261), [(0, 21)]);
    }

    #[test]
    fn refusing_a_side_also_drops_the_centre() {
        assert_eq!(range("includeBefore=0").segments(4, 22), [(5, 17)]);
        assert_eq!(range("includeAfter=0").segments(4, 22), [(0, 4)]);
    }

    #[test]
    fn a_window_of_zero_shows_nothing() {
        assert!(range("window=0").segments(4, 22).is_empty());
    }

    // Measured on a queue of 5 entries, positions 0 to 4.
    #[test]
    fn repeat_wraps_a_clipped_window_around_the_other_end() {
        let flatten = |query: &str, center: i64| -> Vec<i64> {
            range(query)
                .segments(center, 5)
                .into_iter()
                .flat_map(|(start, count)| start..start + count)
                .collect()
        };
        assert_eq!(flatten("repeat=1&window=1", 0), [4, 0, 1]);
        assert_eq!(flatten("repeat=1&window=1", 4), [3, 4, 0]);
        assert_eq!(flatten("repeat=1&window=2", 4), [2, 3, 4, 0, 1]);
        assert_eq!(flatten("repeat=1&window=3", 4), [1, 2, 3, 4, 0]);
        assert_eq!(flatten("repeat=1&window=2", 0), [3, 4, 0, 1, 2]);
        assert_eq!(flatten("repeat=1&window=4", 2), [0, 1, 2, 3, 4]);
        assert_eq!(flatten("repeat=1&window=2", 3), [1, 2, 3, 4, 0]);
        assert_eq!(flatten("repeat=1&window=1&includeBefore=0", 4), [0; 0]);
        assert_eq!(flatten("repeat=0&window=1", 4), [3, 4]);
    }

    #[test]
    fn a_queue_shorter_than_the_window_does_not_repeat() {
        assert_eq!(range("repeat=1&window=1").segments(1, 2), [(0, 2)]);
    }

    #[test]
    fn refuses_what_it_cannot_read() {
        assert!(matches!(
            Range::parse("includeBefore=0&includeAfter=0"),
            Err(ApiError::BadRequest)
        ));
        assert!(matches!(
            Range::parse("window=abc"),
            Err(ApiError::BadRequest)
        ));
    }
}
