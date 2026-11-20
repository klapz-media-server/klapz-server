//! Items: listings, lookups, and the shelves a home screen shows.

use axum::extract::{Path, RawQuery, State};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::dto::{item_dto, Fields, Item, ItemsResult, Shaping, ITEM_FIELDS};
use crate::error::JellyfinError;
use crate::gql::{self, Viewer};
use crate::id;
use crate::listing::{Listing, MAX_PAGE};
use crate::query::Query;
use crate::response::json;
use crate::views::{libraries, library_dto, user_root_id, Library};
use crate::JellyfinState;

pub fn router() -> Router<JellyfinState> {
    Router::new()
        .route("/items", get(items))
        .route("/users/{user}/items", get(legacy_items))
        .route("/items/latest", get(latest))
        .route("/users/{user}/items/latest", get(legacy_latest))
        .route("/useritems/resume", get(resume))
        .route("/users/{user}/items/resume", get(legacy_resume))
        .route("/items/{id}", get(item).delete(delete_item))
        .route("/users/{user}/items/{id}", get(legacy_item))
        .route("/items/{id}/similar", get(similar))
        .route("/items/{id}/ancestors", get(ancestors))
        .route("/items/{id}/thememedia", get(no_theme_media))
        .route("/users/{user}/items/{id}/intros", get(no_intros))
        .route("/items/{id}/intros", get(no_intros))
        .route("/items/{id}/specialfeatures", get(no_items_list))
        .route(
            "/users/{user}/items/{id}/specialfeatures",
            get(no_items_list),
        )
        .route("/items/{id}/localtrailers", get(no_items_list))
        .route("/users/{user}/items/{id}/localtrailers", get(no_items_list))
        .route("/items/{id}/images", get(no_items_list))
        .route("/items/{id}/images/{kind}", get(no_image))
        .route("/items/{id}/images/{kind}/{index}", get(no_image))
}

/// What the request's `Fields` names, and the official API's variables that follow from it.
fn fields(query: &Query) -> Fields {
    Fields::named(query.list("fields"))
}

fn shaped(items: &[Item], state: &JellyfinState, fields: &Fields) -> Vec<Value> {
    let shaping = Shaping {
        server_id: &state.server.id,
        fields,
    };
    items.iter().map(|item| item_dto(item, &shaping)).collect()
}

fn page(items: Vec<Value>, total: i64, start: u32) -> Response {
    json(&ItemsResult {
        items,
        total_record_count: total,
        start_index: start,
    })
}

fn count(items: &[Value]) -> i64 {
    i64::try_from(items.len()).unwrap_or(i64::MAX)
}

#[derive(Deserialize)]
struct NodeData<T> {
    node: Option<T>,
}

#[derive(Deserialize)]
struct NodesData<T> {
    nodes: Vec<Option<T>>,
}

#[derive(Deserialize)]
struct Connection<T> {
    nodes: Vec<T>,
}

/// A library's page, as the official API lists it.
#[derive(Deserialize)]
struct LibraryPage {
    stats: crate::views::LibraryStats,
    items: Connection<Item>,
}

/// A series' seasons or a season's episodes.
#[derive(Deserialize)]
struct Children {
    seasons: Option<Connection<Item>>,
    episodes: Option<Connection<Item>>,
}

fn library_page_query() -> String {
    format!(
        r"query($id: ID!, $filter: ItemFilterInput, $order: ItemOrder, $direction: SortDirection,
                $offset: Int, $first: Int, $people: Boolean!,
                $chapters: Boolean!) {{
            node(id: $id) {{ ... on Library {{
                stats(filter: $filter) {{ itemCount }}
                items(filter: $filter, orderBy: $order, direction: $direction, offset: $offset,
                      first: $first) {{ nodes {{ ...JellyfinItem }} }}
            }} }}
        }}
        {ITEM_FIELDS}"
    )
}

fn children_query() -> String {
    format!(
        r"query($id: ID!, $people: Boolean!,
                $chapters: Boolean!) {{
            node(id: $id) {{
                ... on Show {{ seasons(first: {MAX_PAGE}) {{ nodes {{ ...JellyfinItem }} }} }}
                ... on Season {{ episodes(first: {MAX_PAGE}) {{ nodes {{ ...JellyfinItem }} }} }}
            }}
        }}
        {ITEM_FIELDS}"
    )
}

fn items_query() -> String {
    format!(
        r"query($ids: [ID!]!, $people: Boolean!,
                $chapters: Boolean!) {{
            nodes(ids: $ids) {{ ...JellyfinItem }}
        }}
        {ITEM_FIELDS}"
    )
}

fn search_query() -> String {
    format!(
        r"query($query: String!, $kinds: [ItemType!], $first: Int, $people: Boolean!,
                $chapters: Boolean!) {{
            search(query: $query, kinds: $kinds, first: $first) {{ nodes {{ ...JellyfinItem }} }}
        }}
        {ITEM_FIELDS}"
    )
}

pub(crate) async fn library_page(
    state: &JellyfinState,
    library: &str,
    listing: &Listing,
    fields: &Fields,
) -> Result<(Vec<Item>, i64), JellyfinError> {
    let data: NodeData<LibraryPage> = gql::query(
        &state.schema,
        &library_page_query(),
        json!({
            "id": library,
            "filter": listing.filter_json(),
            "order": listing.order,
            "direction": listing.direction,
            "offset": listing.offset,
            "first": listing.first,
            "people": fields.wants_people(),
            "chapters": fields.has("Chapters"),
        }),
    )
    .await?;
    let page = data.node.ok_or(JellyfinError::NotFound)?;
    Ok((page.items.nodes, page.stats.item_count))
}

/// A series' seasons, or a season's episodes; nothing below a film or an episode.
async fn children(
    state: &JellyfinState,
    parent: &str,
    fields: &Fields,
) -> Result<Vec<Item>, JellyfinError> {
    let data: NodeData<Children> = gql::query(
        &state.schema,
        &children_query(),
        json!({ "id": parent, "people": fields.wants_people(),
            "chapters": fields.has("Chapters") }),
    )
    .await?;
    let children = data.node.ok_or(JellyfinError::NotFound)?;
    Ok(children
        .seasons
        .or(children.episodes)
        .map(|connection| connection.nodes)
        .unwrap_or_default())
}

pub async fn items_by_id(
    state: &JellyfinState,
    ids: &[String],
    fields: &Fields,
) -> Result<Vec<Item>, JellyfinError> {
    let data: NodesData<Item> = gql::query(
        &state.schema,
        &items_query(),
        json!({ "ids": ids, "people": fields.wants_people(),
            "chapters": fields.has("Chapters") }),
    )
    .await?;
    Ok(data.nodes.into_iter().flatten().collect())
}

/// A window of the items held under several libraries, one after another.
async fn across_libraries(
    state: &JellyfinState,
    listing: &Listing,
    fields: &Fields,
) -> Result<(Vec<Item>, i64), JellyfinError> {
    let mut items = Vec::new();
    let mut total = 0;
    let mut skip = i64::from(listing.offset);
    for library in libraries(state).await? {
        let holds = match library.kind.as_str() {
            "MOVIE" => listing.kinds.contains(&"MOVIE"),
            _ => listing
                .kinds
                .iter()
                .any(|kind| matches!(*kind, "SHOW" | "SEASON" | "EPISODE")),
        };
        if !holds {
            continue;
        }
        let room = i64::from(listing.first) - count_items(&items);
        let window = Listing {
            offset: u32::try_from(skip.max(0)).unwrap_or(u32::MAX),
            first: u32::try_from(room.max(0)).unwrap_or(0),
            ..listing.clone()
        };
        let (page, in_library) = library_page(state, &library.id, &window, fields).await?;
        total += in_library;
        skip = (skip - in_library).max(0);
        if window.first > 0 {
            items.extend(page);
        }
    }
    Ok((items, total))
}

fn count_items(items: &[Item]) -> i64 {
    i64::try_from(items.len()).unwrap_or(i64::MAX)
}

async fn search(
    state: &JellyfinState,
    term: &str,
    listing: &Listing,
    fields: &Fields,
) -> Result<Vec<Item>, JellyfinError> {
    #[derive(Deserialize)]
    struct SearchData {
        search: Connection<Item>,
    }
    if listing.offset > 0 {
        return Err(JellyfinError::NotImplemented(
            "a search page past the first",
        ));
    }
    let data: SearchData = gql::query(
        &state.schema,
        &search_query(),
        json!({
            "query": term,
            "kinds": listing.kinds,
            "first": listing.first,
            "people": fields.wants_people(),
            "chapters": fields.has("Chapters"),
        }),
    )
    .await?;
    Ok(data.search.nodes)
}

/// `/Items`: under a parent, by ids, by a search term, or across the caller's libraries.
async fn list(
    state: &JellyfinState,
    user: Option<&str>,
    query: &Query,
) -> Result<Response, JellyfinError> {
    let viewer: Viewer = gql::viewer(&state.schema).await?;
    viewer.require_self(user.or_else(|| query.get("userId")))?;
    if let Some(answer) = containers(state, query).await? {
        return Ok(answer);
    }
    if !query.list("personIds").is_empty() {
        return credited(state, query).await;
    }
    let listing = Listing::parse(query)?;
    let fields = fields(query);
    let start = listing.offset;
    if listing.favorites_only || listing.kinds.is_empty() {
        return Ok(page(Vec::new(), 0, start));
    }
    let ids = query.list("ids");
    if !ids.is_empty() {
        let globals = ids
            .iter()
            .map(|jellyfin| {
                id::decode(jellyfin)
                    .map(|global| klapz_api::GlobalId::encode(&global.type_name, global.key))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let items = shaped(
            &items_by_id(state, &globals, &fields).await?,
            state,
            &fields,
        );
        let total = count(&items);
        return Ok(page(items, total, 0));
    }
    if let Some(parent) = query.get("parentId") {
        let parent = id::decode(parent)?;
        let global = klapz_api::GlobalId::encode(&parent.type_name, parent.key);
        return match parent.type_name.as_str() {
            klapz_api::library::LIBRARY_TYPE => {
                let (items, total) = library_page(state, &global, &listing, &fields).await?;
                Ok(page(shaped(&items, state, &fields), total, start))
            }
            klapz_api::item::ITEM_TYPE => {
                let all = children(state, &global, &fields).await?;
                let items: Vec<Item> = all
                    .into_iter()
                    .skip(usize::try_from(start).unwrap_or(usize::MAX))
                    .collect();
                let total = count_items(&items) + i64::from(start);
                let shown: Vec<Value> = shaped(&items, state, &fields)
                    .into_iter()
                    .take(usize::try_from(listing.first).unwrap_or(usize::MAX))
                    .collect();
                Ok(page(shown, total, start))
            }
            klapz_api::server::SERVER_TYPE => {
                let views: Vec<Value> = libraries(state)
                    .await?
                    .iter()
                    .map(|library: &Library| library_dto(library, &state.server.id))
                    .collect();
                let total = count(&views);
                Ok(page(views, total, 0))
            }
            _ => Err(JellyfinError::NotFound),
        };
    }
    if let Some(term) = query.get("searchTerm") {
        let items = shaped(
            &search(state, term, &listing, &fields).await?,
            state,
            &fields,
        );
        let total = count(&items);
        return Ok(page(items, total, 0));
    }
    let (items, total) = across_libraries(state, &listing, &fields).await?;
    Ok(page(shaped(&items, state, &fields), total, start))
}

async fn items(
    State(state): State<JellyfinState>,
    RawQuery(query): RawQuery,
) -> Result<Response, JellyfinError> {
    list(&state, None, &Query::parse(query.as_deref())).await
}

async fn legacy_items(
    State(state): State<JellyfinState>,
    Path(user): Path<String>,
    RawQuery(query): RawQuery,
) -> Result<Response, JellyfinError> {
    list(&state, Some(&user), &Query::parse(query.as_deref())).await
}

/// One item with every field: an item of a library, a library, or the user's root.
async fn lookup(
    state: &JellyfinState,
    user: Option<&str>,
    item: &str,
) -> Result<Response, JellyfinError> {
    let viewer = gql::viewer(&state.schema).await?;
    viewer.require_self(user)?;
    let global = id::decode(item)?;
    let encoded = klapz_api::GlobalId::encode(&global.type_name, global.key);
    match global.type_name.as_str() {
        klapz_api::item::ITEM_TYPE => {
            let fields = Fields::all();
            let found = items_by_id(state, &[encoded], &fields).await?;
            let item = found.first().ok_or(JellyfinError::NotFound)?;
            Ok(json(&item_dto(
                item,
                &Shaping {
                    server_id: &state.server.id,
                    fields: &fields,
                },
            )))
        }
        klapz_api::library::LIBRARY_TYPE => {
            let library = libraries(state)
                .await?
                .into_iter()
                .find(|library| library.id == encoded)
                .ok_or(JellyfinError::NotFound)?;
            Ok(json(&library_dto(&library, &state.server.id)))
        }
        klapz_api::server::SERVER_TYPE if global.key == 2 => {
            let count = crate::collections::collections(state).await?.len();
            Ok(json(&crate::collections::view_dto(&state.server.id, count)))
        }
        klapz_api::collection::COLLECTION_TYPE => {
            let collection = crate::collections::collection_of(state, &encoded).await?;
            Ok(json(&crate::collections::collection_dto(
                &collection,
                &state.server.id,
                &Fields::all(),
            )))
        }
        klapz_api::server::SERVER_TYPE if global.key == 1 => {
            let count = crate::playlists::playlists(state).await?.len();
            Ok(json(&crate::playlists::view_dto(&state.server.id, count)))
        }
        klapz_api::server::SERVER_TYPE => Ok(json(&user_root(state))),
        klapz_api::playlist::PLAYLIST_TYPE => {
            let playlist = crate::playlists::playlist_of(state, &encoded).await?;
            Ok(json(&crate::playlists::playlist_dto(
                &playlist,
                &state.server.id,
                &Fields::all(),
            )))
        }
        _ => Err(JellyfinError::NotFound),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreditedPage {
    item_count: i64,
    items: Connection<Item>,
}

/// `/Items?PersonIds=`: the films and series crediting one person, in any role, a series for a
/// credit on one of its episodes, through `Person.items`; the request's other filters apply.
async fn credited(state: &JellyfinState, query: &Query) -> Result<Response, JellyfinError> {
    let [person] = query.list("personIds")[..] else {
        return Err(JellyfinError::NotImplemented("several people at once"));
    };
    let person = id::global_of_type(person, klapz_api::tag::PERSON_TYPE)?;
    if query.get("parentId").is_some() {
        return Err(JellyfinError::NotImplemented(
            "a person's credits below a parent",
        ));
    }
    // Films and series together are what a person's credits list; one of them is a kind.
    let mut narrowed = query.clone();
    let types = query.list("includeItemTypes");
    if !types.is_empty()
        && types
            .iter()
            .all(|kind| kind.eq_ignore_ascii_case("movie") || kind.eq_ignore_ascii_case("series"))
        && types.len() > 1
    {
        narrowed.set("includeItemTypes", "");
        narrowed.set("recursive", "false");
    }
    let listing = Listing::parse(&narrowed)?;
    if listing.order.is_some_and(|order| order != "TITLE") {
        // Credits come in the official API's order: library, then title.
        return Err(JellyfinError::NotImplemented(
            "a person's credits in another order",
        ));
    }
    let fields = fields(query);
    let data: NodeData<CreditedPage> = gql::query(
        &state.schema,
        &format!(
            r"query($id: ID!, $filter: ItemFilterInput, $offset: Int, $first: Int,
                    $people: Boolean!, $chapters: Boolean!) {{
                node(id: $id) {{ ... on Person {{
                    itemCount(filter: $filter)
                    items(filter: $filter, offset: $offset, first: $first) {{ nodes {{ ...JellyfinItem }} }}
                }} }}
            }}
            {ITEM_FIELDS}"
        ),
        json!({
            "id": person,
            "filter": listing.filter_json(),
            "offset": listing.offset,
            "first": listing.first,
            "people": fields.wants_people(),
            "chapters": fields.has("Chapters"),
        }),
    )
    .await?;
    let found = data.node.ok_or(JellyfinError::NotFound)?;
    Ok(page(
        shaped(&found.items.nodes, state, &fields),
        found.item_count,
        listing.offset,
    ))
}

/// Listings of what is not a library's item: the caller's playlists and collections, as a type
/// or as their view, and what a playlist or a collection holds. `None` for every other
/// listing.
async fn containers(
    state: &JellyfinState,
    query: &Query,
) -> Result<Option<Response>, JellyfinError> {
    let parent = query.get("parentId").map(id::decode).transpose()?;
    let types = query.list("includeItemTypes");
    let only =
        |name: &str| !types.is_empty() && types.iter().all(|kind| kind.eq_ignore_ascii_case(name));
    let names = |name: &str| types.iter().any(|kind| kind.eq_ignore_ascii_case(name));
    let view = |key: i64| {
        parent.as_ref().is_some_and(|parent| {
            parent.type_name == klapz_api::server::SERVER_TYPE && parent.key == key
        })
    };
    let under_library = parent
        .as_ref()
        .filter(|parent| parent.type_name == klapz_api::library::LIBRARY_TYPE)
        .map(|library| klapz_api::GlobalId::encode(&library.type_name, library.key));
    if view(1) || (only("playlist") && parent.is_none()) {
        return Ok(Some(playlists_page(state, query).await?));
    }
    if view(2) || (only("boxset") && (parent.is_none() || under_library.is_some())) {
        return Ok(Some(
            collections_page(state, query, under_library.as_deref()).await?,
        ));
    }
    if names("playlist") || names("boxset") {
        return Err(JellyfinError::NotImplemented(
            "playlists or collections among other types, or below an item",
        ));
    }
    let Some(parent) = parent else {
        return Ok(None);
    };
    let global = klapz_api::GlobalId::encode(&parent.type_name, parent.key);
    match parent.type_name.as_str() {
        klapz_api::playlist::PLAYLIST_TYPE => Ok(Some(
            crate::playlists::entries_page(state, &global, query).await?,
        )),
        klapz_api::collection::COLLECTION_TYPE => Ok(Some(
            crate::collections::items_page(state, &global, query).await?,
        )),
        _ => Ok(None),
    }
}

async fn playlists_page(state: &JellyfinState, query: &Query) -> Result<Response, JellyfinError> {
    let fields = fields(query);
    let items: Vec<Value> = crate::playlists::playlists(state)
        .await?
        .iter()
        .map(|playlist| crate::playlists::playlist_dto(playlist, &state.server.id, &fields))
        .collect();
    let total = count(&items);
    Ok(page(items, total, 0))
}

/// The caller's collections, or one library's.
async fn collections_page(
    state: &JellyfinState,
    query: &Query,
    library: Option<&str>,
) -> Result<Response, JellyfinError> {
    let fields = fields(query);
    let items: Vec<Value> = crate::collections::collections(state)
        .await?
        .iter()
        .filter(|collection| library.is_none_or(|library| collection.library_id() == Some(library)))
        .map(|collection| crate::collections::collection_dto(collection, &state.server.id, &fields))
        .collect();
    let total = count(&items);
    Ok(page(items, total, 0))
}

/// `DELETE /Items/{id}`: a playlist the caller owns, or a collection of a library it may change.
/// Media files are never deleted here.
async fn delete_item(
    State(state): State<JellyfinState>,
    Path(item): Path<String>,
) -> Result<Response, JellyfinError> {
    let global = id::decode(&item)?;
    let encoded = klapz_api::GlobalId::encode(&global.type_name, global.key);
    match global.type_name.as_str() {
        klapz_api::playlist::PLAYLIST_TYPE => {
            crate::playlists::delete(&state, &encoded).await?;
        }
        klapz_api::collection::COLLECTION_TYPE => {
            crate::collections::delete(&state, &encoded).await?;
        }
        _ => {
            return Err(JellyfinError::NotImplemented(
                "deleting media from the library",
            ))
        }
    }
    Ok(crate::response::no_content())
}

/// The folder a user's views hang from.
fn user_root(state: &JellyfinState) -> Value {
    let root = user_root_id();
    json!({
        "Name": "Media Folders",
        "ServerId": state.server.id,
        "Id": root,
        "ChannelId": null,
        "IsFolder": true,
        "Type": "UserRootFolder",
        "UserData": {
            "PlaybackPositionTicks": 0,
            "PlayCount": 0,
            "IsFavorite": false,
            "Played": false,
            "Key": id::dashed(&root),
            "ItemId": root,
        },
        "ImageTags": {},
        "BackdropImageTags": [],
        "ImageBlurHashes": {},
        "LocationType": "FileSystem",
        "MediaType": "Unknown",
    })
}

async fn item(
    State(state): State<JellyfinState>,
    Path(item): Path<String>,
    RawQuery(query): RawQuery,
) -> Result<Response, JellyfinError> {
    let query = Query::parse(query.as_deref());
    lookup(&state, query.get("userId"), &item).await
}

async fn legacy_item(
    State(state): State<JellyfinState>,
    Path((user, item)): Path<(String, String)>,
) -> Result<Response, JellyfinError> {
    lookup(&state, Some(&user), &item).await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecentEntries {
    recently_added: Connection<Item>,
}

/// What was added last under a library — films, or episodes grouped into their season or
/// series as the official API groups them. An array, not a page, as on the reference.
async fn latest_items(
    state: &JellyfinState,
    user: Option<&str>,
    query: &Query,
) -> Result<Response, JellyfinError> {
    let viewer = gql::viewer(&state.schema).await?;
    viewer.require_self(user.or_else(|| query.get("userId")))?;
    let fields = fields(query);
    let limit = query.count("limit")?.unwrap_or(20).min(MAX_PAGE);
    let mut kinds: Vec<(&str, Option<String>)> = Vec::new();
    if let Some(parent) = query.get("parentId") {
        let library = id::global_of_type(parent, klapz_api::library::LIBRARY_TYPE)?;
        let kind = libraries(state)
            .await?
            .into_iter()
            .find(|candidate| candidate.id == library)
            .map(|library| {
                if library.kind == "MOVIE" {
                    "MOVIE"
                } else {
                    "SHOW"
                }
            })
            .ok_or(JellyfinError::NotFound)?;
        kinds.push((kind, Some(library)));
    } else {
        kinds.push(("MOVIE", None));
        kinds.push(("SHOW", None));
    }
    let latest_query = format!(
        r"query($kind: LibraryKind!, $library: ID, $first: Int, $people: Boolean!,
                $chapters: Boolean!) {{
            recentlyAdded(kind: $kind, library: $library, first: $first) {{
                nodes {{ ...JellyfinItem }}
            }}
        }}
        {ITEM_FIELDS}"
    );
    let mut items = Vec::new();
    for (kind, library) in kinds {
        let data: RecentEntries = gql::query(
            &state.schema,
            &latest_query,
            json!({
                "kind": kind,
                "library": library,
                "first": limit,
                "people": fields.wants_people(),
            "chapters": fields.has("Chapters"),
            }),
        )
        .await?;
        items.extend(data.recently_added.nodes);
    }
    items.sort_by(|one, other| other.added_at.cmp(&one.added_at));
    items.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
    Ok(json(&shaped(&items, state, &fields)))
}

async fn latest(
    State(state): State<JellyfinState>,
    RawQuery(query): RawQuery,
) -> Result<Response, JellyfinError> {
    latest_items(&state, None, &Query::parse(query.as_deref())).await
}

async fn legacy_latest(
    State(state): State<JellyfinState>,
    Path(user): Path<String>,
    RawQuery(query): RawQuery,
) -> Result<Response, JellyfinError> {
    latest_items(&state, Some(&user), &Query::parse(query.as_deref())).await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContinueData {
    continue_watching: ContinueConnection,
}

#[derive(Deserialize)]
struct ContinueConnection {
    edges: Vec<ContinueEdge>,
}

#[derive(Deserialize)]
struct ContinueEdge {
    node: Item,
    reason: String,
}

/// What the caller has started or comes next, by the official API's rule, newest first.
pub async fn continue_watching(
    state: &JellyfinState,
    library: Option<String>,
    fields: &Fields,
) -> Result<Vec<(Item, String)>, JellyfinError> {
    let continue_query = format!(
        r"query($library: ID, $people: Boolean!,
                $chapters: Boolean!) {{
            continueWatching(library: $library, first: {MAX_PAGE}) {{
                edges {{ reason node {{ ...JellyfinItem }} }}
            }}
        }}
        {ITEM_FIELDS}"
    );
    let data: ContinueData = gql::query(
        &state.schema,
        &continue_query,
        json!({ "library": library, "people": fields.wants_people(),
            "chapters": fields.has("Chapters") }),
    )
    .await?;
    Ok(data
        .continue_watching
        .edges
        .into_iter()
        .map(|edge| (edge.node, edge.reason))
        .collect())
}

/// Items in progress: the official API's continue-watching entries started rather than
/// coming next.
async fn resume_items(
    state: &JellyfinState,
    user: Option<&str>,
    query: &Query,
) -> Result<Response, JellyfinError> {
    let viewer = gql::viewer(&state.schema).await?;
    viewer.require_self(user.or_else(|| query.get("userId")))?;
    let fields = fields(query);
    let library = query
        .get("parentId")
        .map(|parent| id::global_of_type(parent, klapz_api::library::LIBRARY_TYPE))
        .transpose()?;
    let start = query.count("startIndex")?.unwrap_or(0);
    let limit = query.count("limit")?.unwrap_or(MAX_PAGE).min(MAX_PAGE);
    let started: Vec<Item> = continue_watching(state, library, &fields)
        .await?
        .into_iter()
        .filter(|(_, reason)| reason == "IN_PROGRESS")
        .map(|(item, _)| item)
        .collect();
    let total = count_items(&started);
    let window: Vec<Item> = started
        .into_iter()
        .skip(usize::try_from(start).unwrap_or(usize::MAX))
        .take(usize::try_from(limit).unwrap_or(usize::MAX))
        .collect();
    Ok(page(shaped(&window, state, &fields), total, start))
}

async fn resume(
    State(state): State<JellyfinState>,
    RawQuery(query): RawQuery,
) -> Result<Response, JellyfinError> {
    resume_items(&state, None, &Query::parse(query.as_deref())).await
}

async fn legacy_resume(
    State(state): State<JellyfinState>,
    Path(user): Path<String>,
    RawQuery(query): RawQuery,
) -> Result<Response, JellyfinError> {
    resume_items(&state, Some(&user), &Query::parse(query.as_deref())).await
}

#[derive(Deserialize)]
struct Similar {
    similar: Option<Connection<Item>>,
}

/// Films or series alike, by the official API's measure; nothing for a season or an episode.
async fn similar(
    State(state): State<JellyfinState>,
    Path(item): Path<String>,
    RawQuery(query): RawQuery,
) -> Result<Response, JellyfinError> {
    let query = Query::parse(query.as_deref());
    let viewer = gql::viewer(&state.schema).await?;
    viewer.require_self(query.get("userId"))?;
    let global = id::global_of_type(&item, klapz_api::item::ITEM_TYPE)?;
    let fields = fields(&query);
    let limit = query.count("limit")?.unwrap_or(12).min(MAX_PAGE);
    let similar_query = format!(
        r"query($id: ID!, $first: Int, $people: Boolean!,
                $chapters: Boolean!) {{
            node(id: $id) {{
                ... on Movie {{ similar(first: $first) {{ nodes {{ ...JellyfinItem }} }} }}
                ... on Show {{ similar(first: $first) {{ nodes {{ ...JellyfinItem }} }} }}
            }}
        }}
        {ITEM_FIELDS}"
    );
    let data: NodeData<Similar> = gql::query(
        &state.schema,
        &similar_query,
        json!({ "id": global, "first": limit, "people": fields.wants_people(),
            "chapters": fields.has("Chapters") }),
    )
    .await?;
    let items = data
        .node
        .ok_or(JellyfinError::NotFound)?
        .similar
        .map(|connection| connection.nodes)
        .unwrap_or_default();
    let shown = shaped(&items, &state, &fields);
    let total = count(&shown);
    Ok(page(shown, total, 0))
}

/// What an item hangs from, nearest first: its season and series, its library, the user's
/// root.
async fn ancestors(
    State(state): State<JellyfinState>,
    Path(item): Path<String>,
) -> Result<Response, JellyfinError> {
    gql::viewer(&state.schema).await?;
    let global = id::global_of_type(&item, klapz_api::item::ITEM_TYPE)?;
    let fields = Fields::all();
    let found = items_by_id(&state, &[global], &fields).await?;
    let item = found.first().ok_or(JellyfinError::NotFound)?;
    let mut parents: Vec<String> = Vec::new();
    if let Some(season) = &item.season {
        parents.push(season.id.clone());
    }
    if let Some(show) = &item.show {
        parents.push(show.id.clone());
    }
    let mut chain = shaped(
        &items_by_id(&state, &parents, &fields).await?,
        &state,
        &fields,
    );
    if let Some(library) = &item.library {
        if let Some(library) = libraries(&state)
            .await?
            .iter()
            .find(|candidate| candidate.id == library.id)
        {
            chain.push(library_dto(library, &state.server.id));
        }
    }
    chain.push(user_root(&state));
    Ok(json(&chain))
}

/// Theme songs and videos: none is held here.
async fn no_theme_media(State(state): State<JellyfinState>) -> Result<Response, JellyfinError> {
    gql::viewer(&state.schema).await?;
    let none = || json!({ "Items": [], "TotalRecordCount": 0, "StartIndex": 0, "OwnerId": null });
    Ok(json(&json!({
        "ThemeVideosResult": none(),
        "ThemeSongsResult": none(),
        "SoundtrackSongsResult": none(),
    })))
}

/// What plays before an item: nothing is set up to here.
async fn no_intros(State(state): State<JellyfinState>) -> Result<Response, JellyfinError> {
    gql::viewer(&state.schema).await?;
    Ok(page(Vec::new(), 0, 0))
}

/// Trailers, special features: none is listed here, an empty array as on an item without.
async fn no_items_list(State(state): State<JellyfinState>) -> Result<Response, JellyfinError> {
    gql::viewer(&state.schema).await?;
    Ok(json(&[(); 0]))
}

/// No artwork is served: every image answers as a missing one does on the reference.
async fn no_image() -> JellyfinError {
    JellyfinError::NotFound
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_listing_queries_carry_the_item_fragment() {
        for query in [
            library_page_query(),
            children_query(),
            items_query(),
            search_query(),
        ] {
            assert!(query.contains("...JellyfinItem"));
            assert!(query.contains("fragment JellyfinItem on Item"));
        }
    }
}
