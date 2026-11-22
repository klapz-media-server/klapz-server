//! Playlists: the caller's own, over the official API's playlists.
//!
//! A playlist is a folder item to Jellyfin's clients, listed in a view of its own, and opened
//! by `/Playlists/{id}/Items`, each entry carrying the `PlaylistItemId` that moves and removes
//! it. Every change is one of the official API's mutations; what it refuses comes back as the
//! status the reference gives.

use axum::body::Bytes;
use axum::extract::{Path, RawQuery, State};
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::dto::{item_dto, Fields, Item, ItemsResult, Shaping, ITEM_FIELDS};
use crate::error::JellyfinError;
use crate::gql;
use crate::id;
use crate::query::Query;
use crate::response::{json, no_content};
use crate::time;
use crate::JellyfinState;

pub fn router() -> Router<JellyfinState> {
    Router::new()
        .route("/playlists", post(create))
        .route("/playlists/{id}", get(playlist).post(update))
        .route(
            "/playlists/{id}/items",
            get(entries).post(add).delete(remove),
        )
        .route(
            "/playlists/{id}/items/{entry}/move/{index}",
            post(move_entry),
        )
        .route("/playlists/{id}/users", get(users))
        .route(
            "/playlists/{id}/users/{user}",
            get(user).post(no_sharing).delete(no_sharing),
        )
}

/// What the official API is asked of a playlist, wherever it is shown.
pub const PLAYLIST_FIELDS: &str = r"
fragment JellyfinPlaylist on Playlist {
    id title kind itemCount durationMs addedAt
    items(first: 500) { nodes { watchState { isWatched } } }
}";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Playlist {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub item_count: i64,
    pub duration_ms: Option<i64>,
    pub added_at: String,
    pub items: Nodes<Watched>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Nodes<T> {
    pub nodes: Vec<T>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Watched {
    pub watch_state: Option<WatchedState>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchedState {
    pub is_watched: bool,
}

/// The view a user's playlists hang from, as the reference lists it among the libraries once
/// the user has one.
pub fn view_id() -> String {
    id::of(klapz_api::server::SERVER_TYPE, 1).unwrap_or_default()
}

pub fn view_dto(server_id: &str, count: usize) -> Value {
    let view = view_id();
    json!({
        "Name": "Playlists",
        "ServerId": server_id,
        "Id": view,
        "IsFolder": true,
        "Type": "UserView",
        "UserData": {
            "PlaybackPositionTicks": 0,
            "PlayCount": 0,
            "IsFavorite": false,
            "Played": false,
            "Key": id::dashed(&view),
            "ItemId": view,
        },
        "ChildCount": count,
        "CollectionType": "playlists",
        "LocationType": "FileSystem",
        "MediaType": "Unknown",
    })
}

fn media_type(kind: &str) -> &'static str {
    match kind {
        "AUDIO" => "Audio",
        "PHOTO" => "Photo",
        _ => "Video",
    }
}

/// A playlist as the reference shows it: a folder of its entries, counting those left to
/// watch.
pub fn playlist_dto(playlist: &Playlist, server_id: &str, fields: &Fields) -> Value {
    let jellyfin_id = id::of_global(&playlist.id).unwrap_or_default();
    let unplayed = playlist
        .items
        .nodes
        .iter()
        .filter(|item| {
            !item
                .watch_state
                .as_ref()
                .is_some_and(|state| state.is_watched)
        })
        .count();
    let count = playlist.item_count;
    #[allow(
        clippy::cast_precision_loss,
        reason = "a playlist's entries are far below 2^52"
    )]
    let percentage = if count > 0 {
        (count - i64::try_from(unplayed).unwrap_or(count)) as f64 * 100.0 / count as f64
    } else {
        0.0
    };
    let mut dto = Map::new();
    let mut put = |key: &str, value: Value| {
        dto.insert(key.to_owned(), value);
    };
    put("Name", json!(playlist.title));
    put("ServerId", json!(server_id));
    put("Id", json!(jellyfin_id));
    if fields.has("DateCreated") {
        put("DateCreated", json!(time::date(&playlist.added_at)));
    }
    if fields.has("CanDelete") {
        put("CanDelete", json!(true));
    }
    if fields.has("SortName") {
        put("SortName", json!(playlist.title.to_lowercase()));
    }
    put("ChannelId", Value::Null);
    if let Some(duration) = playlist.duration_ms {
        put("RunTimeTicks", json!(time::ticks(duration)));
    }
    put("IsFolder", json!(true));
    put("Type", json!("Playlist"));
    put(
        "UserData",
        json!({
            "PlayedPercentage": percentage,
            "UnplayedItemCount": unplayed,
            "PlaybackPositionTicks": 0,
            "PlayCount": 0,
            "IsFavorite": false,
            "Played": unplayed == 0 && count > 0,
            "Key": id::dashed(&jellyfin_id),
            "ItemId": jellyfin_id,
        }),
    );
    if fields.has("RecursiveItemCount") {
        put("RecursiveItemCount", json!(count));
    }
    put("ChildCount", json!(count));
    put("ImageTags", json!({}));
    put("BackdropImageTags", json!([]));
    put("ImageBlurHashes", json!({}));
    put("LocationType", json!("FileSystem"));
    put("MediaType", json!(media_type(&playlist.kind)));
    Value::Object(dto)
}

#[derive(Deserialize)]
struct PlaylistsData {
    playlists: Nodes<Playlist>,
}

/// The caller's playlists, newest last as the reference lists them by name; with `matching`,
/// those whose title a search term finds.
pub async fn playlists(
    state: &JellyfinState,
    matching: Option<&str>,
) -> Result<Vec<Playlist>, JellyfinError> {
    let data: PlaylistsData = gql::query(
        &state.schema,
        &format!(
            "query($matching: String) {{
                playlists(matching: $matching, orderBy: TITLE, first: 500) {{
                    nodes {{ ...JellyfinPlaylist }}
                }}
            }}
            {PLAYLIST_FIELDS}"
        ),
        json!({ "matching": matching }),
    )
    .await?;
    Ok(data.playlists.nodes)
}

#[derive(Deserialize)]
struct NodeData<T> {
    node: Option<T>,
}

/// One of the caller's playlists; another's is not found.
pub async fn playlist_of(state: &JellyfinState, global: &str) -> Result<Playlist, JellyfinError> {
    let data: NodeData<Playlist> = gql::query(
        &state.schema,
        &format!("query($id: ID!) {{ node(id: $id) {{ ...JellyfinPlaylist }} }} {PLAYLIST_FIELDS}"),
        json!({ "id": global }),
    )
    .await?;
    data.node.ok_or(JellyfinError::NotFound)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    id: String,
    item: Option<Item>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EntriesPage {
    item_count: i64,
    entries: Nodes<Entry>,
}

async fn entries_of(
    state: &JellyfinState,
    global: &str,
    offset: u32,
    first: u32,
    fields: &Fields,
) -> Result<EntriesPage, JellyfinError> {
    let data: NodeData<EntriesPage> = gql::query(
        &state.schema,
        &format!(
            r"query($id: ID!, $offset: Int, $first: Int, $people: Boolean!, $chapters: Boolean!) {{
                node(id: $id) {{ ... on Playlist {{
                    itemCount
                    entries(offset: $offset, first: $first) {{ nodes {{ id item {{ ...JellyfinItem }} }} }}
                }} }}
            }}
            {ITEM_FIELDS}"
        ),
        json!({
            "id": global,
            "offset": offset,
            "first": first,
            "people": fields.wants_people(),
            "chapters": fields.has("Chapters"),
        }),
    )
    .await?;
    data.node.ok_or(JellyfinError::NotFound)
}

/// The global id of a playlist named in a path.
fn playlist_global(jellyfin: &str) -> Result<String, JellyfinError> {
    Ok(id::global_of_type(
        jellyfin,
        klapz_api::playlist::PLAYLIST_TYPE,
    )?)
}

/// Item ids a request lists, as the official API's.
fn item_globals(ids: &[&str]) -> Result<Vec<String>, JellyfinError> {
    ids.iter()
        .map(|jellyfin| {
            id::global_of_type(jellyfin, klapz_api::item::ITEM_TYPE).map_err(Into::into)
        })
        .collect()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserError {
    code: String,
}

/// A refusal of the official API, as the reference answers the same.
fn refused(errors: &[UserError]) -> Result<(), JellyfinError> {
    match errors.first().map(|error| error.code.as_str()) {
        None => Ok(()),
        Some("PLAYLIST_NOT_FOUND" | "ENTRY_NOT_FOUND" | "ITEM_NOT_FOUND") => {
            Err(JellyfinError::NotFound)
        }
        Some(_) => Err(JellyfinError::BadRequest("the playlist refuses the change")),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Refusals {
    user_errors: Vec<UserError>,
}

async fn mutate(
    state: &JellyfinState,
    mutation: &str,
    input: Value,
) -> Result<Value, JellyfinError> {
    let data: Map<String, Value> = gql::query(
        &state.schema,
        &format!(
            "mutation($input: {}Input!) {{ {mutation}(input: $input) {{ userErrors {{ code }} {} }} }}",
            input_type(mutation),
            if mutation == "createPlaylist" { "playlist { id }" } else { "" },
        ),
        json!({ "input": input }),
    )
    .await?;
    let payload = data.get(mutation).cloned().unwrap_or(Value::Null);
    let refusals: Refusals = serde_json::from_value(payload.clone())
        .map_err(|error| JellyfinError::Internal(Box::new(error)))?;
    refused(&refusals.user_errors)?;
    Ok(payload)
}

/// `createPlaylist` takes a `CreatePlaylistInput`.
fn input_type(mutation: &str) -> String {
    let mut letters = mutation.chars();
    letters
        .next()
        .map(|first| first.to_uppercase().chain(letters).collect())
        .unwrap_or_default()
}

/// What `/Playlists` creates from: the query string, or a JSON body of the same names.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
struct PlaylistRequest {
    name: Option<String>,
    ids: Vec<String>,
    media_type: Option<String>,
    users: Vec<Value>,
    is_public: Option<bool>,
}

impl PlaylistRequest {
    fn read(body: &[u8], query: &Query) -> Result<Self, JellyfinError> {
        let mut request: Self = if body.is_empty() {
            Self::default()
        } else {
            serde_json::from_slice(body)
                .map_err(|_| JellyfinError::BadRequest("a playlist request is not JSON"))?
        };
        request.name = request
            .name
            .or_else(|| query.get("name").map(str::to_owned));
        if request.ids.is_empty() {
            request.ids = query.list("ids").into_iter().map(str::to_owned).collect();
        }
        request.media_type = request
            .media_type
            .or_else(|| query.get("mediaType").map(str::to_owned));
        // Sharing a playlist with other users: playlists here are their owner's alone.
        if !request.users.is_empty() || request.is_public == Some(true) {
            return Err(JellyfinError::NotImplemented(
                "a playlist shared with others",
            ));
        }
        Ok(request)
    }
}

fn kind_of(media_type: Option<&str>) -> Result<&'static str, JellyfinError> {
    match media_type.map(str::to_ascii_lowercase).as_deref() {
        None | Some("video") => Ok("VIDEO"),
        Some("audio") => Ok("AUDIO"),
        Some("photo") => Ok("PHOTO"),
        Some(_) => Err(JellyfinError::BadRequest(
            "a playlist's media type is unknown",
        )),
    }
}

async fn create(
    State(state): State<JellyfinState>,
    RawQuery(raw): RawQuery,
    body: Bytes,
) -> Result<Response, JellyfinError> {
    let query = Query::parse(raw.as_deref());
    gql::viewer(&state.schema)
        .await?
        .require_self(query.get("userId"))?;
    let request = PlaylistRequest::read(&body, &query)?;
    let ids: Vec<&str> = request.ids.iter().map(String::as_str).collect();
    let payload = mutate(
        &state,
        "createPlaylist",
        json!({
            "title": request.name.unwrap_or_default(),
            "kind": kind_of(request.media_type.as_deref())?,
            "itemIds": item_globals(&ids)?,
        }),
    )
    .await?;
    let created = payload["playlist"]["id"]
        .as_str()
        .and_then(id::of_global)
        .ok_or(JellyfinError::NotFound)?;
    Ok(json(&json!({ "Id": created })))
}

/// `/Playlists/{id}`: its name, its items replaced when the request lists them.
async fn update(
    State(state): State<JellyfinState>,
    Path(id): Path<String>,
    RawQuery(raw): RawQuery,
    body: Bytes,
) -> Result<Response, JellyfinError> {
    let query = Query::parse(raw.as_deref());
    let global = playlist_global(&id)?;
    let request = PlaylistRequest::read(&body, &query)?;
    if let Some(name) = request.name {
        mutate(
            &state,
            "updatePlaylist",
            json!({ "id": global, "title": name }),
        )
        .await?;
    }
    if !request.ids.is_empty() {
        let ids: Vec<&str> = request.ids.iter().map(String::as_str).collect();
        let items = item_globals(&ids)?;
        mutate(&state, "clearPlaylist", json!({ "id": global })).await?;
        mutate(
            &state,
            "addToPlaylist",
            json!({ "playlistId": global, "itemIds": items }),
        )
        .await?;
    }
    Ok(no_content())
}

async fn all_entries(state: &JellyfinState, global: &str) -> Result<Vec<Entry>, JellyfinError> {
    Ok(entries_of(
        state,
        global,
        0,
        crate::listing::MAX_PAGE,
        &Fields::named([]),
    )
    .await?
    .entries
    .nodes)
}

/// `/Playlists/{id}`: who may open it and what it holds.
async fn playlist(
    State(state): State<JellyfinState>,
    Path(id): Path<String>,
) -> Result<Response, JellyfinError> {
    let global = playlist_global(&id)?;
    let items: Vec<String> = all_entries(&state, &global)
        .await?
        .iter()
        .filter_map(|entry| id::of_global(&entry.item.as_ref()?.id))
        .collect();
    // A playlist here is its owner's alone: no open access, nobody it is shared with.
    Ok(json(&json!({
        "OpenAccess": false,
        "Shares": [],
        "ItemIds": items,
    })))
}

/// `/Playlists/{id}/Items`: its entries, each with the id that moves and removes it.
async fn entries(
    State(state): State<JellyfinState>,
    Path(id): Path<String>,
    RawQuery(raw): RawQuery,
) -> Result<Response, JellyfinError> {
    let query = Query::parse(raw.as_deref());
    gql::viewer(&state.schema)
        .await?
        .require_self(query.get("userId"))?;
    entries_page(&state, &playlist_global(&id)?, &query).await
}

/// A page of a playlist's entries, as `/Playlists/{id}/Items` and `/Items?ParentId=` list it.
pub async fn entries_page(
    state: &JellyfinState,
    global: &str,
    query: &Query,
) -> Result<Response, JellyfinError> {
    let fields = Fields::named(query.list("fields"));
    let start = query.count("startIndex")?.unwrap_or(0);
    let first = query
        .count("limit")?
        .unwrap_or(crate::listing::MAX_PAGE)
        .min(crate::listing::MAX_PAGE);
    let page = entries_of(state, global, start, first, &fields).await?;
    let shaping = Shaping {
        server_id: &state.server.id,
        fields: &fields,
    };
    let items = page
        .entries
        .nodes
        .iter()
        .filter_map(|entry| {
            let mut dto = item_dto(entry.item.as_ref()?, &shaping);
            dto["PlaylistItemId"] = json!(id::of_global(&entry.id).unwrap_or_default());
            Some(dto)
        })
        .collect();
    Ok(json(&ItemsResult {
        items,
        total_record_count: page.item_count,
        start_index: start,
    }))
}

/// `/Playlists/{id}/Items?ids=`: appended, in order. A position before the end is not built.
async fn add(
    State(state): State<JellyfinState>,
    Path(id): Path<String>,
    RawQuery(raw): RawQuery,
) -> Result<Response, JellyfinError> {
    let query = Query::parse(raw.as_deref());
    gql::viewer(&state.schema)
        .await?
        .require_self(query.get("userId"))?;
    let global = playlist_global(&id)?;
    if let Some(position) = query.count("position")? {
        let count = playlist_of(&state, &global).await?.item_count;
        if i64::from(position) < count {
            return Err(JellyfinError::NotImplemented(
                "items added before a playlist's end",
            ));
        }
    }
    mutate(
        &state,
        "addToPlaylist",
        json!({ "playlistId": global, "itemIds": item_globals(&query.list("ids"))? }),
    )
    .await?;
    Ok(no_content())
}

/// The global id of an entry a request names, which must be of this playlist.
fn entry_of<'a>(entries: &'a [Entry], jellyfin: &str) -> Result<&'a Entry, JellyfinError> {
    let global = id::global_of_type(jellyfin, klapz_api::playlist::PLAYLIST_ENTRY_TYPE)?;
    entries
        .iter()
        .find(|entry| entry.id == global)
        .ok_or(JellyfinError::NotFound)
}

/// `/Playlists/{id}/Items?entryIds=`.
async fn remove(
    State(state): State<JellyfinState>,
    Path(id): Path<String>,
    RawQuery(raw): RawQuery,
) -> Result<Response, JellyfinError> {
    let query = Query::parse(raw.as_deref());
    let global = playlist_global(&id)?;
    let entries = all_entries(&state, &global).await?;
    let named = query.list("entryIds");
    let removed = named
        .iter()
        .map(|entry| entry_of(&entries, entry).map(|entry| entry.id.clone()))
        .collect::<Result<Vec<_>, _>>()?;
    for entry in removed {
        mutate(&state, "removeFromPlaylist", json!({ "entryId": entry })).await?;
    }
    Ok(no_content())
}

/// Where an entry lands when the reference's clients ask for an index: after the entry that
/// holds the index before it once the moved one is taken out, first for index 0.
fn after_for_index<'a>(order: &[&'a str], moved: &str, index: usize) -> Option<&'a str> {
    let others: Vec<&'a str> = order
        .iter()
        .copied()
        .filter(|entry| *entry != moved)
        .collect();
    let index = index.min(others.len());
    index.checked_sub(1).map(|before| others[before])
}

/// `/Playlists/{id}/Items/{entry}/Move/{index}`.
async fn move_entry(
    State(state): State<JellyfinState>,
    Path((id, entry, index)): Path<(String, String, String)>,
) -> Result<Response, JellyfinError> {
    let global = playlist_global(&id)?;
    let index: usize = index
        .parse()
        .map_err(|_| JellyfinError::BadRequest("an index is not a number"))?;
    let entries = all_entries(&state, &global).await?;
    let moved = entry_of(&entries, &entry)?.id.clone();
    let order: Vec<&str> = entries.iter().map(|entry| entry.id.as_str()).collect();
    let after = after_for_index(&order, &moved, index);
    mutate(
        &state,
        "movePlaylistEntry",
        json!({ "entryId": moved, "afterEntryId": after }),
    )
    .await?;
    Ok(no_content())
}

/// `/Playlists/{id}/Users`: whom it is shared with, nobody here.
async fn users(
    State(state): State<JellyfinState>,
    Path(id): Path<String>,
) -> Result<Response, JellyfinError> {
    playlist_of(&state, &playlist_global(&id)?).await?;
    Ok(json(&json!([])))
}

/// `/Playlists/{id}/Users/{user}`: its owner may edit it; anyone else does not reach it.
async fn user(
    State(state): State<JellyfinState>,
    Path((id, user)): Path<(String, String)>,
) -> Result<Response, JellyfinError> {
    playlist_of(&state, &playlist_global(&id)?).await?;
    let viewer = gql::viewer(&state.schema).await?;
    viewer.require_self(Some(&user))?;
    Ok(json(
        &json!({ "UserId": viewer.jellyfin_id(), "CanEdit": true }),
    ))
}

/// Deletes one of the caller's playlists.
pub async fn delete(state: &JellyfinState, global: &str) -> Result<(), JellyfinError> {
    mutate(state, "deletePlaylist", json!({ "id": global })).await?;
    Ok(())
}

async fn no_sharing() -> JellyfinError {
    JellyfinError::NotImplemented("a playlist shared with others")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn an_index_lands_after_the_entry_before_it_once_the_moved_one_is_out() {
        let order = ["a", "b", "c", "d"];
        assert_eq!(after_for_index(&order, "c", 0), None);
        assert_eq!(after_for_index(&order, "c", 1), Some("a"));
        assert_eq!(after_for_index(&order, "a", 2), Some("c"));
        assert_eq!(after_for_index(&order, "a", 99), Some("d"));
    }

    #[test]
    fn a_playlist_is_a_folder_counting_what_is_left_to_watch() {
        let watched = |is_watched| Watched {
            watch_state: Some(WatchedState { is_watched }),
        };
        let playlist = Playlist {
            id: klapz_api::GlobalId::encode("Playlist", 3),
            title: "Soirée".to_owned(),
            kind: "VIDEO".to_owned(),
            item_count: 2,
            duration_ms: Some(178_301),
            added_at: "2026-11-20T12:31:16Z".to_owned(),
            items: Nodes {
                nodes: vec![watched(true), watched(false)],
            },
        };
        let dto = playlist_dto(&playlist, "s", &Fields::named([]));
        assert_eq!(dto["Type"], "Playlist");
        assert_eq!(dto["IsFolder"], true);
        assert_eq!(dto["MediaType"], "Video");
        assert_eq!(dto["ChildCount"], 2);
        assert_eq!(dto["UserData"]["UnplayedItemCount"], 1);
        assert_eq!(dto["UserData"]["PlayedPercentage"], 50.0);
        assert_eq!(dto["RunTimeTicks"], 1_783_010_000_i64);
        assert!(dto.get("DateCreated").is_none());
    }

    #[test]
    fn a_shared_playlist_is_refused_and_a_body_wins_over_the_query() {
        let query = Query::parse(Some("Name=query&Ids=a,b"));
        let request = PlaylistRequest::read(br#"{"Name":"body"}"#, &query).unwrap();
        assert_eq!(request.name.as_deref(), Some("body"));
        assert_eq!(request.ids, ["a", "b"]);
        assert!(matches!(
            PlaylistRequest::read(br#"{"IsPublic":true}"#, &query),
            Err(JellyfinError::NotImplemented(_))
        ));
        assert_eq!(input_type("movePlaylistEntry"), "MovePlaylistEntry");
    }
}
