//! Collections, bound onto the official API's: a library's collections, a collection's
//! items, creating, filling, ordering, emptying and deleting one.
//!
//! What a client names in a `uri` is expanded here into the items the official API is
//! handed, and a smart collection's library listing translated into that API's filter, as
//! for playlists. Everything kept, checked and counted is the official API's. The shapes are
//! the reference's, measured on its test libraries (`docs/spec-gaps.md`).
//!
//! The reference draws a collection's poster from its items' (`composite`); we publish the
//! address it publishes and draw nothing, as artwork is not served yet. The reference itself
//! answered `404` for every composite of its test libraries and of an imported collection.

use async_graphql::{Name, Value, Variables};
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post, put};
use axum::Router;
use open_plex_api::collection::COLLECTION_TYPE;
use open_plex_api::item::ITEM_TYPE;
use open_plex_api::library::LIBRARY_TYPE;
use open_plex_api::GlobalId;
use open_plex_core::page::MAX_ITEMS;
use serde::Deserialize;
use serde_json::{json, Map, Value as Json};

use crate::auth::Caller;
use crate::client::ClientIdentity;
use crate::error::ApiError;
use crate::gql;
use crate::gql_items::{database_id, with_shared_fragments, GqlItem, GqlLibraryRef};
use crate::items::{ItemContainer, ItemsContainer, MetadataEntry};
use crate::language::Language;
use crate::play_queues::{existing, expand, library_path};
use crate::response::{PlexResponse, ResponseFormat};
use crate::PlexState;

pub fn router() -> Router<PlexState> {
    Router::new()
        .route("/library/collections", post(create))
        .route("/library/collections/{collectionId}", get(lookup))
        .route(
            "/library/collections/{collectionId}/children",
            get(children),
        )
        .route(
            "/library/collections/{collectionId}/items",
            get(children).put(add),
        )
        // The spec removes an item with `PUT`, which the reference answers `404`: it
        // removes with `DELETE`, as Plex Web sends it.
        .route(
            "/library/collections/{collectionId}/items/{itemId}",
            put(not_found).delete(remove),
        )
        .route(
            "/library/collections/{collectionId}/items/{itemId}/move",
            put(move_item),
        )
        .route(
            "/library/collections/{collectionId}/composite/{updatedAt}",
            get(composite),
        )
        .route("/library/sections/{sectionId}/collections", get(list))
        .route(
            "/library/sections/{sectionId}/collection/{tagNumber}",
            axum::routing::delete(empty_by_tag),
        )
        .route("/library/metadata/{ids}/prefs", put(prefs))
}

/// What every answer about a collection reads.
const COLLECTION_FIELDS: &str = r"
fragment CollectionFields on Collection {
  id
  guid
  tagNumber
  title
  titleSort
  summary
  contentRating
  kind
  isSmart
  sourceUri
  mode
  order
  lockedFields
  addedAt
  updatedAt
  itemCount
  minYear
  maxYear
  library { id title uuid kind }
}
";

fn with_fields(query: &str) -> String {
    format!("{query}\n{COLLECTION_FIELDS}")
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GqlCollection {
    id: String,
    guid: String,
    tag_number: String,
    title: String,
    title_sort: Option<String>,
    summary: String,
    content_rating: Option<String>,
    kind: String,
    is_smart: bool,
    source_uri: Option<String>,
    mode: String,
    order: String,
    locked_fields: Vec<String>,
    added_at: i64,
    updated_at: i64,
    item_count: u64,
    min_year: Option<i32>,
    max_year: Option<i32>,
    library: Option<GqlLibraryRef>,
}

/// Which of the reference's shapes a collection is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Form {
    /// In a library's listing: the library is stated once, on the container.
    Listed,
    /// Answering its creation: as listed, with no container around it.
    Created,
    /// Answering a change of its items: with its library.
    Changed,
    /// Looked up: with its library and its locks.
    Lookup,
}

impl GqlCollection {
    fn database_id(&self) -> i64 {
        database_id(&self.id)
    }

    /// Plex's `type` number for what it holds.
    fn type_number(&self) -> i64 {
        match self.kind.as_str() {
            "SHOW" => 2,
            "SEASON" => 3,
            "EPISODE" => 4,
            _ => 1,
        }
    }

    /// The collection as the reference writes it, attributes in its order.
    pub(crate) fn metadata(&self, form: Form) -> Json {
        let key = self.database_id();
        let mut fields = Map::new();
        fields.insert("ratingKey".into(), json!(key.to_string()));
        fields.insert(
            "key".into(),
            json!(format!("/library/collections/{key}/children")),
        );
        fields.insert("guid".into(), json!(self.guid));
        fields.insert("type".into(), json!("collection"));
        fields.insert("title".into(), json!(self.title));
        if let Some(sort) = self.title_sort.as_ref().filter(|sort| **sort != self.title) {
            fields.insert("titleSort".into(), json!(sort));
        }
        if matches!(form, Form::Changed | Form::Lookup) {
            if let Some(library) = &self.library {
                fields.insert("librarySectionTitle".into(), json!(library.title));
                fields.insert("librarySectionID".into(), json!(library.database_id()));
                fields.insert(
                    "librarySectionKey".into(),
                    json!(format!("/library/sections/{}", library.database_id())),
                );
            }
        }
        if let Some(rating) = &self.content_rating {
            fields.insert("contentRating".into(), json!(rating));
        }
        fields.insert("subtype".into(), json!(self.kind.to_lowercase()));
        // Always written, empty included.
        fields.insert("summary".into(), json!(self.summary));
        if let Ok(number) = self.tag_number.parse::<i64>() {
            fields.insert("index".into(), json!(number));
        }
        if self.is_smart {
            if let Some(content) = &self.source_uri {
                fields.insert("content".into(), json!(content));
            }
        }
        // The reference stores the type the collection was created with here, plus 100 for a
        // smart one.
        let smart_offset = if self.is_smart { 100 } else { 0 };
        fields.insert(
            "ratingCount".into(),
            json!(self.type_number() + smart_offset),
        );
        fields.insert(
            "thumb".into(),
            json!(format!(
                "/library/collections/{key}/composite/{}?width=400&height=600",
                self.updated_at
            )),
        );
        fields.insert("addedAt".into(), json!(self.added_at));
        fields.insert("updatedAt".into(), json!(self.updated_at));
        fields.insert("childCount".into(), json!(self.item_count));
        if let Some(mode) = mode_number(&self.mode) {
            fields.insert("collectionMode".into(), json!(mode));
        }
        if let Some(order) = order_number(&self.order) {
            fields.insert("collectionSort".into(), json!(order));
        }
        // Years as strings, unlike an item's.
        if let Some(year) = self.max_year {
            fields.insert("maxYear".into(), json!(year.to_string()));
        }
        if let Some(year) = self.min_year {
            fields.insert("minYear".into(), json!(year.to_string()));
        }
        if self.is_smart {
            fields.insert("smart".into(), json!("1"));
        }
        if form == Form::Lookup {
            let locks: Vec<Json> = crate::edit::plex_locked_fields(&self.locked_fields)
                .map(|name| json!({ "locked": true, "name": name }))
                .collect();
            if !locks.is_empty() {
                fields.insert("Field".into(), Json::Array(locks));
            }
        }
        Json::Object(fields)
    }
}

/// `collectionMode`, written only once it is not the library's: the reference writes `-1`
/// too once a client set it, which storage does not tell apart from never set.
fn mode_number(mode: &str) -> Option<&'static str> {
    match mode {
        "HIDDEN" => Some("0"),
        "INSTEAD_OF_ITEMS" => Some("1"),
        "WITH_ITEMS" => Some("2"),
        _ => None,
    }
}

/// `collectionSort`, written once it is not the default, release date.
fn order_number(order: &str) -> Option<&'static str> {
    match order {
        "TITLE" => Some("1"),
        "CUSTOM" => Some("2"),
        _ => None,
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

fn collection_id(key: i64) -> Value {
    Value::String(GlobalId::encode(COLLECTION_TYPE, key))
}

fn item_id(key: i64) -> Value {
    Value::String(GlobalId::encode(ITEM_TYPE, key))
}

fn parse_key(key: &str) -> Result<i64, ApiError> {
    key.parse().map_err(|_| ApiError::NotFound)
}

fn param<'a>(params: &'a [(String, String)], name: &str) -> Option<&'a str> {
    params
        .iter()
        .rev()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

/// `X-Plex-Container-Start` and `-Size`, which page only together.
fn container_window(params: &[(String, String)]) -> Option<(u64, u32)> {
    let start = param(params, "X-Plex-Container-Start")?.parse().ok()?;
    let size: u32 = param(params, "X-Plex-Container-Size")?.parse().ok()?;
    Some((start, size.min(MAX_ITEMS)))
}

/// One collection in the response shape every change answers with.
fn single(collection: &GqlCollection, form: Form) -> Json {
    json!({ "size": 1, "Metadata": [collection.metadata(form)] })
}

#[derive(Debug, Deserialize)]
struct NodeData {
    node: Option<GqlCollection>,
}

/// The collection under this number, if the caller may read it; `404` otherwise.
async fn collection(state: &PlexState, key: i64) -> Result<GqlCollection, ApiError> {
    let query =
        with_fields("query PlexCollection($id: ID!) { node(id: $id) { ...CollectionFields } }");
    let data: NodeData = gql::execute(
        &state.schema,
        &query,
        variables([("id", collection_id(key))]),
    )
    .await?;
    data.node.ok_or(ApiError::NotFound)
}

#[derive(Debug, Deserialize)]
struct ByIdData {
    collections: Vec<Option<GqlCollection>>,
}

/// The collections among these numbers, in lookup form, with the number each answers: what
/// `/library/metadata/{ids}` shows for a number that names a collection.
pub(crate) async fn lookups(
    state: &PlexState,
    keys: &[i64],
) -> Result<Vec<(i64, Json, Option<GqlLibraryRef>)>, ApiError> {
    let query = with_fields(
        "query PlexCollections($ids: [ID!]!) { collections: nodes(ids: $ids) { ...CollectionFields } }",
    );
    let ids = Value::List(keys.iter().map(|key| collection_id(*key)).collect());
    let data: ByIdData = gql::execute(&state.schema, &query, variables([("ids", ids)])).await?;
    Ok(keys
        .iter()
        .zip(data.collections)
        .filter_map(|(key, found)| {
            let found = found?;
            Some((*key, found.metadata(Form::Lookup), found.library))
        })
        .collect())
}

/// The lookup container, as `/library/metadata/{ids}` writes it around anything it finds:
/// items and collections alike.
pub(crate) fn lookup_container(
    entries: Vec<Json>,
    library: Option<&GqlLibraryRef>,
) -> Result<Json, ApiError> {
    let container = ItemContainer::new(Vec::new(), library);
    let mut body =
        serde_json::to_value(container).map_err(|error| ApiError::Internal(Box::new(error)))?;
    if let Some(fields) = body.as_object_mut() {
        fields.insert("size".into(), json!(entries.len()));
        if !entries.is_empty() {
            fields.insert("Metadata".into(), Json::Array(entries));
        }
    }
    Ok(body)
}

/// `GET /library/collections/{collectionId}`: the collection as `/library/metadata` shows it.
async fn lookup(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(key): Path<String>,
    format: ResponseFormat,
) -> Result<PlexResponse<Json>, ApiError> {
    let key = parse_key(&key)?;
    let found = collection(&state, key).await?;
    let body = lookup_container(vec![found.metadata(Form::Lookup)], found.library.as_ref())?;
    Ok(PlexResponse::new(body, format))
}

#[derive(Debug, Deserialize)]
struct ListData {
    node: Option<GqlListedLibrary>,
}

#[derive(Debug, Deserialize)]
struct GqlListedLibrary {
    #[serde(flatten)]
    library: GqlLibraryRef,
    collections: GqlCollectionConnection,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlCollectionConnection {
    total_count: u64,
    edges: Vec<GqlCollectionEdge>,
}

#[derive(Debug, Deserialize)]
struct GqlCollectionEdge {
    node: GqlCollection,
}

/// The order a listing asks for, as the official API names it.
fn ordering(sort: Option<&str>) -> Result<(&'static str, &'static str), ApiError> {
    let Some(sort) = sort else {
        return Ok(("TITLE", "ASCENDING"));
    };
    let (field, suffix) = sort.split_once(':').unwrap_or((sort, "asc"));
    let order = match field {
        "titleSort" | "title" => "TITLE",
        "addedAt" => "ADDED_AT",
        "updatedAt" => "UPDATED_AT",
        // Refused like a listing's sort it cannot honor, rather than answered by title.
        _ => return Err(ApiError::NotImplemented),
    };
    let direction = if suffix == "desc" {
        "DESCENDING"
    } else {
        "ASCENDING"
    };
    Ok((order, direction))
}

/// `GET /library/sections/{sectionId}/collections`, and `/all?type=18`: a library's
/// collections, in the listing container of `/all`.
pub(crate) async fn listing(
    state: &PlexState,
    section: i64,
    params: &[(String, String)],
    format: ResponseFormat,
) -> Result<PlexResponse<Json>, ApiError> {
    let (order_by, direction) = ordering(param(params, "sort"))?;
    // Windowed like `/all`, whose container this is, with a whole page when the client
    // names none: a library holds far fewer collections than items.
    let (offset, size) = container_window(params).unwrap_or((0, MAX_ITEMS));
    let query = with_fields(
        r"
query PlexLibraryCollections(
  $id: ID!, $first: Int!, $offset: Int!, $orderBy: CollectionListOrder!, $direction: SortDirection!
) {
  node(id: $id) {
    ... on Library {
      id title uuid kind
      collections(first: $first, offset: $offset, orderBy: $orderBy, direction: $direction) {
        totalCount
        edges { node { ...CollectionFields } }
      }
    }
  }
}",
    );
    let data: ListData = gql::execute(
        &state.schema,
        &query,
        variables([
            ("id", Value::String(GlobalId::encode(LIBRARY_TYPE, section))),
            ("first", Value::Number(size.into())),
            ("offset", Value::Number(offset.into())),
            ("orderBy", Value::Enum(Name::new(order_by))),
            ("direction", Value::Enum(Name::new(direction))),
        ]),
    )
    .await?;
    let listed = data.node.ok_or(ApiError::NotFound)?;
    let total = listed.collections.total_count;
    let entries: Vec<Json> = listed
        .collections
        .edges
        .iter()
        .map(|edge| edge.node.metadata(Form::Listed))
        .collect();
    let container = ItemsContainer::new(&listed.library, Vec::new(), offset, total);
    let mut body =
        serde_json::to_value(container).map_err(|error| ApiError::Internal(Box::new(error)))?;
    if let Some(fields) = body.as_object_mut() {
        fields.insert("size".into(), json!(entries.len()));
        if !entries.is_empty() {
            fields.insert("Metadata".into(), Json::Array(entries));
        }
    }
    Ok(PlexResponse::new(body, format).paged(offset, total))
}

async fn list(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(section): Path<i64>,
    Query(params): Query<Vec<(String, String)>>,
    format: ResponseFormat,
) -> Result<PlexResponse<Json>, ApiError> {
    listing(&state, section, &params, format).await
}

const CHILDREN_QUERY: &str = r"
query PlexCollectionItems($id: ID!, $first: Int!, $offset: Int!) {
  node(id: $id) {
    ... on Collection {
      itemCount
      items(first: $first, offset: $offset) { edges { node { ...ListedItem } } }
    }
  }
}
";

static CHILDREN_QUERY_WITH_FRAGMENTS: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| with_shared_fragments(CHILDREN_QUERY));

#[derive(Debug, Deserialize)]
struct ChildrenData {
    node: Option<GqlChildren>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlChildren {
    item_count: u64,
    items: GqlItemConnection,
}

#[derive(Debug, Deserialize)]
struct GqlItemConnection {
    edges: Vec<GqlItemEdge>,
}

#[derive(Debug, Deserialize)]
struct GqlItemEdge {
    node: GqlItem,
}

/// `GET /library/collections/{collectionId}/children`, and `/items`, which answers the same:
/// its items in a listing's form, each naming its library.
async fn children(
    _caller: Caller,
    client: ClientIdentity,
    State(state): State<PlexState>,
    Path(key): Path<String>,
    Query(params): Query<Vec<(String, String)>>,
    format: ResponseFormat,
) -> Result<PlexResponse<Json>, ApiError> {
    let key = parse_key(&key)?;
    let language = Language::of(&client);
    let window = container_window(&params);
    let (offset, size) = window.unwrap_or((0, MAX_ITEMS));
    let data: ChildrenData = gql::execute(
        &state.schema,
        &CHILDREN_QUERY_WITH_FRAGMENTS,
        variables([
            ("id", collection_id(key)),
            ("first", Value::Number(size.into())),
            ("offset", Value::Number(offset.into())),
        ]),
    )
    .await?;
    let found = data.node.ok_or(ApiError::NotFound)?;
    let entries: Vec<Json> = found
        .items
        .edges
        .iter()
        .filter_map(|edge| MetadataEntry::from_graphql(&edge.node, true, language))
        .map(|entry| serde_json::to_value(entry).unwrap_or(Json::Null))
        .filter(|entry| !entry.is_null())
        .collect();
    let total = found.item_count;
    let mut container = Map::new();
    container.insert("size".into(), json!(entries.len()));
    // The reference states the window only when one was asked for; a collection longer than
    // a page says so whatever was asked, so that `size` never claims what the body lacks.
    let paged = window.is_some() || total > entries.len() as u64;
    if paged {
        container.insert("totalSize".into(), json!(total));
        container.insert("offset".into(), json!(offset));
    }
    if !entries.is_empty() {
        container.insert("Metadata".into(), Json::Array(entries));
    }
    let response = PlexResponse::new(Json::Object(container), format);
    Ok(if paged {
        response.paged(offset, total)
    } else {
        response
    })
}

/// A mutation's refusal, in the status the reference answers it with.
#[derive(Debug, Deserialize)]
struct GqlUserError {
    code: String,
    field: Vec<String>,
}

fn refused(errors: &[GqlUserError]) -> Option<ApiError> {
    errors.first().map(|error| match error.code.as_str() {
        "COLLECTION_NOT_FOUND" => ApiError::NotFound,
        "INVALID_RULE" => ApiError::NotImplemented,
        _ => ApiError::BadRequest,
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlPayload {
    collection: Option<GqlCollection>,
    user_errors: Vec<GqlUserError>,
}

/// Runs one collection mutation that answers `{ collection, userErrors }`, under `field`.
async fn mutate(
    state: &PlexState,
    field: &str,
    input_type: &str,
    variables: Variables,
) -> Result<GqlPayload, ApiError> {
    let query = with_fields(&format!(
        "mutation PlexCollectionChange($input: {input_type}!) {{ \
           {field}(input: $input) {{ collection {{ ...CollectionFields }} userErrors {{ code field }} }} \
         }}"
    ));
    let mut data: Map<String, Json> = gql::execute(&state.schema, &query, variables).await?;
    let payload = data.remove(field).ok_or(ApiError::NotFound)?;
    serde_json::from_value(payload).map_err(|error| ApiError::Internal(Box::new(error)))
}

impl GqlPayload {
    fn into_collection(self) -> Result<GqlCollection, ApiError> {
        if let Some(error) = refused(&self.user_errors) {
            return Err(error);
        }
        self.collection.ok_or(ApiError::NotFound)
    }
}

/// The items a `uri` names, as global ids: the items themselves for `/library/metadata`, the
/// listing's items for a library path. Another server's names none.
async fn named_items(state: &PlexState, uri: &str) -> Result<Option<Vec<String>>, ApiError> {
    let machine = gql::machine_identifier(&state.identity.machine_identifier);
    let Some(path) = library_path(uri, &machine) else {
        return Ok(None);
    };
    if let Some(keys) = path.strip_prefix("/library/metadata/") {
        let keys: Vec<i64> = keys
            .split(',')
            .filter_map(|key| key.parse().ok())
            .take(MAX_ITEMS as usize)
            .collect();
        // A series stands for itself here, where a play queue plays its episodes.
        return Ok(Some(existing(state, keys).await?));
    }
    // `continuous` is a player's wish, not a collection's.
    Ok(Some(expand(state, path, false).await?))
}

/// The kind a collection holds, from Plex's `type` number; absent, its library's top level.
fn kind_of(type_number: Option<&str>, library_kind: &str) -> Result<&'static str, ApiError> {
    Ok(match type_number {
        None => match library_kind {
            "SHOW" => "SHOW",
            "MOVIE" => "MOVIE",
            _ => return Err(ApiError::BadRequest),
        },
        Some("1") => "MOVIE",
        Some("2") => "SHOW",
        Some("3") => "SEASON",
        Some("4") => "EPISODE",
        // Music is held nowhere here; the official API refuses the kind.
        Some("8") => "ARTIST",
        Some("9") => "ALBUM",
        Some("10") => "TRACK",
        // The reference makes a collection of an `unknown` subtype; nothing could join it.
        Some(_) => return Err(ApiError::BadRequest),
    })
}

#[derive(Debug, Deserialize)]
struct LibraryKindData {
    node: Option<GqlLibraryKind>,
}

#[derive(Debug, Deserialize)]
struct GqlLibraryKind {
    kind: String,
}

/// `POST /library/collections`: a collection of the items a `uri` names, or a smart one of
/// the listing it names.
async fn create(
    _caller: Caller,
    State(state): State<PlexState>,
    Query(params): Query<Vec<(String, String)>>,
    format: ResponseFormat,
) -> Result<PlexResponse<Json>, ApiError> {
    let section: i64 = param(&params, "sectionId")
        .and_then(|id| id.parse().ok())
        .ok_or(ApiError::BadRequest)?;
    let title = param(&params, "title")
        .filter(|title| !title.is_empty())
        .ok_or(ApiError::BadRequest)?;
    let library = GlobalId::encode(LIBRARY_TYPE, section);
    let found: LibraryKindData = gql::execute(
        &state.schema,
        "query PlexCollectionLibrary($id: ID!) { node(id: $id) { ... on Library { kind } } }",
        variables([("id", Value::String(library.clone()))]),
    )
    .await?;
    let library_kind = found.node.ok_or(ApiError::BadRequest)?.kind;
    let kind = kind_of(param(&params, "type"), &library_kind)?;
    let smart = param(&params, "smart") == Some("1");
    let uri = param(&params, "uri");

    let mut fields = vec![
        ("libraryId", Value::String(library)),
        ("title", Value::String(title.to_owned())),
        ("kind", Value::Enum(Name::new(kind))),
    ];
    if smart {
        let uri = uri.ok_or(ApiError::BadRequest)?;
        let machine = gql::machine_identifier(&state.identity.machine_identifier);
        let path = library_path(uri, &machine).ok_or(ApiError::BadRequest)?;
        let (listed, rule) = crate::playlists::listing_rule(path)?;
        // A smart collection lists its own library.
        if listed != section {
            return Err(ApiError::BadRequest);
        }
        fields.push(("rule", rule));
        fields.push(("sourceUri", Value::String(uri.to_owned())));
    } else if let Some(uri) = uri {
        // What names nothing here makes an empty collection, as on the reference.
        let items = named_items(&state, uri).await?.unwrap_or_default();
        fields.push((
            "itemIds",
            Value::List(items.into_iter().map(Value::String).collect()),
        ));
    }
    let created = mutate(
        &state,
        "createCollection",
        "CreateCollectionInput",
        input(fields),
    )
    .await?
    .into_collection()?;
    Ok(PlexResponse::new(single(&created, Form::Created), format))
}

/// `PUT /library/collections/{collectionId}/items?uri=`: the items a `uri` names, those of
/// the collection's kind and library, added at its end.
async fn add(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(key): Path<String>,
    Query(params): Query<Vec<(String, String)>>,
    format: ResponseFormat,
) -> Result<PlexResponse<Json>, ApiError> {
    let key = parse_key(&key)?;
    let uri = param(&params, "uri").ok_or(ApiError::BadRequest)?;
    let items = named_items(&state, uri)
        .await?
        .ok_or(ApiError::BadRequest)?;
    let changed = mutate(
        &state,
        "addToCollection",
        "AddToCollectionInput",
        input(vec![
            ("collectionId", collection_id(key)),
            (
                "itemIds",
                Value::List(items.into_iter().map(Value::String).collect()),
            ),
        ]),
    )
    .await?
    .into_collection()?;
    Ok(PlexResponse::new(single(&changed, Form::Changed), format))
}

/// `DELETE /library/collections/{collectionId}/items/{itemId}`. An item that is not in it
/// changes nothing and answers the collection all the same.
async fn remove(
    _caller: Caller,
    State(state): State<PlexState>,
    Path((key, item)): Path<(String, String)>,
    format: ResponseFormat,
) -> Result<PlexResponse<Json>, ApiError> {
    let key = parse_key(&key)?;
    let item: i64 = item.parse().map_err(|_| ApiError::BadRequest)?;
    let changed = mutate(
        &state,
        "removeFromCollection",
        "RemoveFromCollectionInput",
        input(vec![
            ("collectionId", collection_id(key)),
            ("itemId", item_id(item)),
        ]),
    )
    .await?
    .into_collection()?;
    Ok(PlexResponse::new(single(&changed, Form::Changed), format))
}

async fn not_found(_caller: Caller) -> ApiError {
    ApiError::NotFound
}

/// `PUT /library/collections/{collectionId}/items/{itemId}/move?after=`. Measured on the
/// reference: an item after itself, an item not in the collection and a non-numeric `after`
/// are `400`s, and an `after` that is not in it moves the item first.
async fn move_item(
    _caller: Caller,
    State(state): State<PlexState>,
    Path((key, item)): Path<(String, String)>,
    Query(params): Query<Vec<(String, String)>>,
    format: ResponseFormat,
) -> Result<PlexResponse<Json>, ApiError> {
    let key = parse_key(&key)?;
    let item: i64 = item.parse().map_err(|_| ApiError::BadRequest)?;
    let after = param(&params, "after")
        .map(|after| after.parse::<i64>().map_err(|_| ApiError::BadRequest))
        .transpose()?;
    let moved = |after: Option<i64>| {
        let mut fields = vec![
            ("collectionId", collection_id(key)),
            ("itemId", item_id(item)),
        ];
        if let Some(after) = after {
            fields.push(("afterItemId", item_id(after)));
        }
        input(fields)
    };
    let mut payload = mutate(
        &state,
        "moveCollectionItem",
        "MoveCollectionItemInput",
        moved(after),
    )
    .await?;
    let after_is_missing = payload.user_errors.first().is_some_and(|error| {
        error.code == "NOT_IN_COLLECTION" && error.field.last().is_some_and(|f| f == "afterItemId")
    });
    if after_is_missing && after != Some(item) {
        payload = mutate(
            &state,
            "moveCollectionItem",
            "MoveCollectionItemInput",
            moved(None),
        )
        .await?;
    }
    let changed = payload.into_collection()?;
    Ok(PlexResponse::new(single(&changed, Form::Changed), format))
}

/// `GET /library/collections/{collectionId}/composite/{updatedAt}`: no image is drawn here,
/// and the reference answered `404` for every one asked of it.
async fn composite(_caller: Caller) -> ApiError {
    ApiError::NotFound
}

#[derive(Debug, Deserialize)]
struct TagData {
    library: Option<GqlTagLibrary>,
    everywhere: GqlTagConnection,
}

#[derive(Debug, Deserialize)]
struct GqlTagLibrary {
    collections: GqlTagConnection,
}

#[derive(Debug, Deserialize)]
struct GqlTagConnection {
    edges: Vec<GqlTagEdge>,
}

#[derive(Debug, Deserialize)]
struct GqlTagEdge {
    node: GqlTagged,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlTagged {
    id: String,
    is_smart: bool,
}

/// `DELETE /library/sections/{sectionId}/collection/{tagNumber}`: what the reference does
/// with it, measured, is take every item of that library out of the collection carried under
/// that number, and keep the collection. A number some other library carries answers `200`
/// and changes nothing; one no library carries, `404`.
async fn empty_by_tag(
    _caller: Caller,
    State(state): State<PlexState>,
    Path((section, number)): Path<(i64, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let number: i64 = number.parse().map_err(|_| ApiError::NotFound)?;
    let query = r"
query PlexCollectionTag($library: ID!, $number: String!) {
  library: node(id: $library) {
    ... on Library { collections(tagNumber: $number, first: 1) { edges { node { id isSmart } } } }
  }
  everywhere: collections(tagNumber: $number, first: 1) { edges { node { id isSmart } } }
}";
    let data: TagData = gql::execute(
        &state.schema,
        query,
        variables([
            (
                "library",
                Value::String(GlobalId::encode(LIBRARY_TYPE, section)),
            ),
            ("number", Value::String(number.to_string())),
        ]),
    )
    .await?;
    let library = data.library.ok_or(ApiError::NotFound)?;
    if data.everywhere.edges.is_empty() {
        return Err(ApiError::NotFound);
    }
    if let Some(edge) = library.collections.edges.first() {
        if !edge.node.is_smart {
            mutate(
                &state,
                "clearCollection",
                "ClearCollectionInput",
                input(vec![("id", Value::String(edge.node.id.clone()))]),
            )
            .await?
            .into_collection()?;
        }
    }
    Ok((StatusCode::OK, [(header::CONTENT_TYPE, "text/html")]))
}

/// Which of the two a number names; only presence is read.
#[derive(Debug, Deserialize)]
struct KindData {
    collection: Option<serde::de::IgnoredAny>,
    item: Option<serde::de::IgnoredAny>,
}

const KIND_QUERY: &str = r"
query PlexWhatIs($collection: ID!, $item: ID!) {
  collection: node(id: $collection) { id }
  item: node(id: $item) { id }
}";

/// Whether a number names a collection the caller reads, an item, or nothing.
async fn collection_or_item(state: &PlexState, key: i64) -> Result<Option<bool>, ApiError> {
    let data: KindData = gql::execute(
        &state.schema,
        KIND_QUERY,
        variables([("collection", collection_id(key)), ("item", item_id(key))]),
    )
    .await?;
    Ok(match (data.collection, data.item) {
        (Some(_), _) => Some(true),
        (None, Some(_)) => Some(false),
        (None, None) => None,
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlDeletePayload {
    deleted_collection_id: Option<String>,
    user_errors: Vec<GqlUserError>,
}

/// `DELETE /library/metadata/{ids}`, which is how Plex Web deletes a collection. Deleting an
/// item is the scanner's business, and stays `501`.
pub(crate) async fn delete_metadata(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(ids): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let key: i64 = ids.parse().map_err(|_| ApiError::NotImplemented)?;
    match collection_or_item(&state, key).await? {
        Some(true) => {}
        Some(false) => return Err(ApiError::NotImplemented),
        None => return Err(ApiError::NotFound),
    }
    let query = "mutation PlexDeleteCollection($input: DeleteCollectionInput!) { \
                 deleteCollection(input: $input) { deletedCollectionId userErrors { code field } } }";
    let mut data: Map<String, Json> = gql::execute(
        &state.schema,
        query,
        input(vec![("id", collection_id(key))]),
    )
    .await?;
    let payload: GqlDeletePayload =
        serde_json::from_value(data.remove("deleteCollection").ok_or(ApiError::NotFound)?)
            .map_err(|error| ApiError::Internal(Box::new(error)))?;
    if let Some(error) = refused(&payload.user_errors) {
        return Err(error);
    }
    payload.deleted_collection_id.ok_or(ApiError::NotFound)?;
    Ok((StatusCode::OK, [(header::CONTENT_TYPE, "text/html")]))
}

/// `PUT /library/metadata/{ids}/prefs`: a collection's `collectionMode` and `collectionSort`.
/// Measured: `-1` to `2` for the mode, `0` to `2` for the order, an empty `200`, and any other
/// name a `400`; the reference keeps an order of `7` as well, which is refused here. An item's
/// preferences stay `501`.
async fn prefs(
    _caller: Caller,
    State(state): State<PlexState>,
    Path(ids): Path<String>,
    Query(params): Query<Vec<(String, String)>>,
) -> Result<impl IntoResponse, ApiError> {
    let key: i64 = ids.parse().map_err(|_| ApiError::NotImplemented)?;
    match collection_or_item(&state, key).await? {
        Some(true) => {}
        Some(false) => return Err(ApiError::NotImplemented),
        None => return Err(ApiError::NotFound),
    }
    let mut fields = vec![("id", collection_id(key))];
    for (name, value) in &params {
        if name.starts_with("X-Plex-") {
            continue;
        }
        let setting = match (name.as_str(), value.as_str()) {
            ("collectionMode", "-1") => ("mode", "LIBRARY_DEFAULT"),
            ("collectionMode", "0") => ("mode", "HIDDEN"),
            ("collectionMode", "1") => ("mode", "INSTEAD_OF_ITEMS"),
            ("collectionMode", "2") => ("mode", "WITH_ITEMS"),
            ("collectionSort", "0") => ("order", "RELEASE_DATE"),
            ("collectionSort", "1") => ("order", "TITLE"),
            ("collectionSort", "2") => ("order", "CUSTOM"),
            _ => return Err(ApiError::BadRequest),
        };
        fields.push((setting.0, Value::Enum(Name::new(setting.1))));
    }
    mutate(
        &state,
        "updateCollection",
        "UpdateCollectionInput",
        input(fields),
    )
    .await?
    .into_collection()?;
    Ok((StatusCode::OK, [(header::CONTENT_TYPE, "text/html")]))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn captured() -> GqlCollection {
        GqlCollection {
            id: GlobalId::encode(COLLECTION_TYPE, 578_398),
            guid: "collection://3817377a-620b-4fa6-ab64-372853883318".to_owned(),
            tag_number: "987809".to_owned(),
            title: "OP Coll D".to_owned(),
            title_sort: Some("OP Coll D".to_owned()),
            summary: String::new(),
            content_rating: None,
            kind: "EPISODE".to_owned(),
            is_smart: false,
            source_uri: None,
            mode: "LIBRARY_DEFAULT".to_owned(),
            order: "CUSTOM".to_owned(),
            locked_fields: Vec::new(),
            added_at: 1_794_406_159,
            updated_at: 1_794_406_160,
            item_count: 2,
            min_year: Some(2021),
            max_year: Some(2021),
            library: Some(GqlLibraryRef {
                id: GlobalId::encode(LIBRARY_TYPE, 28),
                title: "open-plex tests series".to_owned(),
                uuid: "ba1c46de-199e-4464-8efd-aa7d9c374421".to_owned(),
                kind: "SHOW".to_owned(),
            }),
        }
    }

    #[test]
    fn writes_a_changed_collection_as_the_reference_does() {
        // `PUT /library/collections/578398/items/577990/move` on the reference, less
        // `collectionMode="-1"`, which it writes once a client set it.
        let written = captured().metadata(Form::Changed);
        let keys: Vec<&str> = written
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            [
                "ratingKey",
                "key",
                "guid",
                "type",
                "title",
                "librarySectionTitle",
                "librarySectionID",
                "librarySectionKey",
                "subtype",
                "summary",
                "index",
                "ratingCount",
                "thumb",
                "addedAt",
                "updatedAt",
                "childCount",
                "collectionSort",
                "maxYear",
                "minYear"
            ]
        );
        assert_eq!(written["ratingCount"], 4);
        assert_eq!(written["subtype"], "episode");
        assert_eq!(written["index"], 987_809);
        assert_eq!(written["collectionSort"], "2");
        assert_eq!(
            written["thumb"],
            "/library/collections/578398/composite/1794406160?width=400&height=600"
        );
    }

    #[test]
    fn a_smart_collection_carries_its_content_and_counts_a_hundred_more() {
        let mut smart = captured();
        smart.is_smart = true;
        smart.source_uri = Some(
            "server://x/com.plexapp.plugins.library/library/sections/28/all?type=4".to_owned(),
        );
        let written = smart.metadata(Form::Created);
        assert_eq!(written["ratingCount"], 104);
        assert_eq!(written["smart"], "1");
        assert!(written.get("content").is_some());
        assert!(written.get("librarySectionID").is_none());
    }

    #[test]
    fn a_type_number_names_a_kind_and_none_takes_the_librarys() {
        assert_eq!(kind_of(Some("4"), "SHOW").unwrap(), "EPISODE");
        assert_eq!(kind_of(None, "SHOW").unwrap(), "SHOW");
        assert_eq!(kind_of(None, "MOVIE").unwrap(), "MOVIE");
        assert!(kind_of(Some("99"), "MOVIE").is_err());
    }

    #[test]
    fn sorts_a_listing_by_title_or_date_and_refuses_the_rest() {
        assert_eq!(ordering(None).unwrap(), ("TITLE", "ASCENDING"));
        assert_eq!(
            ordering(Some("addedAt:desc")).unwrap(),
            ("ADDED_AT", "DESCENDING")
        );
        assert!(matches!(
            ordering(Some("rating")),
            Err(ApiError::NotImplemented)
        ));
    }
}
