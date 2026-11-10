//! `POST /playQueues` and `GET /playQueues/{id}`: what a player will play next.
//!
//! What the client names is expanded here, through the official API, into the list of items
//! a queue holds; storing it and reading it back is the official API's too. Editing a queue is
//! not served yet.

use std::collections::HashMap;

use async_graphql::{Name, Value, Variables};
use axum::extract::{Path, RawQuery, State};
use axum::routing::{get, post};
use axum::Router;
use open_plex_api::item::ITEM_TYPE;
use open_plex_api::library::LIBRARY_TYPE;
use open_plex_api::play_queue::{PLAY_QUEUE_ENTRY_TYPE, PLAY_QUEUE_TYPE};
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
        .route("/playQueues/{playQueueId}", get(read).put(refuse_change))
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
}

#[derive(Debug, Deserialize)]
struct GqlEpisodes {
    edges: Vec<GqlEpisodeEdge>,
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
fn library_path<'a>(uri: &'a str, machine: &str) -> Option<&'a str> {
    let rest = uri.strip_prefix("server://")?;
    let (host, rest) = rest.split_once('/')?;
    if host != machine {
        return None;
    }
    rest.strip_prefix(LIBRARY_AGENT)
}

/// The reference restates what the queue was built from in a form of its own: one item, or a
/// directory of them.
fn source_uri(path: &str) -> String {
    let single = path
        .strip_prefix("/library/metadata/")
        .is_some_and(|key| key.bytes().all(|byte| byte.is_ascii_digit()));
    let kind = if single { "item" } else { "directory" };
    let encoded: String = form_urlencoded::byte_serialize(path.as_bytes()).collect();
    format!("library://x/{kind}/{encoded}")
}

/// The global ids of what a path names, in play order. Empty for anything that names nothing,
/// which is a valid, empty queue on the reference.
async fn expand(state: &PlexState, path: &str) -> Result<Vec<String>, ApiError> {
    if let Some(keys) = path.strip_prefix("/library/metadata/") {
        let keys: Vec<i64> = keys.split(',').filter_map(|key| key.parse().ok()).collect();
        if let [key] = keys[..] {
            return expand_item(state, key).await;
        }
        return existing(state, keys).await;
    }
    if let Some(rest) = path.strip_prefix("/library/sections/") {
        return expand_section(state, rest).await;
    }
    Ok(Vec::new())
}

async fn expand_item(state: &PlexState, key: i64) -> Result<Vec<String>, ApiError> {
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
        "Show" | "Season" => node
            .episodes
            .map(|episodes| {
                episodes
                    .edges
                    .into_iter()
                    .map(|edge| edge.node.id)
                    .collect()
            })
            .unwrap_or_default(),
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

async fn create(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    RawQuery(query): RawQuery,
    format: ResponseFormat,
) -> Result<PlexResponse<Json>, ApiError> {
    let params: HashMap<String, String> =
        form_urlencoded::parse(query.as_deref().unwrap_or_default().as_bytes())
            .into_owned()
            .collect();
    params.get("type").ok_or(ApiError::BadRequest)?;
    let uri = params.get("uri").ok_or(ApiError::BadRequest)?;

    let path = library_path(
        uri,
        &crate::gql::machine_identifier(&state.identity.machine_identifier),
    );
    let items = match path {
        Some(path) => expand(&state, path).await?,
        None => Vec::new(),
    };
    let selected = params
        .get("key")
        .and_then(|key| key.strip_prefix("/library/metadata/"))
        .and_then(|key| key.parse::<i64>().ok())
        .and_then(|key| items.iter().position(|id| database_id(id) == key))
        .unwrap_or(0);

    let fields = [
        (
            "sourceUri",
            Value::String(path.map_or_else(|| uri.clone(), source_uri)),
        ),
        (
            "itemIds",
            Value::List(items.into_iter().map(Value::String).collect()),
        ),
        (
            "selectedPosition",
            Value::Number(u32::try_from(selected).unwrap_or(0).into()),
        ),
        (
            "shuffle",
            Value::Boolean(params.get("shuffle").is_some_and(|value| value == "1")),
        ),
    ];
    let input = fields
        .into_iter()
        .map(|(name, value)| (Name::new(name), value))
        .collect();
    let mut variables = Variables::default();
    variables.insert(Name::new("input"), Value::Object(input));
    let data: CreatedData = gql::execute(&state.schema, CREATE_MUTATION, variables).await?;

    let body = respond(
        &state,
        &data.create_play_queue.id,
        Range::default(),
        Language::of(&client),
    )
    .await?;
    Ok(PlexResponse::new(body, format))
}

async fn refuse_change(_caller: Caller) -> ApiError {
    ApiError::NotImplemented
}

/// Which entries to show, around which one.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Range {
    /// `None` is the queue's selected entry.
    center: Option<i64>,
    window: i64,
    before: bool,
    after: bool,
}

impl Default for Range {
    fn default() -> Self {
        Self {
            center: None,
            window: DEFAULT_WINDOW,
            before: true,
            after: true,
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
        };
        if !range.before && !range.after {
            return Err(ApiError::BadRequest);
        }
        Ok(range)
    }

    /// `(first position, how many)`. The centre itself is shown only when both sides are: with
    /// either refused, the reference returns just the other side.
    fn span(&self, center: i64, total: i64) -> (i64, i64) {
        if self.window <= 0 {
            return (0, 0);
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
        (start, (end - start + 1).max(0))
    }
}

const QUEUE_QUERY: &str = r"
query PlexPlayQueue($id: ID!) {
  node(id: $id) {
    ... on PlayQueue {
      sourceUri
      shuffled
      version
      totalCount
      selectedEntry { id position item { id } }
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
    source_uri: String,
    shuffled: bool,
    version: i32,
    total_count: i64,
    selected_entry: Option<GqlEntry>,
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

/// The position to centre on. `None` when the entry named is not in this queue, for which the
/// reference answers an empty window rather than an error.
async fn center_position(
    state: &PlexState,
    queue_id: &str,
    queue: &GqlQueue,
    range: &Range,
) -> Result<Option<i64>, ApiError> {
    let Some(center) = range.center else {
        return Ok(queue.selected_entry.as_ref().map(|entry| entry.position));
    };
    let variables = id_variable(GlobalId::encode(PLAY_QUEUE_ENTRY_TYPE, center));
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

async fn respond(
    state: &PlexState,
    queue_id: &str,
    range: Range,
    language: Language,
) -> Result<Json, ApiError> {
    let data: QueueData =
        gql::execute(&state.schema, QUEUE_QUERY, id_variable(queue_id.to_owned())).await?;
    let queue = data.node.ok_or(ApiError::NotFound)?;

    let center = center_position(state, queue_id, &queue, &range).await?;
    let entries = match center {
        Some(center) => {
            let (start, count) = range.span(center, queue.total_count);
            window_entries(state, queue_id, start, count).await?
        }
        None => Vec::new(),
    };
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
    let metadata: Vec<Json> = shown
        .zip(items.iter())
        .filter_map(|(entry, item)| queue_item(database_id(&entry.id), item.as_ref()?, language))
        .collect();

    let mut body = Map::new();
    body.insert("size".into(), json!(metadata.len()));
    body.insert("identifier".into(), json!(LIBRARY_IDENTIFIER));
    body.insert("mediaTagPrefix".into(), json!(MEDIA_TAG_PREFIX));
    body.insert("mediaTagVersion".into(), json!(MEDIA_TAG_VERSION));
    body.insert("playQueueID".into(), json!(database_id(queue_id)));
    if let Some(selected) = &queue.selected_entry {
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
    body.insert("playQueueShuffled".into(), json!(queue.shuffled));
    body.insert("playQueueSourceURI".into(), json!(queue.source_uri));
    if queue.total_count > 0 {
        body.insert("playQueueTotalCount".into(), json!(queue.total_count));
    }
    body.insert("playQueueVersion".into(), json!(queue.version));
    if !metadata.is_empty() {
        body.insert("Metadata".into(), Json::Array(metadata));
    }
    Ok(Json::Object(body))
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
    let id = GlobalId::encode(PLAY_QUEUE_TYPE, queue);
    let body = respond(&state, &id, range, Language::of(&client)).await?;
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
        assert_eq!(Range::default().span(4, 22), (0, 22));
        assert_eq!(range("window=4").span(4, 22), (0, 9));
        assert_eq!(range("window=5").span(4, 22), (0, 10));
        assert_eq!(range("window=4").span(10, 22), (6, 9));
    }

    #[test]
    fn a_new_queue_starting_at_its_first_entry_shows_twenty_one() {
        assert_eq!(Range::default().span(0, 261), (0, 21));
    }

    #[test]
    fn refusing_a_side_also_drops_the_centre() {
        assert_eq!(range("includeBefore=0").span(4, 22), (5, 17));
        assert_eq!(range("includeAfter=0").span(4, 22), (0, 4));
    }

    #[test]
    fn a_window_of_zero_shows_nothing() {
        assert_eq!(range("window=0").span(4, 22), (0, 0));
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
