//! `/playlists`: a viewer's playlists, bound onto the official API's.
//!
//! What a client names in a `uri` is expanded here, as a play queue expands it, into the items
//! the official API is handed; a smart playlist's library listing is translated into that
//! API's filter. Everything kept, checked and counted is the official API's. The shapes are
//! the reference's, measured on its test libraries (`docs/spec-gaps.md`).
//!
//! The generator routes belong to the reference's optimizer, which keeps optimized versions
//! as playlists of its own. Nothing here optimizes, so no such generator exists: they answer
//! as the reference answers one it does not have.

use async_graphql::{Name, Value, Variables};
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post, put};
use axum::Router;
use open_plex_api::library::LIBRARY_TYPE;
use open_plex_api::play_queue::PLAY_QUEUE_TYPE;
use open_plex_api::playlist::{PLAYLIST_ENTRY_TYPE, PLAYLIST_TYPE};
use open_plex_api::GlobalId;
use open_plex_core::page::MAX_ITEMS;
use serde::Deserialize;
use serde_json::{json, Map, Value as Json};

use crate::auth::Caller;
use crate::client::ClientIdentity;
use crate::error::ApiError;
use crate::gql;
use crate::gql_items::{database_id, with_shared_fragments, GqlItem};
use crate::items::MetadataEntry;
use crate::language::Language;
use crate::library::item_type;
use crate::listing;
use crate::play_queues::{expand, library_path, source_uri};
use crate::response::{PlexResponse, ResponseFormat};
use crate::PlexState;

pub fn router() -> Router<PlexState> {
    Router::new()
        .route("/playlists", get(list).post(create))
        // The key a library's shelf of playlists links to.
        .route("/playlists/all", get(list))
        .route("/playlists/upload", post(upload))
        .route(
            "/playlists/{playlistId}",
            get(lookup).put(edit).delete(delete),
        )
        .route("/playlists/{playlistId}/generators", get(generators))
        .route(
            "/playlists/{playlistId}/items",
            get(items).put(add).delete(clear),
        )
        .route(
            "/playlists/{playlistId}/items/{generatorId}",
            get(no_generator).put(modify_generator).delete(remove),
        )
        .route(
            "/playlists/{playlistId}/items/{generatorId}/items",
            get(no_generator),
        )
        .route(
            "/playlists/{playlistId}/items/{generatorId}/{metadataId}/{action}",
            put(no_generator),
        )
        .route(
            "/playlists/{playlistId}/items/{playlistItemId}/move",
            put(move_entry),
        )
}

/// The reference's `type` numbers: a playlist is 15, and nothing else is listed here.
const PLAYLIST_ITEM_TYPE: &str = "15";
const SMART_ICON: &str = "playlist://image.smart";
const GUID_PREFIX: &str = "com.plexapp.agents.none://";

const PLAYLIST_FIELDS: &str = r"
fragment PlaylistFields on Playlist {
  id
  guid
  title
  summary
  kind
  isSmart
  sourceUri
  lockedFields
  viewCount
  lastViewedAt
  addedAt
  updatedAt
  itemCount
  durationMs
}
";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlPlaylist {
    id: String,
    guid: String,
    title: String,
    summary: String,
    kind: String,
    is_smart: bool,
    source_uri: Option<String>,
    locked_fields: Vec<String>,
    view_count: u64,
    last_viewed_at: i64,
    added_at: i64,
    updated_at: i64,
    item_count: u64,
    duration_ms: Option<i64>,
}

/// Where a playlist is shown, which decides what it carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Form {
    /// `/playlists`: with how often and when it was played.
    Listing,
    /// `/playlists/{id}`: with a smart playlist's rule.
    Lookup,
    /// The answer to a change: neither.
    Changed,
}

impl GqlPlaylist {
    fn rating_key(&self) -> i64 {
        database_id(&self.id)
    }

    /// The attributes in the reference's order, each where the reference sends it.
    fn metadata(&self, form: Form) -> Json {
        let key = self.rating_key();
        let mut fields = Map::new();
        if form == Form::Lookup && self.is_smart {
            if let Some(content) = &self.source_uri {
                fields.insert("content".into(), json!(content));
            }
        }
        fields.insert("ratingKey".into(), json!(key.to_string()));
        fields.insert("key".into(), json!(format!("/playlists/{key}/items")));
        fields.insert("guid".into(), json!(format!("{GUID_PREFIX}{}", self.guid)));
        fields.insert("type".into(), json!("playlist"));
        fields.insert("title".into(), json!(self.title));
        fields.insert("summary".into(), json!(self.summary));
        fields.insert("smart".into(), json!(self.is_smart));
        fields.insert("playlistType".into(), json!(self.kind.to_lowercase()));
        // Drawn from the items' art, so an empty playlist has none.
        if self.item_count > 0 {
            fields.insert("composite".into(), json!(self.composite()));
        }
        if self.is_smart {
            fields.insert("icon".into(), json!(SMART_ICON));
        }
        if form == Form::Listing {
            fields.insert("viewCount".into(), json!(self.view_count));
            fields.insert("lastViewedAt".into(), json!(self.last_viewed_at));
        }
        if let Some(seconds) = self.duration_seconds().filter(|seconds| *seconds > 0) {
            fields.insert("duration".into(), json!(seconds * 1000));
        }
        fields.insert("leafCount".into(), json!(self.item_count));
        fields.insert("addedAt".into(), json!(self.added_at));
        fields.insert("updatedAt".into(), json!(self.updated_at));
        let locked: Vec<Json> = self
            .locked_fields
            .iter()
            .map(|field| json!({ "locked": true, "name": field.to_lowercase() }))
            .collect();
        if !locked.is_empty() {
            fields.insert("Field".into(), Json::Array(locked));
        }
        Json::Object(fields)
    }

    /// Versioned by the last change, so a client refetches it when the items change.
    fn composite(&self) -> String {
        format!(
            "/playlists/{}/composite/{}",
            self.rating_key(),
            self.updated_at
        )
    }

    /// The reference counts whole seconds.
    fn duration_seconds(&self) -> Option<i64> {
        self.duration_ms.map(|ms| ms / 1000)
    }
}

fn variables<const N: usize>(values: [(&str, Value); N]) -> Variables {
    let mut variables = Variables::default();
    for (name, value) in values {
        variables.insert(Name::new(name), value);
    }
    variables
}

fn input(fields: Vec<(&str, Value)>) -> Variables {
    let object = fields
        .into_iter()
        .map(|(name, value)| (Name::new(name), value))
        .collect();
    variables([("input", Value::Object(object))])
}

fn playlist_id(key: i64) -> Value {
    Value::String(GlobalId::encode(PLAYLIST_TYPE, key))
}

fn single(playlist: &GqlPlaylist, form: Form) -> Json {
    json!({ "size": 1, "Metadata": [playlist.metadata(form)] })
}

#[derive(Debug, Deserialize)]
struct PlaylistNodeData {
    node: Option<GqlPlaylist>,
}

/// The caller's playlist, or `404` for any other id: someone else's reads as none.
async fn playlist(state: &PlexState, key: i64) -> Result<GqlPlaylist, ApiError> {
    let query = format!(
        "query PlexPlaylist($id: ID!) {{ node(id: $id) {{ ...PlaylistFields }} }}\n{PLAYLIST_FIELDS}"
    );
    let data: PlaylistNodeData =
        gql::execute(&state.schema, &query, variables([("id", playlist_id(key))])).await?;
    data.node.ok_or(ApiError::NotFound)
}

/// A mutation's refusal, in the status the reference answers it with.
#[derive(Debug, Deserialize)]
struct GqlUserError {
    code: String,
}

fn refused(errors: &[GqlUserError]) -> Option<ApiError> {
    errors.first().map(|error| match error.code.as_str() {
        "PLAYLIST_NOT_FOUND" | "ENTRY_NOT_FOUND" => ApiError::NotFound,
        _ => ApiError::BadRequest,
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlPlaylistPayload {
    playlist: Option<GqlPlaylist>,
    #[serde(default)]
    added_count: u64,
    user_errors: Vec<GqlUserError>,
}

impl GqlPlaylistPayload {
    fn into_playlist(self) -> Result<GqlPlaylist, ApiError> {
        if let Some(error) = refused(&self.user_errors) {
            return Err(error);
        }
        self.playlist.ok_or(ApiError::NotFound)
    }
}

/// Runs one playlist mutation that answers `{ playlist, userErrors }`, under `field`.
async fn mutate(
    state: &PlexState,
    field: &str,
    input_type: &str,
    variables: Variables,
) -> Result<GqlPlaylistPayload, ApiError> {
    let added = if field == "addToPlaylist" {
        "addedCount"
    } else {
        ""
    };
    let query = format!(
        "mutation PlexPlaylistChange($input: {input_type}!) {{ \
           {field}(input: $input) {{ playlist {{ ...PlaylistFields }} {added} userErrors {{ code }} }} \
         }}\n{PLAYLIST_FIELDS}"
    );
    let mut data: Map<String, Json> = gql::execute(&state.schema, &query, variables).await?;
    let payload = data.remove(field).ok_or(ApiError::NotFound)?;
    serde_json::from_value(payload).map_err(|error| ApiError::Internal(Box::new(error)))
}

/// `X-Plex-Container-Start` and `-Size`, which page only together.
fn container_window(params: &[(String, String)]) -> Option<(u64, u32)> {
    let value = |name: &str| {
        params
            .iter()
            .rev()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };
    let start = value("X-Plex-Container-Start")?.parse().ok()?;
    let size: u32 = value("X-Plex-Container-Size")?.parse().ok()?;
    Some((start, size.min(MAX_ITEMS)))
}

fn param<'a>(params: &'a [(String, String)], name: &str) -> Option<&'a str> {
    params
        .iter()
        .rev()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListData {
    playlists: GqlPlaylistConnection,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlPlaylistConnection {
    total_count: u64,
    edges: Vec<GqlPlaylistEdge>,
}

#[derive(Debug, Deserialize)]
struct GqlPlaylistEdge {
    node: GqlPlaylist,
}

/// The official API's order and direction for a Plex `sort`, or `501` for one it cannot
/// honor: a duration or a count, which a smart playlist only knows by listing.
fn ordering(sort: Option<&str>) -> Result<(&'static str, &'static str), ApiError> {
    let Some(sort) = sort.filter(|sort| !sort.is_empty()) else {
        return Ok(("TITLE", "ASCENDING"));
    };
    let (field, direction) = sort.split_once(':').unwrap_or((sort, "asc"));
    let order = match field {
        "titleSort" | "title" => "TITLE",
        "addedAt" => "ADDED_AT",
        "updatedAt" => "UPDATED_AT",
        "lastViewedAt" => "LAST_VIEWED_AT",
        _ => return Err(ApiError::NotImplemented),
    };
    let direction = match direction {
        "asc" => "ASCENDING",
        "desc" => "DESCENDING",
        _ => return Err(ApiError::BadRequest),
    };
    Ok((order, direction))
}

async fn list(
    _caller: Caller,
    State(state): State<PlexState>,
    Query(params): Query<Vec<(String, String)>>,
    format: ResponseFormat,
) -> Result<PlexResponse<Json>, ApiError> {
    let kind = match param(&params, "playlistType") {
        None | Some("") => Value::Null,
        Some("video") => Value::Enum(Name::new("VIDEO")),
        Some("audio") => Value::Enum(Name::new("AUDIO")),
        Some("photo") => Value::Enum(Name::new("PHOTO")),
        Some(_) => return Err(ApiError::BadRequest),
    };
    let smart_only = match param(&params, "smart") {
        None | Some("") => Value::Null,
        Some("1") => Value::Boolean(true),
        Some("0") => Value::Boolean(false),
        Some(_) => return Err(ApiError::BadRequest),
    };
    let library = match param(&params, "sectionID") {
        None | Some("") => Value::Null,
        Some(section) => Value::String(GlobalId::encode(
            LIBRARY_TYPE,
            section.parse().map_err(|_| ApiError::BadRequest)?,
        )),
    };
    // Anything but a playlist is a type no playlist listing holds.
    let other_type = param(&params, "type").is_some_and(|kind| kind != PLAYLIST_ITEM_TYPE);
    let (order, direction) = ordering(param(&params, "sort"))?;
    let window = container_window(&params);
    let (start, size) = window.unwrap_or((0, MAX_ITEMS));

    let query = format!(
        "query PlexPlaylists($kind: PlaylistKind, $smart: Boolean, $library: ID, \
           $orderBy: PlaylistOrder!, $direction: SortDirection!, $first: Int!, $offset: Int!) {{ \
           playlists(kind: $kind, smart: $smart, library: $library, orderBy: $orderBy, \
             direction: $direction, first: $first, offset: $offset) {{ \
             totalCount edges {{ node {{ ...PlaylistFields }} }} }} }}\n{PLAYLIST_FIELDS}"
    );
    let (playlists, total) = if other_type || size == 0 {
        (Vec::new(), 0)
    } else {
        let data: ListData = gql::execute(
            &state.schema,
            &query,
            variables([
                ("kind", kind),
                ("smart", smart_only),
                ("library", library),
                ("orderBy", Value::Enum(Name::new(order))),
                ("direction", Value::Enum(Name::new(direction))),
                ("first", Value::Number(size.into())),
                (
                    "offset",
                    Value::Number(u32::try_from(start).unwrap_or(u32::MAX).into()),
                ),
            ]),
        )
        .await?;
        (data.playlists.edges, data.playlists.total_count)
    };

    let metadata: Vec<Json> = playlists
        .iter()
        .map(|edge| edge.node.metadata(Form::Listing))
        .collect();
    let mut body = Map::new();
    body.insert("size".into(), json!(metadata.len()));
    if window.is_some() {
        body.insert("totalSize".into(), json!(total));
        body.insert("offset".into(), json!(start));
    }
    if !metadata.is_empty() {
        body.insert("Metadata".into(), Json::Array(metadata));
    }
    let response = PlexResponse::new(Json::Object(body), format);
    Ok(match window {
        Some((start, _)) => response.paged(start, total),
        None => response,
    })
}

/// A smart playlist's rule, read off a library listing's path as the listing reads it.
fn rule_of(path: &str) -> Result<Value, ApiError> {
    let rest = path
        .strip_prefix("/library/sections/")
        .ok_or(ApiError::BadRequest)?;
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let section: i64 = path
        .strip_suffix("/all")
        .and_then(|id| id.parse().ok())
        .ok_or(ApiError::BadRequest)?;
    let pairs: Vec<(String, String)> = form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect();
    let request = listing::parse(&pairs)?;
    let (order_by, direction) = request.ordering();
    let fields = [
        (
            "libraryId",
            Value::String(GlobalId::encode(LIBRARY_TYPE, section)),
        ),
        (
            "filter",
            request.filters.to_graphql(item_type(request.kind)),
        ),
        ("orderBy", Value::Enum(Name::new(order_by))),
        ("direction", Value::Enum(Name::new(direction))),
    ];
    Ok(Value::Object(
        fields
            .into_iter()
            .map(|(name, value)| (Name::new(name), value))
            .collect(),
    ))
}

#[derive(Debug, Deserialize)]
struct QueueData {
    node: Option<GqlQueueEntries>,
}

#[derive(Debug, Deserialize)]
struct GqlQueueEntries {
    entries: GqlQueueConnection,
}

#[derive(Debug, Deserialize)]
struct GqlQueueConnection {
    edges: Vec<GqlQueueEdge>,
}

#[derive(Debug, Deserialize)]
struct GqlQueueEdge {
    node: GqlQueueEntry,
}

#[derive(Debug, Deserialize)]
struct GqlQueueEntry {
    item: Option<GqlIdentified>,
}

#[derive(Debug, Deserialize)]
struct GqlIdentified {
    id: String,
}

/// What a play queue plays, in order. A queue the caller does not have plays nothing, as on
/// the reference.
async fn queue_items(state: &PlexState, queue: &str) -> Result<Vec<String>, ApiError> {
    let key: i64 = queue.parse().map_err(|_| ApiError::BadRequest)?;
    let query = r"
query PlexQueueItems($id: ID!, $first: Int!) {
  node(id: $id) { ... on PlayQueue { entries(first: $first) { edges { node { item { id } } } } } }
}
";
    let data: QueueData = gql::execute(
        &state.schema,
        query,
        variables([
            ("id", Value::String(GlobalId::encode(PLAY_QUEUE_TYPE, key))),
            ("first", Value::Number(MAX_ITEMS.into())),
        ]),
    )
    .await?;
    Ok(data
        .node
        .map(|queue| {
            queue
                .entries
                .edges
                .into_iter()
                .filter_map(|edge| edge.node.item.map(|item| item.id))
                .collect()
        })
        .unwrap_or_default())
}

/// What a request names to put in a playlist.
enum Content {
    /// Items, as global ids, and where they came from.
    Items(Vec<String>, Option<String>),
    /// A library listing, as a rule, and where it came from.
    Rule(Value, String),
}

async fn content(
    state: &PlexState,
    params: &[(String, String)],
    smart: bool,
) -> Result<Content, ApiError> {
    if let Some(uri) = param(params, "uri") {
        let machine = gql::machine_identifier(&state.identity.machine_identifier);
        // Another server's items are none of this one's: the reference makes the playlist
        // and leaves it empty.
        let Some(path) = library_path(uri, &machine) else {
            // A rule must list a library of this server.
            if smart {
                return Err(ApiError::BadRequest);
            }
            return Ok(Content::Items(Vec::new(), Some(uri.to_owned())));
        };
        if smart {
            return Ok(Content::Rule(rule_of(path)?, source_uri(path)));
        }
        return Ok(Content::Items(
            expand(state, path, false).await?,
            Some(source_uri(path)),
        ));
    }
    if let Some(queue) = param(params, "playQueueID") {
        if smart {
            return Err(ApiError::BadRequest);
        }
        return Ok(Content::Items(queue_items(state, queue).await?, None));
    }
    Err(ApiError::BadRequest)
}

fn kind_of(params: &[(String, String)]) -> Result<&'static str, ApiError> {
    match param(params, "type") {
        Some("video") => Ok("VIDEO"),
        Some("audio") => Ok("AUDIO"),
        Some("photo") => Ok("PHOTO"),
        _ => Err(ApiError::BadRequest),
    }
}

fn ids(items: Vec<String>) -> Value {
    Value::List(items.into_iter().map(Value::String).collect())
}

fn optional(text: Option<String>) -> Value {
    text.map_or(Value::Null, Value::String)
}

async fn create(
    _caller: Caller,
    State(state): State<PlexState>,
    Query(params): Query<Vec<(String, String)>>,
    format: ResponseFormat,
) -> Result<PlexResponse<Json>, ApiError> {
    let kind = kind_of(&params)?;
    let title = param(&params, "title")
        .filter(|title| !title.is_empty())
        .ok_or(ApiError::BadRequest)?;
    let smart = param(&params, "smart") == Some("1");
    let mut fields = vec![
        ("title", Value::String(title.to_owned())),
        ("kind", Value::Enum(Name::new(kind))),
    ];
    match content(&state, &params, smart).await? {
        Content::Items(items, source) => {
            fields.push(("itemIds", ids(items)));
            fields.push(("sourceUri", optional(source)));
        }
        Content::Rule(rule, source) => {
            fields.push(("rule", rule));
            fields.push(("sourceUri", Value::String(source)));
        }
    }
    let created = mutate(
        &state,
        "createPlaylist",
        "CreatePlaylistInput",
        input(fields),
    )
    .await?
    .into_playlist()?;
    Ok(PlexResponse::new(single(&created, Form::Changed), format))
}

/// Importing `.m3u` files names a path on the server, and no endpoint here accepts a path
/// from a client: a path the server opens on a client's word is how a media server ends up
/// reading what it was never given (`security.md`).
async fn upload(_caller: Caller, Query(params): Query<Vec<(String, String)>>) -> ApiError {
    if param(&params, "path").is_some() {
        ApiError::Forbidden
    } else {
        ApiError::BadRequest
    }
}

async fn lookup(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(key): Path<String>,
    format: ResponseFormat,
) -> Result<PlexResponse<Json>, ApiError> {
    let found = playlist(&state, parse_key(&key)?).await?;
    Ok(PlexResponse::new(single(&found, Form::Lookup), format))
}

/// A path id that is not a number names nothing, as on the reference.
fn parse_key(key: &str) -> Result<i64, ApiError> {
    key.parse().map_err(|_| ApiError::NotFound)
}

/// The fields of `PUT /playlists/{id}`: bare as the spec writes them or as the item editor
/// sends them, `.value` and `.locked`.
#[derive(Debug, Default, PartialEq)]
struct PlaylistChanges {
    title: Option<String>,
    summary: Option<String>,
    lock: Vec<&'static str>,
    unlock: Vec<&'static str>,
}

impl PlaylistChanges {
    fn parse(params: &[(String, String)]) -> Result<Self, ApiError> {
        let mut changes = Self::default();
        for (key, value) in params {
            let (field, part) = key.split_once('.').unwrap_or((key.as_str(), ""));
            let target = match field {
                "title" => Some(("TITLE", &mut changes.title)),
                "summary" => Some(("SUMMARY", &mut changes.summary)),
                _ => None,
            };
            match (target, part) {
                (Some((_, slot)), "" | "value") => *slot = Some(value.clone()),
                (Some((name, _)), "locked") => match value.as_str() {
                    "1" => changes.lock.push(name),
                    "0" => changes.unlock.push(name),
                    _ => return Err(ApiError::BadRequest),
                },
                // Artwork and the rest of an item's fields: a playlist here keeps none.
                (None, "value" | "locked") => return Err(ApiError::NotImplemented),
                _ => {}
            }
        }
        Ok(changes)
    }

    fn input(self, key: i64) -> Variables {
        let names = |fields: Vec<&'static str>| {
            Value::List(
                fields
                    .into_iter()
                    .map(|field| Value::Enum(Name::new(field)))
                    .collect(),
            )
        };
        let mut fields = vec![("id", playlist_id(key))];
        if let Some(title) = self.title {
            fields.push(("title", Value::String(title)));
        }
        if let Some(summary) = self.summary {
            fields.push(("summary", Value::String(summary)));
        }
        fields.push(("lock", names(self.lock)));
        fields.push(("unlock", names(self.unlock)));
        input(fields)
    }
}

async fn edit(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(key): Path<String>,
    Query(params): Query<Vec<(String, String)>>,
) -> Result<impl IntoResponse, ApiError> {
    let key = parse_key(&key)?;
    let changes = PlaylistChanges::parse(&params)?;
    mutate(
        &state,
        "updatePlaylist",
        "UpdatePlaylistInput",
        changes.input(key),
    )
    .await?
    .into_playlist()?;
    // As the reference answers an edit: `200`, no body, labelled HTML.
    Ok((StatusCode::OK, [(header::CONTENT_TYPE, "text/html")]))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteData {
    delete_playlist: GqlDeletePayload,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlDeletePayload {
    user_errors: Vec<GqlUserError>,
}

async fn delete(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(key): Path<String>,
) -> Result<StatusCode, ApiError> {
    let key = parse_key(&key)?;
    let query = r"
mutation PlexDeletePlaylist($input: DeletePlaylistInput!) {
  deletePlaylist(input: $input) { userErrors { code } }
}
";
    let data: DeleteData =
        gql::execute(&state.schema, query, input(vec![("id", playlist_id(key))])).await?;
    if let Some(error) = refused(&data.delete_playlist.user_errors) {
        return Err(error);
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn add(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(key): Path<String>,
    Query(params): Query<Vec<(String, String)>>,
    format: ResponseFormat,
) -> Result<PlexResponse<Json>, ApiError> {
    let key = parse_key(&key)?;
    let current = playlist(&state, key).await?;
    let (payload, requested) = match content(&state, &params, current.is_smart).await? {
        Content::Items(items, _) => {
            let requested = items.len();
            let fields = vec![("playlistId", playlist_id(key)), ("itemIds", ids(items))];
            let payload =
                mutate(&state, "addToPlaylist", "AddToPlaylistInput", input(fields)).await?;
            (payload, Some(requested))
        }
        Content::Rule(rule, source) => {
            let fields = vec![
                ("playlistId", playlist_id(key)),
                ("rule", rule),
                ("sourceUri", Value::String(source)),
            ];
            let payload = mutate(
                &state,
                "replacePlaylistRule",
                "ReplacePlaylistRuleInput",
                input(fields),
            )
            .await?;
            (payload, None)
        }
    };
    let added = payload.added_count;
    let changed = payload.into_playlist()?;
    let mut body = Map::new();
    body.insert("size".into(), json!(1));
    // Counted for items, not for a rule: the reference reports neither on a smart playlist.
    if let Some(requested) = requested {
        body.insert("leafCountAdded".into(), json!(added));
        body.insert("leafCountRequested".into(), json!(requested));
    }
    body.insert("Metadata".into(), json!([changed.metadata(Form::Changed)]));
    Ok(PlexResponse::new(Json::Object(body), format))
}

async fn clear(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(key): Path<String>,
    format: ResponseFormat,
) -> Result<PlexResponse<Json>, ApiError> {
    let key = parse_key(&key)?;
    let cleared = mutate(
        &state,
        "clearPlaylist",
        "ClearPlaylistInput",
        input(vec![("id", playlist_id(key))]),
    )
    .await?
    .into_playlist()?;
    Ok(PlexResponse::new(single(&cleared, Form::Changed), format))
}

#[derive(Debug, Deserialize)]
struct EntryNodeData {
    node: Option<GqlEntryPlaylist>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlEntryPlaylist {
    playlist_id: Option<String>,
}

/// An entry of this playlist, by the `playlistItemID` a client was given, or `404`: one of
/// another playlist is not this one's.
async fn entry_of(
    state: &PlexState,
    playlist: &GqlPlaylist,
    entry: &str,
) -> Result<Value, ApiError> {
    // A smart playlist's generator is its rule, which no entry route edits.
    if playlist.is_smart {
        return Err(ApiError::BadRequest);
    }
    let key: i64 = entry.parse().map_err(|_| ApiError::NotFound)?;
    let id = Value::String(GlobalId::encode(PLAYLIST_ENTRY_TYPE, key));
    let query = r"
query PlexPlaylistEntry($id: ID!) { node(id: $id) { ... on PlaylistEntry { playlistId } } }
";
    let data: EntryNodeData =
        gql::execute(&state.schema, query, variables([("id", id.clone())])).await?;
    match data.node.and_then(|node| node.playlist_id) {
        Some(owner) if owner == playlist.id => Ok(id),
        _ => Err(ApiError::NotFound),
    }
}

async fn remove(
    _caller: Caller,
    State(state): State<PlexState>,
    Path((key, entry)): Path<(String, String)>,
    format: ResponseFormat,
) -> Result<PlexResponse<Json>, ApiError> {
    let found = playlist(&state, parse_key(&key)?).await?;
    let entry = entry_of(&state, &found, &entry).await?;
    let changed = mutate(
        &state,
        "removeFromPlaylist",
        "RemoveFromPlaylistInput",
        input(vec![("entryId", entry)]),
    )
    .await?
    .into_playlist()?;
    Ok(PlexResponse::new(single(&changed, Form::Changed), format))
}

async fn move_entry(
    _caller: Caller,
    State(state): State<PlexState>,
    Path((key, entry)): Path<(String, String)>,
    Query(params): Query<Vec<(String, String)>>,
    format: ResponseFormat,
) -> Result<PlexResponse<Json>, ApiError> {
    let found = playlist(&state, parse_key(&key)?).await?;
    let entry = entry_of(&state, &found, &entry).await?;
    let mut fields = vec![("entryId", entry)];
    if let Some(after) = param(&params, "after").filter(|after| !after.is_empty()) {
        let after: i64 = after.parse().map_err(|_| ApiError::BadRequest)?;
        fields.push((
            "afterEntryId",
            Value::String(GlobalId::encode(PLAYLIST_ENTRY_TYPE, after)),
        ));
    }
    let payload = mutate(
        &state,
        "movePlaylistEntry",
        "MovePlaylistEntryInput",
        input(fields),
    )
    .await?;
    // The entry moved is this playlist's, checked above: an entry not found now is the
    // `after` named, which is the client's mistake rather than a missing target.
    if payload
        .user_errors
        .first()
        .is_some_and(|error| error.code == "ENTRY_NOT_FOUND")
    {
        return Err(ApiError::BadRequest);
    }
    let changed = payload.into_playlist()?;
    Ok(PlexResponse::new(single(&changed, Form::Changed), format))
}

/// The optimizer's generators by id: none exists here.
async fn no_generator(_caller: Caller) -> ApiError {
    ApiError::NotFound
}

/// Measured on a playlist of chosen items, where the reference refuses the change as `400`.
async fn modify_generator(
    _caller: Caller,
    State(state): State<PlexState>,
    Path((key, _generator)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    playlist(&state, parse_key(&key)?).await?;
    Err(ApiError::BadRequest)
}

const ENTRIES_QUERY: &str = r"
query PlexPlaylistEntries($id: ID!, $first: Int!, $offset: Int!) {
  node(id: $id) {
    ... on Playlist {
      entries(first: $first, offset: $offset) {
        edges { node { id item { ...ListedItem } } }
      }
    }
  }
}
";

const SMART_ITEMS_QUERY: &str = r"
query PlexPlaylistItems($id: ID!, $first: Int!, $offset: Int!) {
  node(id: $id) {
    ... on Playlist {
      items(first: $first, offset: $offset) { edges { node { ...ListedItem } } }
    }
  }
}
";

#[derive(Debug, Deserialize)]
struct EntriesData {
    node: Option<GqlEntries>,
}

#[derive(Debug, Deserialize)]
struct GqlEntries {
    entries: GqlEdges<GqlEntry>,
}

#[derive(Debug, Deserialize)]
struct SmartItemsData {
    node: Option<GqlSmartItems>,
}

#[derive(Debug, Deserialize)]
struct GqlSmartItems {
    items: GqlEdges<GqlItem>,
}

#[derive(Debug, Deserialize)]
struct GqlEdges<T> {
    edges: Vec<GqlEdge<T>>,
}

#[derive(Debug, Deserialize)]
struct GqlEdge<T> {
    node: T,
}

#[derive(Debug, Deserialize)]
struct GqlEntry {
    id: String,
    item: Option<GqlItem>,
}

/// An item as a playlist shows it: a listing's form, with its library, and with the entry
/// it is held under.
fn playlist_item(item: &GqlItem, entry: Option<i64>, language: Language) -> Option<Json> {
    let metadata = MetadataEntry::from_graphql(item, true, language)?;
    let Json::Object(fields) = serde_json::to_value(metadata).ok()? else {
        return None;
    };
    Some(Json::Object(match entry {
        Some(entry) => with_entry(fields, entry),
        None => fields,
    }))
}

/// Puts `playlistItemID` where the reference does: just before the duration, or before what
/// follows it when an item has none.
fn with_entry(fields: Map<String, Json>, entry: i64) -> Map<String, Json> {
    let mut ordered = Map::with_capacity(fields.len() + 1);
    for (key, value) in fields {
        let follows = matches!(
            key.as_str(),
            "duration" | "originallyAvailableAt" | "addedAt"
        );
        if follows && !ordered.contains_key("playlistItemID") {
            ordered.insert("playlistItemID".to_owned(), json!(entry));
        }
        ordered.insert(key, value);
    }
    ordered
}

/// The reference's numbers for the two kinds a playlist holds.
fn plex_type_number(item: &Json) -> Option<&'static str> {
    match item.get("type").and_then(Json::as_str) {
        Some("movie") => Some("1"),
        Some("episode") => Some("4"),
        _ => None,
    }
}

async fn items(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    Path(key): Path<String>,
    Query(params): Query<Vec<(String, String)>>,
    format: ResponseFormat,
) -> Result<PlexResponse<Json>, ApiError> {
    let found = playlist(&state, parse_key(&key)?).await?;
    let language = Language::of(&client);
    let window = container_window(&params);
    let (start, size) = window.unwrap_or((0, MAX_ITEMS));
    let page = variables([
        ("id", Value::String(found.id.clone())),
        ("first", Value::Number(size.into())),
        (
            "offset",
            Value::Number(u32::try_from(start).unwrap_or(u32::MAX).into()),
        ),
    ]);
    let mut metadata: Vec<Json> = if size == 0 {
        Vec::new()
    } else if found.is_smart {
        let data: SmartItemsData = gql::execute(
            &state.schema,
            &with_shared_fragments(SMART_ITEMS_QUERY),
            page,
        )
        .await?;
        data.node
            .map(|node| node.items.edges)
            .unwrap_or_default()
            .iter()
            .filter_map(|edge| playlist_item(&edge.node, None, language))
            .collect()
    } else {
        let data: EntriesData =
            gql::execute(&state.schema, &with_shared_fragments(ENTRIES_QUERY), page).await?;
        data.node
            .map(|node| node.entries.edges)
            .unwrap_or_default()
            .iter()
            .filter_map(|edge| {
                let item = edge.node.item.as_ref()?;
                playlist_item(item, Some(database_id(&edge.node.id)), language)
            })
            .collect()
    };
    // Values past the first serve a background list this server does not keep.
    if let Some(first) = param(&params, "type")
        .and_then(|kinds| kinds.split(',').next())
        .filter(|kind| !kind.is_empty())
    {
        metadata.retain(|item| plex_type_number(item) == Some(first));
    }

    let mut body = Map::new();
    body.insert("size".into(), json!(metadata.len()));
    if window.is_some() {
        body.insert("totalSize".into(), json!(found.item_count));
        body.insert("offset".into(), json!(start));
    }
    // Unlike the playlist's own attributes, the container carries its image even empty.
    body.insert("composite".into(), json!(found.composite()));
    if let Some(seconds) = found.duration_seconds().filter(|seconds| *seconds > 0) {
        body.insert("duration".into(), json!(seconds));
    }
    body.insert("leafCount".into(), json!(found.item_count));
    body.insert("playlistType".into(), json!(found.kind.to_lowercase()));
    body.insert("ratingKey".into(), json!(found.rating_key().to_string()));
    body.insert("smart".into(), json!(found.is_smart));
    body.insert("title".into(), json!(found.title));
    if !metadata.is_empty() {
        body.insert("Metadata".into(), Json::Array(metadata));
    }
    let response = PlexResponse::new(Json::Object(body), format);
    Ok(match window {
        Some((start, _)) => response.paged(start, found.item_count),
        None => response,
    })
}

#[derive(Debug, Deserialize)]
struct GeneratorEntriesData {
    node: Option<GqlGeneratorEntries>,
}

#[derive(Debug, Deserialize)]
struct GqlGeneratorEntries {
    entries: GqlEdges<GqlGeneratorEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlGeneratorEntry {
    id: String,
    position: i64,
    added_at: i64,
    item: Option<GqlIdentified>,
}

/// What the reference's play queue generators are: one per entry, or the rule of a smart
/// playlist.
async fn generators(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(key): Path<String>,
    format: ResponseFormat,
) -> Result<PlexResponse<Json>, ApiError> {
    let found = playlist(&state, parse_key(&key)?).await?;
    let rating_key = found.rating_key();
    let generators: Vec<Json> = if found.is_smart {
        vec![json!({
            "id": rating_key,
            "playlistID": rating_key,
            "uri": found.source_uri,
            "createdAt": found.added_at,
            "updatedAt": found.updated_at,
            "type": -1,
        })]
    } else {
        let query = r"
query PlexPlaylistGenerators($id: ID!, $first: Int!) {
  node(id: $id) {
    ... on Playlist {
      entries(first: $first) { edges { node { id position addedAt item { id } } } }
    }
  }
}
";
        let data: GeneratorEntriesData = gql::execute(
            &state.schema,
            query,
            variables([
                ("id", Value::String(found.id.clone())),
                ("first", Value::Number(MAX_ITEMS.into())),
            ]),
        )
        .await?;
        data.node
            .map(|node| node.entries.edges)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|edge| {
                let entry = edge.node;
                let item = entry.item?;
                #[allow(
                    clippy::cast_precision_loss,
                    reason = "the reference publishes the order as a real number; positions stay far below 2^52"
                )]
                let order = entry.position as f64;
                Some(json!({
                    "id": database_id(&entry.id),
                    "playlistID": rating_key,
                    "metadataItemID": database_id(&item.id),
                    "order": order,
                    "createdAt": entry.added_at,
                    "updatedAt": entry.added_at,
                    "type": -1,
                }))
            })
            .collect()
    };
    let mut body = Map::new();
    body.insert("size".into(), json!(generators.len()));
    if !generators.is_empty() {
        body.insert("PlayQueueGenerator".into(), Json::Array(generators));
    }
    Ok(PlexResponse::new(Json::Object(body), format))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn params(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn reads_an_edit_in_either_form() {
        let changes =
            PlaylistChanges::parse(&params(&[("title", "OP Renamed"), ("summary", "Hello")]))
                .unwrap();
        assert_eq!(changes.title.as_deref(), Some("OP Renamed"));
        assert_eq!(changes.summary.as_deref(), Some("Hello"));
        let changes = PlaylistChanges::parse(&params(&[
            ("title.value", "OP Renamed2"),
            ("title.locked", "1"),
            ("X-Plex-Token", "ignored"),
        ]))
        .unwrap();
        assert_eq!(changes.title.as_deref(), Some("OP Renamed2"));
        assert_eq!(changes.lock, ["TITLE"]);
    }

    #[test]
    fn refuses_what_a_playlist_does_not_keep() {
        assert!(matches!(
            PlaylistChanges::parse(&params(&[("thumb.value", "x")])),
            Err(ApiError::NotImplemented)
        ));
        assert!(matches!(
            PlaylistChanges::parse(&params(&[("title.locked", "2")])),
            Err(ApiError::BadRequest)
        ));
    }

    #[test]
    fn orders_as_the_official_api_can_and_refuses_the_rest() {
        assert_eq!(ordering(None).unwrap(), ("TITLE", "ASCENDING"));
        assert_eq!(
            ordering(Some("lastViewedAt:desc")).unwrap(),
            ("LAST_VIEWED_AT", "DESCENDING")
        );
        assert_eq!(
            ordering(Some("titleSort:desc")).unwrap(),
            ("TITLE", "DESCENDING")
        );
        assert!(matches!(
            ordering(Some("duration:desc")),
            Err(ApiError::NotImplemented)
        ));
    }

    fn keys_after_entry(fields: &[&str]) -> Vec<String> {
        let item: Map<String, Json> = fields
            .iter()
            .map(|field| ((*field).to_owned(), json!(1)))
            .collect();
        with_entry(item, 7).keys().cloned().collect()
    }

    #[test]
    fn places_the_entry_before_the_duration_as_measured() {
        assert_eq!(
            keys_after_entry(&["ratingKey", "art", "duration", "addedAt"]),
            ["ratingKey", "art", "playlistItemID", "duration", "addedAt"]
        );
        assert_eq!(
            keys_after_entry(&["ratingKey", "thumb", "addedAt", "updatedAt"]),
            [
                "ratingKey",
                "thumb",
                "playlistItemID",
                "addedAt",
                "updatedAt"
            ]
        );
    }

    #[test]
    fn a_rule_is_read_off_a_library_listing() {
        let rule = rule_of("/library/sections/28/all?type=4&sort=titleSort:desc").unwrap();
        let Value::Object(fields) = rule else {
            panic!("a rule is an object");
        };
        assert_eq!(
            fields.get("libraryId"),
            Some(&Value::String(GlobalId::encode(LIBRARY_TYPE, 28)))
        );
        assert_eq!(
            fields.get("direction"),
            Some(&Value::Enum(Name::new("DESCENDING")))
        );
        assert!(matches!(
            rule_of("/library/metadata/577986"),
            Err(ApiError::BadRequest)
        ));
    }
}
