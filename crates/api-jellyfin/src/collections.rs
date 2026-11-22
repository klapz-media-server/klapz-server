//! Collections, Jellyfin's box sets, over the official API's collections.
//!
//! A collection here belongs to a library, where Jellyfin's stand apart: one made from Jellyfin
//! goes to the library of its first item, which is the only one whose items it can hold. Listed
//! by type and in a `Collections` view once there is one, opened as a parent, read and changed
//! by whoever may change its library.

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
        .route("/collections", post(create))
        .route("/collections/{id}/items", post(add).delete(remove))
        .route("/items/{id}/collections", get(item_collections))
}

/// What the official API is asked of a collection, wherever it is shown.
pub const COLLECTION_FIELDS: &str = r"
fragment JellyfinCollection on Collection {
    id title titleSort contentRating order minYear addedAt itemCount
    library { id }
    items(first: 500) { nodes { watchState { isWatched } } }
}";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Collection {
    pub id: String,
    pub title: String,
    pub title_sort: Option<String>,
    pub content_rating: Option<String>,
    pub order: String,
    pub min_year: Option<i32>,
    pub added_at: String,
    pub item_count: i64,
    pub library: Option<LibraryRef>,
    pub items: Nodes<Watched>,
}

impl Collection {
    /// The global id of its library.
    pub fn library_id(&self) -> Option<&str> {
        self.library.as_ref().map(|library| library.id.as_str())
    }
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

/// The view the caller's collections hang from, among the libraries once there is one.
pub fn view_id() -> String {
    id::of(klapz_api::server::SERVER_TYPE, 2).unwrap_or_default()
}

pub fn view_dto(server_id: &str, count: usize) -> Value {
    let view = view_id();
    json!({
        "Name": "Collections",
        "ServerId": server_id,
        "Id": view,
        "IsFolder": true,
        "Type": "CollectionFolder",
        "UserData": {
            "PlaybackPositionTicks": 0,
            "PlayCount": 0,
            "IsFavorite": false,
            "Played": false,
            "Key": id::dashed(&view),
            "ItemId": view,
        },
        "ChildCount": count,
        "CollectionType": "boxsets",
        "LocationType": "FileSystem",
        "MediaType": "Unknown",
    })
}

/// How a collection orders its items, as the reference names it.
fn display_order(order: &str) -> &'static str {
    match order {
        "TITLE" => "SortName",
        "CUSTOM" => "Default",
        _ => "PremiereDate",
    }
}

/// A collection as the reference shows a box set: a folder counting what is left to watch.
pub fn collection_dto(collection: &Collection, server_id: &str, fields: &Fields) -> Value {
    let jellyfin_id = id::of_global(&collection.id).unwrap_or_default();
    let unplayed = collection
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
    let count = collection.item_count;
    #[allow(
        clippy::cast_precision_loss,
        reason = "a collection's items are far below 2^52"
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
    put("Name", json!(collection.title));
    put("ServerId", json!(server_id));
    put("Id", json!(jellyfin_id));
    if fields.has("DateCreated") {
        put("DateCreated", json!(time::date(&collection.added_at)));
    }
    if fields.has("CanDelete") {
        put("CanDelete", json!(true));
    }
    if fields.has("SortName") {
        let sort = collection.title_sort.as_ref().unwrap_or(&collection.title);
        put("SortName", json!(sort.to_lowercase()));
    }
    if let Some(rating) = &collection.content_rating {
        put("OfficialRating", json!(rating));
    }
    put("ChannelId", Value::Null);
    if let Some(year) = collection.min_year {
        put("ProductionYear", json!(year));
    }
    put("IsFolder", json!(true));
    put("Type", json!("BoxSet"));
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
    if fields.has("ChildCount") {
        put("ChildCount", json!(count));
    }
    put("DisplayOrder", json!(display_order(&collection.order)));
    put("ImageTags", json!({}));
    put("BackdropImageTags", json!([]));
    put("ImageBlurHashes", json!({}));
    put("LocationType", json!("FileSystem"));
    put("MediaType", json!("Unknown"));
    Value::Object(dto)
}

#[derive(Deserialize)]
struct CollectionsData {
    collections: Nodes<Collection>,
}

/// The collections of every library the caller reads, by title; with `matching`, those whose
/// title a search term finds.
pub async fn collections(
    state: &JellyfinState,
    matching: Option<&str>,
) -> Result<Vec<Collection>, JellyfinError> {
    let data: CollectionsData = gql::query(
        &state.schema,
        &format!(
            "query($matching: String) {{
                collections(matching: $matching, orderBy: TITLE, first: 500) {{
                    nodes {{ ...JellyfinCollection }}
                }}
            }}
            {COLLECTION_FIELDS}"
        ),
        json!({ "matching": matching }),
    )
    .await?;
    Ok(data.collections.nodes)
}

#[derive(Deserialize)]
struct NodeData<T> {
    node: Option<T>,
}

pub async fn collection_of(
    state: &JellyfinState,
    global: &str,
) -> Result<Collection, JellyfinError> {
    let data: NodeData<Collection> = gql::query(
        &state.schema,
        &format!(
            "query($id: ID!) {{ node(id: $id) {{ ...JellyfinCollection }} }} {COLLECTION_FIELDS}"
        ),
        json!({ "id": global }),
    )
    .await?;
    data.node.ok_or(JellyfinError::NotFound)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ItemsPage {
    item_count: i64,
    items: Nodes<Item>,
}

/// A page of a collection's items, as `/Items?ParentId=` lists them.
pub async fn items_page(
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
    let data: NodeData<ItemsPage> = gql::query(
        &state.schema,
        &format!(
            r"query($id: ID!, $offset: Int, $first: Int, $people: Boolean!, $chapters: Boolean!) {{
                node(id: $id) {{ ... on Collection {{
                    itemCount
                    items(offset: $offset, first: $first) {{ nodes {{ ...JellyfinItem }} }}
                }} }}
            }}
            {ITEM_FIELDS}"
        ),
        json!({
            "id": global,
            "offset": start,
            "first": first,
            "people": fields.wants_people(),
            "chapters": fields.has("Chapters"),
        }),
    )
    .await?;
    let page = data.node.ok_or(JellyfinError::NotFound)?;
    let shaping = Shaping {
        server_id: &state.server.id,
        fields: &fields,
    };
    Ok(json(&ItemsResult {
        items: page
            .items
            .nodes
            .iter()
            .map(|item| item_dto(item, &shaping))
            .collect(),
        total_record_count: page.item_count,
        start_index: start,
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserError {
    code: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Refusals {
    user_errors: Vec<UserError>,
}

/// A refusal of the official API, as the reference answers the same.
fn refused(errors: &[UserError]) -> Result<(), JellyfinError> {
    match errors.first().map(|error| error.code.as_str()) {
        None => Ok(()),
        Some("COLLECTION_NOT_FOUND" | "LIBRARY_NOT_FOUND" | "ITEM_NOT_FOUND") => {
            Err(JellyfinError::NotFound)
        }
        Some(_) => Err(JellyfinError::BadRequest(
            "the collection refuses the change",
        )),
    }
}

async fn mutate(
    state: &JellyfinState,
    mutation: &str,
    input_type: &str,
    input: Value,
    selection: &str,
) -> Result<Value, JellyfinError> {
    let data: Map<String, Value> = gql::query(
        &state.schema,
        &format!(
            "mutation($input: {input_type}!) {{ {mutation}(input: $input) {{ userErrors {{ code }} {selection} }} }}"
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

fn item_globals(ids: &[&str]) -> Result<Vec<String>, JellyfinError> {
    ids.iter()
        .map(|jellyfin| {
            id::global_of_type(jellyfin, klapz_api::item::ITEM_TYPE).map_err(Into::into)
        })
        .collect()
}

#[derive(Deserialize)]
struct Placed {
    nodes: Vec<Option<PlacedItem>>,
}

#[derive(Deserialize)]
struct PlacedItem {
    #[serde(rename = "__typename")]
    type_name: String,
    library: Option<LibraryRef>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LibraryRef {
    pub id: String,
}

/// The official API's kind of an item, by its type.
fn kind_of(type_name: &str) -> &'static str {
    match type_name {
        "Movie" => "MOVIE",
        "Show" => "SHOW",
        "Season" => "SEASON",
        _ => "EPISODE",
    }
}

/// `/Collections?Name=&Ids=`: in the library of its first item, holding that item's kind.
async fn create(
    State(state): State<JellyfinState>,
    RawQuery(raw): RawQuery,
) -> Result<Response, JellyfinError> {
    let query = Query::parse(raw.as_deref());
    if query.flag("isLocked")? == Some(true) {
        return Err(JellyfinError::NotImplemented("a collection made locked"));
    }
    if query.get("parentId").is_some() {
        return Err(JellyfinError::NotImplemented(
            "a collection placed in a folder of its own",
        ));
    }
    let items = item_globals(&query.list("ids"))?;
    let Some(first) = items.first() else {
        return Err(JellyfinError::NotImplemented(
            "a collection with no item to place it in a library",
        ));
    };
    let placed: Placed = gql::query(
        &state.schema,
        "query($ids: [ID!]!) { nodes(ids: $ids) { __typename ... on Item { library { id } } } }",
        json!({ "ids": [first] }),
    )
    .await?;
    let first = placed
        .nodes
        .into_iter()
        .flatten()
        .next()
        .ok_or(JellyfinError::NotFound)?;
    let library = first.library.ok_or(JellyfinError::NotFound)?;
    let payload = mutate(
        &state,
        "createCollection",
        "CreateCollectionInput",
        json!({
            "libraryId": library.id,
            "title": query.get("name").unwrap_or_default(),
            "kind": kind_of(&first.type_name),
            "itemIds": items,
        }),
        "collection { id }",
    )
    .await?;
    let created = payload["collection"]["id"]
        .as_str()
        .and_then(id::of_global)
        .ok_or(JellyfinError::NotFound)?;
    Ok(json(&json!({ "Id": created })))
}

fn collection_global(jellyfin: &str) -> Result<String, JellyfinError> {
    Ok(id::global_of_type(
        jellyfin,
        klapz_api::collection::COLLECTION_TYPE,
    )?)
}

async fn add(
    State(state): State<JellyfinState>,
    Path(id): Path<String>,
    RawQuery(raw): RawQuery,
) -> Result<Response, JellyfinError> {
    let query = Query::parse(raw.as_deref());
    mutate(
        &state,
        "addToCollection",
        "AddToCollectionInput",
        json!({
            "collectionId": collection_global(&id)?,
            "itemIds": item_globals(&query.list("ids"))?,
        }),
        "",
    )
    .await?;
    Ok(no_content())
}

async fn remove(
    State(state): State<JellyfinState>,
    Path(id): Path<String>,
    RawQuery(raw): RawQuery,
) -> Result<Response, JellyfinError> {
    let query = Query::parse(raw.as_deref());
    let collection = collection_global(&id)?;
    for item in item_globals(&query.list("ids"))? {
        mutate(
            &state,
            "removeFromCollection",
            "RemoveFromCollectionInput",
            json!({ "collectionId": collection, "itemId": item }),
            "",
        )
        .await?;
    }
    Ok(no_content())
}

/// Deletes a collection: its items stay.
pub async fn delete(state: &JellyfinState, global: &str) -> Result<(), JellyfinError> {
    mutate(
        state,
        "deleteCollection",
        "DeleteCollectionInput",
        json!({ "id": global }),
        "",
    )
    .await?;
    Ok(())
}

#[derive(Deserialize)]
struct Memberships {
    node: Option<MembershipsOf>,
}

#[derive(Deserialize)]
struct MembershipsOf {
    collections: Option<Nodes<Collection>>,
}

/// `/Items/{id}/Collections`: the collections an item is in.
async fn item_collections(
    State(state): State<JellyfinState>,
    Path(id): Path<String>,
    RawQuery(raw): RawQuery,
) -> Result<Response, JellyfinError> {
    let query = Query::parse(raw.as_deref());
    gql::viewer(&state.schema)
        .await?
        .require_self(query.get("userId"))?;
    let global = id::global_of_type(&id, klapz_api::item::ITEM_TYPE)?;
    let data: Memberships = gql::query(
        &state.schema,
        &format!(
            r"query($id: ID!) {{ node(id: $id) {{
                ... on Movie {{ collections(first: 100) {{ nodes {{ ...JellyfinCollection }} }} }}
                ... on Show {{ collections(first: 100) {{ nodes {{ ...JellyfinCollection }} }} }}
                ... on Episode {{ collections(first: 100) {{ nodes {{ ...JellyfinCollection }} }} }}
            }} }}
            {COLLECTION_FIELDS}"
        ),
        json!({ "id": global }),
    )
    .await?;
    let found = data.node.ok_or(JellyfinError::NotFound)?;
    let fields = Fields::named(query.list("fields"));
    let items: Vec<Value> = found
        .collections
        .map(|collections| collections.nodes)
        .unwrap_or_default()
        .iter()
        .map(|collection| collection_dto(collection, &state.server.id, &fields))
        .collect();
    Ok(json(&ItemsResult {
        total_record_count: i64::try_from(items.len()).unwrap_or(i64::MAX),
        items,
        start_index: 0,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collection(order: &str) -> Collection {
        let watched = |is_watched| Watched {
            watch_state: Some(WatchedState { is_watched }),
        };
        Collection {
            id: klapz_api::GlobalId::encode("Collection", 7),
            title: "Jurassic".to_owned(),
            title_sort: None,
            content_rating: Some("FR-12".to_owned()),
            order: order.to_owned(),
            min_year: Some(1997),
            added_at: "2026-11-20T12:40:00Z".to_owned(),
            item_count: 2,
            library: None,
            items: Nodes {
                nodes: vec![watched(false), watched(false)],
            },
        }
    }

    #[test]
    fn a_collection_is_a_box_set_ordered_as_it_orders_its_items() {
        let dto = collection_dto(&collection("RELEASE_DATE"), "s", &Fields::named([]));
        assert_eq!(dto["Type"], "BoxSet");
        assert_eq!(dto["IsFolder"], true);
        assert_eq!(dto["DisplayOrder"], "PremiereDate");
        assert_eq!(dto["ProductionYear"], 1997);
        assert_eq!(dto["UserData"]["UnplayedItemCount"], 2);
        assert!(dto.get("ChildCount").is_none());
        let by_title = collection_dto(&collection("TITLE"), "s", &Fields::all());
        assert_eq!(by_title["DisplayOrder"], "SortName");
        assert_eq!(by_title["ChildCount"], 2);
        assert_eq!(by_title["CanDelete"], true);
    }
}
