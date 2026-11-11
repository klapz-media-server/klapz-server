//! Collections on the official API.
//!
//! A collection is its library's: whoever may read the library reads the collection and
//! everything in it, and asking for one in another library answers as asking for nothing.
//! Its library's admins and the operator change it, as they edit the library's items.
//!
//! A smart collection keeps a listing of its library — filter, order, direction — in the
//! shape `Library.items` takes it, and lists it anew on every read.

// Resolvers are `async` even when they only read a field: async-graphql requires that shape,
// and `unused_async` does not know it.
#![allow(clippy::unused_async)]

use std::collections::HashMap;
use std::sync::Arc;

use async_graphql::connection::{query, Connection, Edge, EmptyFields};
use async_graphql::dataloader::{DataLoader, Loader};
use async_graphql::{
    Context, Enum, InputObject, InputType, Object, Result, SimpleObject, Value, ID,
};
use open_plex_core::collection::{
    Collection, CollectionCondition, CollectionEdit, CollectionError, CollectionField,
    CollectionMode, CollectionOrder, CollectionQuery, CollectionRef, CollectionRepository,
    CollectionSort, CollectionTotals, Destination, NewCollection,
};
use open_plex_core::metadata::{ItemDepth, ItemFilter, ItemKind, ItemQuery, ItemSort};
use open_plex_core::page::{Direction, MAX_ITEMS};
use open_plex_core::watch::Viewer;
use open_plex_core::{now_epoch, CollectionId, MetadataItem, RatingKey, SectionId};

use crate::cursor::OpaqueCursor;
use crate::filter::{ItemFilterInput, ItemOrder, ItemType, SortDirection};
use crate::item::{Item, ItemConnection, ITEM_TYPE};
use crate::library::{Library, LIBRARY_TYPE};
use crate::loader::ItemLoader;
use crate::node::GlobalId;
use crate::schema::{self, ApiContext};
use crate::DateTime;

pub const COLLECTION_TYPE: &str = "Collection";
const DEFAULT_PAGE_SIZE: u32 = 50;

pub type CollectionConnection =
    Connection<OpaqueCursor, CollectionObject, CollectionTotal, EmptyFields>;
/// The collections an item is in.
pub type MembershipConnection = Connection<
    OpaqueCursor,
    CollectionObject,
    EmptyFields,
    EmptyFields,
    MembershipConnectionName,
    MembershipEdgeName,
>;

/// Names the connection of an item's collections apart from a listing's, whose total it
/// does not carry.
#[derive(Debug)]
pub struct MembershipConnectionName;

impl async_graphql::connection::ConnectionNameType for MembershipConnectionName {
    fn type_name<T: async_graphql::OutputType>() -> String {
        "CollectionMembershipConnection".to_owned()
    }
}

/// Names the edges of [`MembershipConnection`].
#[derive(Debug)]
pub struct MembershipEdgeName;

impl async_graphql::connection::EdgeNameType for MembershipEdgeName {
    fn type_name<T: async_graphql::OutputType>() -> String {
        "CollectionMembershipEdge".to_owned()
    }
}

fn collections<'c>(context: &Context<'c>) -> Result<&'c dyn CollectionRepository> {
    Ok(context.data::<ApiContext>()?.collections.as_ref())
}

/// How a library page treats a collection and its items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
#[graphql(name = "CollectionMode")]
pub enum CollectionModeType {
    /// As the library's own setting says.
    LibraryDefault,
    /// The collection is not shown; its items are.
    Hidden,
    /// The collection is shown in place of its items.
    InsteadOfItems,
    /// The collection is shown, and its items too.
    WithItems,
}

impl From<CollectionMode> for CollectionModeType {
    fn from(mode: CollectionMode) -> Self {
        match mode {
            CollectionMode::LibraryDefault => Self::LibraryDefault,
            CollectionMode::Hidden => Self::Hidden,
            CollectionMode::InsteadOfItems => Self::InsteadOfItems,
            CollectionMode::WithItems => Self::WithItems,
        }
    }
}

impl From<CollectionModeType> for CollectionMode {
    fn from(mode: CollectionModeType) -> Self {
        match mode {
            CollectionModeType::LibraryDefault => Self::LibraryDefault,
            CollectionModeType::Hidden => Self::Hidden,
            CollectionModeType::InsteadOfItems => Self::InsteadOfItems,
            CollectionModeType::WithItems => Self::WithItems,
        }
    }
}

/// How a collection of chosen items orders them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
#[graphql(name = "CollectionOrder")]
pub enum CollectionOrderType {
    /// By release date, items without one first.
    ReleaseDate,
    /// By sort title.
    Title,
    /// As an editor arranged them with `moveCollectionItem`.
    Custom,
}

impl From<CollectionOrder> for CollectionOrderType {
    fn from(order: CollectionOrder) -> Self {
        match order {
            CollectionOrder::ReleaseDate => Self::ReleaseDate,
            CollectionOrder::Title => Self::Title,
            CollectionOrder::Custom => Self::Custom,
        }
    }
}

impl From<CollectionOrderType> for CollectionOrder {
    fn from(order: CollectionOrderType) -> Self {
        match order {
            CollectionOrderType::ReleaseDate => Self::ReleaseDate,
            CollectionOrderType::Title => Self::Title,
            CollectionOrderType::Custom => Self::Custom,
        }
    }
}

/// A collection field an editor can lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
#[graphql(name = "CollectionField")]
pub enum CollectionFieldType {
    /// `title`.
    Title,
    /// `titleSort`.
    TitleSort,
    /// `summary`.
    Summary,
    /// `contentRating`.
    ContentRating,
    /// The poster.
    Poster,
    /// The background.
    Background,
}

impl From<CollectionField> for CollectionFieldType {
    fn from(field: CollectionField) -> Self {
        match field {
            CollectionField::Title => Self::Title,
            CollectionField::TitleSort => Self::TitleSort,
            CollectionField::Summary => Self::Summary,
            CollectionField::ContentRating => Self::ContentRating,
            CollectionField::Poster => Self::Poster,
            CollectionField::Background => Self::Background,
        }
    }
}

impl From<CollectionFieldType> for CollectionField {
    fn from(field: CollectionFieldType) -> Self {
        match field {
            CollectionFieldType::Title => Self::Title,
            CollectionFieldType::TitleSort => Self::TitleSort,
            CollectionFieldType::Summary => Self::Summary,
            CollectionFieldType::ContentRating => Self::ContentRating,
            CollectionFieldType::Poster => Self::Poster,
            CollectionFieldType::Background => Self::Background,
        }
    }
}

/// How a list of collections is ordered. Equal values follow by sort title, ascending.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Enum)]
pub enum CollectionListOrder {
    /// By sort title, case-insensitively.
    #[default]
    Title,
    /// By when the collection was created.
    AddedAt,
    /// By when an item last joined or left it.
    UpdatedAt,
}

impl From<CollectionListOrder> for CollectionSort {
    fn from(order: CollectionListOrder) -> Self {
        match order {
            CollectionListOrder::Title => Self::Title,
            CollectionListOrder::AddedAt => Self::AddedAt,
            CollectionListOrder::UpdatedAt => Self::UpdatedAt,
        }
    }
}

/// What a smart collection lists: its library's items, as `Library.items` lists them.
#[derive(Debug, Clone, Default, InputObject)]
pub struct SmartCollectionRuleInput {
    /// Narrows the listing, as `Library.items` takes it.
    pub filter: Option<ItemFilterInput>,
    /// How to order it. Omitted, by title.
    pub order_by: Option<ItemOrder>,
    /// Which direction to order in. Omitted, ascending.
    pub direction: Option<SortDirection>,
}

impl SmartCollectionRuleInput {
    /// The rule as stored: this input, in its own JSON shape.
    fn stored(&self) -> Result<String> {
        Ok(serde_json::to_string(&self.to_value())?)
    }

    fn read(listing: &str) -> Result<Self> {
        let value: Value = serde_json::from_str(listing)?;
        // Only this module writes the rule, so one that no longer parses was stored by an
        // older schema.
        Self::parse(Some(value))
            .map_err(|_| async_graphql::Error::new("a stored rule no longer reads as a rule"))
    }
}

/// A smart collection's rule.
#[derive(Debug, Clone)]
pub struct SmartCollectionRule(SmartCollectionRuleInput);

#[Object]
impl SmartCollectionRule {
    /// How the listing is ordered.
    async fn order_by(&self) -> ItemOrder {
        self.0.order_by.unwrap_or_default()
    }

    /// Which direction it is ordered in.
    async fn direction(&self) -> SortDirection {
        self.0.direction.unwrap_or_default()
    }
}

/// How many collections a listing holds, past the page.
#[derive(Debug, Clone)]
pub struct CollectionTotal {
    query: Option<CollectionQuery>,
}

#[Object]
impl CollectionTotal {
    /// How many collections the listing holds, every page together.
    async fn total_count(&self, context: &Context<'_>) -> Result<u64> {
        match &self.query {
            Some(query) => Ok(collections(context)?.count(query).await?),
            None => Ok(0),
        }
    }
}

/// Items of one library grouped under a name: chosen one by one, or found by a rule.
#[derive(Debug, Clone)]
pub struct CollectionObject(pub Collection);

#[Object(name = "Collection")]
impl CollectionObject {
    /// Globally unique, opaque identifier. Pass it to `node` to refetch this collection.
    pub async fn id(&self) -> ID {
        ID(GlobalId::encode(COLLECTION_TYPE, self.0.id.get()))
    }

    /// The server-local number, shared with the items' numbering, which the Plex surface
    /// publishes as `ratingKey`.
    async fn database_id(&self) -> String {
        self.0.id.get().to_string()
    }

    /// An identifier unique across servers.
    async fn guid(&self) -> &str {
        &self.0.guid
    }

    /// The number its items carry it under, which the Plex surface filters on
    /// (`collection=…`). Shared by collections of one name in different libraries, as the
    /// reference numbers them; unique within a library.
    async fn tag_number(&self) -> String {
        self.0.tag_number.to_string()
    }

    /// Its name.
    async fn title(&self) -> &str {
        &self.0.title
    }

    /// The title to sort by. Null when none is stored.
    async fn title_sort(&self) -> Option<&str> {
        self.0.title_sort.as_deref()
    }

    /// Its description. Empty when it has none.
    async fn summary(&self) -> &str {
        &self.0.summary
    }

    /// An age-rating label, such as `fr/12`, as the metadata source gave it.
    async fn content_rating(&self) -> Option<&str> {
        self.0.content_rating.as_deref()
    }

    /// The kind of item it holds: every item in it is of this kind.
    async fn kind(&self) -> ItemType {
        self.0.kind.into()
    }

    /// Whether it lists what a rule finds rather than items chosen one by one.
    async fn is_smart(&self) -> bool {
        self.0.is_smart()
    }

    /// The rule of a smart collection. Null for one of chosen items.
    async fn rule(&self) -> Result<Option<SmartCollectionRule>> {
        self.0
            .rule
            .as_deref()
            .map(|rule| SmartCollectionRuleInput::read(rule).map(SmartCollectionRule))
            .transpose()
    }

    /// Where a smart collection's content came from, as the client that created it named
    /// it. Null when it named none.
    async fn source_uri(&self) -> Option<&str> {
        self.0.source_uri.as_deref()
    }

    /// How a library page treats it and its items. Kept for clients; nothing on the server
    /// lists differently for it.
    async fn mode(&self) -> CollectionModeType {
        self.0.mode.into()
    }

    /// How its chosen items are ordered in `items`. A smart collection follows its rule.
    async fn order(&self) -> CollectionOrderType {
        self.0.order.into()
    }

    /// The fields an editor locked.
    async fn locked_fields(&self) -> Vec<CollectionFieldType> {
        self.0.locked.iter().copied().map(Into::into).collect()
    }

    /// A poster, hosted by the metadata provider. Null when it has none there.
    async fn thumb_url(&self) -> Option<&str> {
        self.0.thumb_url.as_deref()
    }

    /// A background, hosted by the metadata provider. Null when it has none there.
    async fn art_url(&self) -> Option<&str> {
        self.0.art_url.as_deref()
    }

    /// When it was created.
    async fn added_at(&self) -> DateTime {
        self.0.added_at.into()
    }

    /// When an item last joined or left it. An edit of its
    /// fields leaves it.
    async fn updated_at(&self) -> DateTime {
        self.0.updated_at.into()
    }

    /// The library it belongs to.
    async fn library(&self, context: &Context<'_>) -> Result<Option<Library>> {
        schema::load_library(context, self.0.section).await
    }

    /// How many items it holds: the chosen ones, or what the rule finds now.
    async fn item_count(&self, context: &Context<'_>) -> Result<u64> {
        match &self.0.rule {
            Some(rule) => {
                let query = smart_query(context, &self.0, rule)?;
                Ok(schema::metadata(context)?.item_count(&query).await?)
            }
            None => Ok(totals(context, self.0.id).await?.items),
        }
    }

    /// The earliest release year among its chosen items. Null for a smart collection, or
    /// when no item has a year.
    async fn min_year(&self, context: &Context<'_>) -> Result<Option<i32>> {
        if self.0.is_smart() {
            return Ok(None);
        }
        Ok(totals(context, self.0.id).await?.min_year)
    }

    /// The latest release year among its chosen items. Null for a smart collection, or when
    /// no item has a year.
    async fn max_year(&self, context: &Context<'_>) -> Result<Option<i32>> {
        if self.0.is_smart() {
            return Ok(None);
        }
        Ok(totals(context, self.0.id).await?.max_year)
    }

    /// Its items: the chosen ones in its `order`, or what the rule finds now.
    async fn items(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Return items after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return items before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many items from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many items from the end of the range.")] last: Option<
            i32,
        >,
        #[graphql(desc = "Skip this many items first, to land in the middle of the collection.")]
        offset: Option<u32>,
    ) -> Result<ItemConnection> {
        let skip = u64::from(offset.unwrap_or(0));
        let collection = &self.0;
        query(
            after,
            before,
            first,
            last,
            |after, before, first, last| async move {
                let window = schema::window_for(after, before, first, last, DEFAULT_PAGE_SIZE)?
                    .skipping(skip);
                let mut connection = Connection::new(false, false);
                if collection.rule.is_none() && collection.order == CollectionOrder::Custom {
                    let page = collections(context)?.entries(collection.id, window).await?;
                    connection.has_previous_page = page.has_previous;
                    connection.has_next_page = page.has_next;
                    let keys: Vec<RatingKey> = page.items.iter().map(|entry| entry.item).collect();
                    let mut loaded = context
                        .data::<DataLoader<ItemLoader>>()?
                        .load_many(keys)
                        .await?;
                    connection
                        .edges
                        .extend(page.items.iter().filter_map(|entry| {
                            loaded.remove(&entry.item).map(|item| {
                                Edge::new(OpaqueCursor(entry.cursor()), Item::from_domain(item))
                            })
                        }));
                    return Ok::<_, async_graphql::Error>(connection);
                }
                let listing = match &collection.rule {
                    Some(rule) => smart_query(context, collection, rule)?,
                    None => chosen_query(context, collection),
                };
                let page = schema::metadata(context)?.items(&listing, window).await?;
                connection.has_previous_page = page.has_previous;
                connection.has_next_page = page.has_next;
                connection
                    .edges
                    .extend(page.items.into_iter().map(|listed| {
                        Edge::new(OpaqueCursor(listed.cursor), Item::from_domain(listed.item))
                    }));
                Ok(connection)
            },
        )
        .await
    }
}

/// The listing a smart collection runs for its reader.
fn smart_query(context: &Context<'_>, collection: &Collection, rule: &str) -> Result<ItemQuery> {
    let rule = SmartCollectionRuleInput::read(rule)?;
    let viewer = crate::access::caller(context).and_then(Viewer::of);
    let sort = rule.order_by.unwrap_or_default().sort(viewer, 0);
    crate::library::listing_query(
        context,
        collection.section,
        rule.filter.clone(),
        sort,
        rule.direction,
    )
}

/// The listing of a collection of chosen items, in its order: its library's items, at any
/// depth, kept to its entries.
fn chosen_query(context: &Context<'_>, collection: &Collection) -> ItemQuery {
    let sort = match collection.order {
        CollectionOrder::Title => ItemSort::Title,
        CollectionOrder::ReleaseDate | CollectionOrder::Custom => ItemSort::ReleasedOn,
    };
    let filter = ItemFilter {
        depth: ItemDepth::All,
        collections: vec![CollectionCondition {
            any_of: vec![CollectionRef::Id(collection.id)],
        }],
        viewer: crate::access::caller(context).and_then(Viewer::of),
        ..ItemFilter::default()
    };
    ItemQuery::new(collection.section)
        .filtered(filter)
        .sorted_by(sort, Direction::Ascending)
}

/// Loads collections' totals in one call.
#[derive(Debug, Clone)]
pub struct CollectionTotalsLoader {
    repository: Arc<dyn CollectionRepository>,
}

impl CollectionTotalsLoader {
    pub const fn new(repository: Arc<dyn CollectionRepository>) -> Self {
        Self { repository }
    }
}

impl Loader<CollectionId> for CollectionTotalsLoader {
    type Value = CollectionTotals;
    type Error = Arc<CollectionError>;

    async fn load(
        &self,
        keys: &[CollectionId],
    ) -> Result<HashMap<CollectionId, Self::Value>, Self::Error> {
        Ok(self
            .repository
            .totals(keys)
            .await
            .map_err(Arc::new)?
            .into_iter()
            .collect())
    }
}

async fn totals(context: &Context<'_>, collection: CollectionId) -> Result<CollectionTotals> {
    Ok(context
        .data::<DataLoader<CollectionTotalsLoader>>()?
        .load_one(collection)
        .await?
        .unwrap_or_default())
}

/// Loads the collections several items are in, in one call.
#[derive(Debug, Clone)]
pub struct MembershipsLoader {
    repository: Arc<dyn CollectionRepository>,
}

impl MembershipsLoader {
    pub const fn new(repository: Arc<dyn CollectionRepository>) -> Self {
        Self { repository }
    }
}

impl Loader<RatingKey> for MembershipsLoader {
    type Value = Vec<Collection>;
    type Error = Arc<CollectionError>;

    async fn load(
        &self,
        keys: &[RatingKey],
    ) -> Result<HashMap<RatingKey, Self::Value>, Self::Error> {
        let mut grouped: HashMap<RatingKey, Self::Value> = HashMap::new();
        for membership in self.repository.memberships(keys).await.map_err(Arc::new)? {
            grouped
                .entry(membership.item)
                .or_default()
                .push(membership.collection);
        }
        Ok(grouped)
    }
}

/// The collections an item is in, in the order it joined them. An item and its collections
/// share a library, so a reader of the item reads them all.
pub(crate) async fn collections_of(
    context: &Context<'_>,
    item: RatingKey,
    (after, before, first, last): (Option<String>, Option<String>, Option<i32>, Option<i32>),
) -> Result<MembershipConnection> {
    let rows = context
        .data::<DataLoader<MembershipsLoader>>()?
        .load_one(item)
        .await?
        .unwrap_or_default()
        .into_iter()
        .filter(|collection| crate::access::can_read(context, collection.section))
        .map(|collection| (CollectionObject(collection), EmptyFields))
        .collect();
    crate::tag::page_in_memory(rows, after, before, first, last).await
}

/// The collection under this id, if the caller may read its library.
async fn readable(context: &Context<'_>, id: CollectionId) -> Result<Option<Collection>> {
    Ok(collections(context)?
        .collection(id)
        .await?
        .filter(|collection| crate::access::can_read(context, collection.section)))
}

pub(crate) async fn collection_node(
    context: &Context<'_>,
    key: i64,
) -> Result<Option<CollectionObject>> {
    Ok(readable(context, CollectionId::new(key))
        .await?
        .map(CollectionObject))
}

/// Every library the caller reads.
pub(crate) async fn readable_sections(context: &Context<'_>) -> Result<Vec<SectionId>> {
    let visible = crate::access::visible_sections(context);
    let window = schema::id_window(None, None, Some(MAX_ITEMS as usize), None)?;
    Ok(schema::library_repository(context)?
        .sections(visible.as_deref(), window)
        .await?
        .items
        .into_iter()
        .map(|section| section.id)
        .collect())
}

/// What a listing of collections lists: which, in what order.
#[derive(Debug)]
pub(crate) struct ListRequest {
    pub kind: Option<ItemType>,
    pub smart: Option<bool>,
    pub with_entries: Option<bool>,
    pub tag_number: Option<String>,
    pub matching: Option<String>,
    pub order_by: Option<CollectionListOrder>,
    pub direction: Option<SortDirection>,
}

/// `Library.collections`, for one library, and `Query.collections`, across every library the
/// caller reads.
pub(crate) async fn list(
    context: &Context<'_>,
    sections: Vec<SectionId>,
    request: ListRequest,
    (after, before, first, last): (Option<String>, Option<String>, Option<i32>, Option<i32>),
    offset: Option<u32>,
) -> Result<CollectionConnection> {
    let mut empty = false;
    let kind = request.kind.and_then(ItemType::held);
    // A music kind holds nothing here, and a filter on it finds nothing.
    if request.kind.is_some() && kind.is_none() {
        empty = true;
    }
    let tag_number = request
        .tag_number
        .as_deref()
        .and_then(|number| number.parse::<i64>().ok());
    // A number that does not parse names no collection.
    if request.tag_number.is_some() && tag_number.is_none() {
        empty = true;
    }
    let sections: Vec<SectionId> = sections
        .into_iter()
        .filter(|section| crate::access::can_read(context, *section))
        .collect();
    let collection_query = CollectionQuery {
        sections,
        kind,
        smart: request.smart,
        tag_number,
        with_entries: request.with_entries.unwrap_or(false),
        title_word_prefix: request.matching.filter(|text| !text.trim().is_empty()),
        sort: request.order_by.unwrap_or_default().into(),
        direction: request.direction.unwrap_or_default().into(),
    };
    let skip = u64::from(offset.unwrap_or(0));
    query(
        after,
        before,
        first,
        last,
        |after, before, first, last| async move {
            let total = CollectionTotal {
                query: (!empty).then(|| collection_query.clone()),
            };
            if empty {
                return Ok(Connection::with_additional_fields(false, false, total));
            }
            let window =
                schema::window_for(after, before, first, last, DEFAULT_PAGE_SIZE)?.skipping(skip);
            let page = collections(context)?
                .collections(&collection_query, window)
                .await?;
            let mut connection =
                Connection::with_additional_fields(page.has_previous, page.has_next, total);
            connection
                .edges
                .extend(page.items.into_iter().map(|collection| {
                    Edge::new(
                        OpaqueCursor(collection_query.cursor(&collection)),
                        CollectionObject(collection),
                    )
                }));
            Ok::<_, async_graphql::Error>(connection)
        },
    )
    .await
}

/// Why a collection mutation did nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum CollectionErrorCode {
    /// No collection the caller may change carries this id.
    CollectionNotFound,
    /// No library the caller may change carries this id.
    LibraryNotFound,
    /// No item the caller can read carries this id.
    ItemNotFound,
    /// None of the items can join: a collection holds items of its kind, from its library.
    NotAddable,
    /// The item is not in the collection.
    NotInCollection,
    /// The collection is smart: its items come from its rule, and cannot be changed one by
    /// one.
    SmartCollection,
    /// The collection holds chosen items, and has no rule.
    NotSmart,
    /// The rule names a filter this server cannot honor.
    InvalidRule,
    /// A collection cannot hold items of this kind: music is held nowhere here.
    UnsupportedKind,
    /// A title cannot be empty.
    BlankTitle,
}

/// A request a collection mutation refused, and where in the input the reason lies.
#[derive(Debug, Clone, SimpleObject)]
pub struct CollectionUserError {
    /// The path to the input field at fault.
    pub field: Vec<String>,
    /// What went wrong, for a person to read.
    pub message: String,
    /// What went wrong, for a program to branch on.
    pub code: CollectionErrorCode,
}

impl CollectionUserError {
    fn new(field: &str, code: CollectionErrorCode) -> Self {
        let message = match code {
            CollectionErrorCode::CollectionNotFound => "no such collection",
            CollectionErrorCode::LibraryNotFound => "no such library",
            CollectionErrorCode::ItemNotFound => "no such item",
            CollectionErrorCode::NotAddable => "none of these items can join this collection",
            CollectionErrorCode::NotInCollection => "this item is not in this collection",
            CollectionErrorCode::SmartCollection => "a smart collection's items come from its rule",
            CollectionErrorCode::NotSmart => "this collection has no rule",
            CollectionErrorCode::InvalidRule => "this rule cannot be listed",
            CollectionErrorCode::UnsupportedKind => "no collection here holds this kind",
            CollectionErrorCode::BlankTitle => "a title cannot be empty",
        };
        Self {
            field: vec!["input".to_owned(), field.to_owned()],
            message: message.to_owned(),
            code,
        }
    }
}

type Refusal<T> = std::result::Result<T, CollectionUserError>;

/// The collection an input names, if the caller may change it.
async fn target(context: &Context<'_>, id: &ID, field: &str) -> Result<Refusal<Collection>> {
    let not_found = || {
        Ok(Err(CollectionUserError::new(
            field,
            CollectionErrorCode::CollectionNotFound,
        )))
    };
    let Some(key) = GlobalId::decode(id)
        .ok()
        .filter(|decoded| decoded.type_name == COLLECTION_TYPE)
        .map(|decoded| CollectionId::new(decoded.key))
    else {
        return not_found();
    };
    // What a caller may not change answers as if it did not exist, readable or not.
    match readable(context, key).await? {
        Some(collection) if crate::access::can_edit(context, collection.section) => {
            Ok(Ok(collection))
        }
        _ => not_found(),
    }
}

/// Reads a collection back after a change, or says it went in between.
async fn reread(context: &Context<'_>, id: CollectionId) -> Result<Refusal<Collection>> {
    Ok(readable(context, id)
        .await?
        .ok_or_else(|| CollectionUserError::new("id", CollectionErrorCode::CollectionNotFound)))
}

fn item_key(id: &ID) -> Option<RatingKey> {
    GlobalId::decode(id)
        .ok()
        .filter(|decoded| decoded.type_name == ITEM_TYPE)
        .map(|decoded| RatingKey::new(decoded.key))
}

/// The items that can join a collection of this kind in this library, in order. An id that
/// names no readable item is refused; a readable item of another kind or library is left
/// out, as the reference leaves it out.
async fn joinable(
    context: &Context<'_>,
    section: SectionId,
    kind: ItemKind,
    ids: &[ID],
) -> Result<Refusal<Vec<RatingKey>>> {
    let mut items = Vec::with_capacity(ids.len());
    for id in ids {
        let Some(item) = item_key(id) else {
            return Ok(Err(CollectionUserError::new(
                "itemIds",
                CollectionErrorCode::ItemNotFound,
            )));
        };
        let Some(found): Option<MetadataItem> = schema::readable_item(context, item).await? else {
            return Ok(Err(CollectionUserError::new(
                "itemIds",
                CollectionErrorCode::ItemNotFound,
            )));
        };
        if found.section_id == section && found.kind == kind {
            items.push(item);
        }
    }
    Ok(Ok(items))
}

/// A rule checked against what the listing can honor, in the form it is kept.
fn checked_rule(rule: &SmartCollectionRuleInput) -> Refusal<String> {
    let invalid = || CollectionUserError::new("rule", CollectionErrorCode::InvalidRule);
    if ItemFilter::try_from(rule.filter.clone().unwrap_or_default()).is_err() {
        return Err(invalid());
    }
    rule.stored().map_err(|_| invalid())
}

/// A collection to create: of chosen items, or smart with a rule.
#[derive(Debug, InputObject)]
pub struct CreateCollectionInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// Its library, as returned in `Library.id`. The caller must be allowed to change it.
    pub library_id: ID,
    /// Its name. Not empty.
    pub title: String,
    /// The kind of item it holds.
    pub kind: ItemType,
    /// Its first items, in order, as returned in `Item.id`. A repeat is kept once, and an
    /// item of another kind or library is left out. Not with `rule`.
    #[graphql(default)]
    pub item_ids: Vec<ID>,
    /// Makes it smart: it lists what this rule finds in its library.
    pub rule: Option<SmartCollectionRuleInput>,
    /// Where the content came from, as the client names it; kept and returned as given.
    pub source_uri: Option<String>,
}

/// A collection after a change.
#[derive(Debug, SimpleObject)]
pub struct CollectionPayload {
    /// As given in the input.
    pub client_mutation_id: Option<String>,
    /// The collection as it now stands. Null when the request was refused.
    pub collection: Option<CollectionObject>,
    /// Why the request was refused. Empty on success.
    pub user_errors: Vec<CollectionUserError>,
}

impl CollectionPayload {
    fn of(client_mutation_id: Option<String>, outcome: Refusal<Collection>) -> Self {
        match outcome {
            Ok(collection) => Self {
                client_mutation_id,
                collection: Some(CollectionObject(collection)),
                user_errors: Vec::new(),
            },
            Err(error) => Self {
                client_mutation_id,
                collection: None,
                user_errors: vec![error],
            },
        }
    }
}

pub(crate) async fn create(
    context: &Context<'_>,
    input: CreateCollectionInput,
) -> Result<CollectionPayload> {
    let outcome = async {
        let library_not_found = || {
            Ok(Err(CollectionUserError::new(
                "libraryId",
                CollectionErrorCode::LibraryNotFound,
            )))
        };
        let Some(section) = GlobalId::decode(&input.library_id)
            .ok()
            .filter(|decoded| decoded.type_name == LIBRARY_TYPE)
            .map(|decoded| SectionId::new(decoded.key))
        else {
            return library_not_found();
        };
        if !crate::access::can_edit(context, section)
            || schema::load_library(context, section).await?.is_none()
        {
            return library_not_found();
        }
        if input.title.trim().is_empty() {
            return Ok(Err(CollectionUserError::new(
                "title",
                CollectionErrorCode::BlankTitle,
            )));
        }
        let Some(kind) = input.kind.held() else {
            return Ok(Err(CollectionUserError::new(
                "kind",
                CollectionErrorCode::UnsupportedKind,
            )));
        };
        let rule = match &input.rule {
            Some(_) if !input.item_ids.is_empty() => {
                return Ok(Err(CollectionUserError::new(
                    "itemIds",
                    CollectionErrorCode::SmartCollection,
                )))
            }
            Some(rule) => match checked_rule(rule) {
                Ok(rule) => Some(rule),
                Err(error) => return Ok(Err(error)),
            },
            None => None,
        };
        let items = match joinable(context, section, kind, &input.item_ids).await? {
            Ok(items) => items,
            Err(error) => return Ok(Err(error)),
        };
        let created = collections(context)?
            .create(
                NewCollection {
                    section,
                    kind,
                    title: input.title.clone(),
                    items,
                    rule,
                    source_uri: input.source_uri.clone(),
                },
                now_epoch(),
            )
            .await?;
        Ok::<_, async_graphql::Error>(Ok(created))
    }
    .await?;
    Ok(CollectionPayload::of(input.client_mutation_id, outcome))
}

/// Changes to a collection's own fields. An absent field is left alone.
#[derive(Debug, InputObject)]
pub struct UpdateCollectionInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The collection, as returned in `Collection.id`.
    pub id: ID,
    /// Its new name. Not empty. A new one gives a new `titleSort`, the title less a leading
    /// article, unless `titleSort` is locked or given too.
    pub title: Option<String>,
    /// The title to sort by; null clears it.
    pub title_sort: async_graphql::MaybeUndefined<String>,
    /// Its new description; empty or null clears it.
    pub summary: async_graphql::MaybeUndefined<String>,
    /// The age-rating label; null clears it.
    pub content_rating: async_graphql::MaybeUndefined<String>,
    /// How a library page treats it.
    pub mode: Option<CollectionModeType>,
    /// How its chosen items are ordered.
    pub order: Option<CollectionOrderType>,
    /// Fields to lock.
    #[graphql(default)]
    pub lock: Vec<CollectionFieldType>,
    /// Fields to unlock.
    #[graphql(default)]
    pub unlock: Vec<CollectionFieldType>,
}

pub(crate) async fn update(
    context: &Context<'_>,
    input: UpdateCollectionInput,
) -> Result<CollectionPayload> {
    let UpdateCollectionInput {
        client_mutation_id,
        id,
        title,
        title_sort,
        summary,
        content_rating,
        mode,
        order,
        lock,
        unlock,
    } = input;
    let outcome = async {
        let collection = match target(context, &id, "id").await? {
            Ok(collection) => collection,
            Err(error) => return Ok(Err(error)),
        };
        if title
            .as_deref()
            .is_some_and(|title| title.trim().is_empty())
        {
            return Ok(Err(CollectionUserError::new(
                "title",
                CollectionErrorCode::BlankTitle,
            )));
        }
        let edit = CollectionEdit {
            title,
            title_sort: title_sort.as_opt_ref().map(Option::<&String>::cloned),
            summary: summary
                .as_opt_ref()
                .map(|summary| summary.cloned().unwrap_or_default()),
            content_rating: content_rating.as_opt_ref().map(Option::<&String>::cloned),
            mode: mode.map(Into::into),
            order: order.map(Into::into),
            lock: lock.into_iter().map(Into::into).collect(),
            unlock: unlock.into_iter().map(Into::into).collect(),
        };
        Ok::<_, async_graphql::Error>(Ok(collections(context)?.edit(collection.id, &edit).await?))
    }
    .await?;
    Ok(CollectionPayload::of(client_mutation_id, outcome))
}

/// A collection to delete.
#[derive(Debug, InputObject)]
pub struct DeleteCollectionInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The collection, as returned in `Collection.id`.
    pub id: ID,
}

/// What a deletion removed.
#[derive(Debug, SimpleObject)]
pub struct DeleteCollectionPayload {
    /// As given in the input.
    pub client_mutation_id: Option<String>,
    /// The collection deleted, as it was returned in `Collection.id`. Null when refused.
    pub deleted_collection_id: Option<ID>,
    /// Why the request was refused. Empty on success.
    pub user_errors: Vec<CollectionUserError>,
}

pub(crate) async fn delete(
    context: &Context<'_>,
    input: DeleteCollectionInput,
) -> Result<DeleteCollectionPayload> {
    let outcome = match target(context, &input.id, "id").await? {
        Ok(collection) if collections(context)?.delete(collection.id).await? => Ok(input.id),
        Ok(_) => Err(CollectionUserError::new(
            "id",
            CollectionErrorCode::CollectionNotFound,
        )),
        Err(error) => Err(error),
    };
    let (deleted_collection_id, user_errors) = match outcome {
        Ok(id) => (Some(id), Vec::new()),
        Err(error) => (None, vec![error]),
    };
    Ok(DeleteCollectionPayload {
        client_mutation_id: input.client_mutation_id,
        deleted_collection_id,
        user_errors,
    })
}

/// Items to add to a collection of chosen items.
#[derive(Debug, InputObject)]
pub struct AddToCollectionInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The collection, as returned in `Collection.id`.
    pub collection_id: ID,
    /// The items, in order, as returned in `Item.id`. One it holds already is not added
    /// again, and one of another kind or library is left out; refused when that leaves none.
    pub item_ids: Vec<ID>,
}

/// A collection after items were added.
#[derive(Debug, SimpleObject)]
pub struct AddToCollectionPayload {
    /// As given in the input.
    pub client_mutation_id: Option<String>,
    /// The collection as it now stands. Null when the request was refused.
    pub collection: Option<CollectionObject>,
    /// How many of the items were added: those it did not hold already, each once.
    pub added_count: u64,
    /// Why the request was refused. Empty on success.
    pub user_errors: Vec<CollectionUserError>,
}

pub(crate) async fn add(
    context: &Context<'_>,
    input: AddToCollectionInput,
) -> Result<AddToCollectionPayload> {
    let outcome = async {
        let collection = match target(context, &input.collection_id, "collectionId").await? {
            Ok(collection) => collection,
            Err(error) => return Ok(Err(error)),
        };
        if collection.is_smart() {
            return Ok(Err(CollectionUserError::new(
                "collectionId",
                CollectionErrorCode::SmartCollection,
            )));
        }
        let items = match joinable(
            context,
            collection.section,
            collection.kind,
            &input.item_ids,
        )
        .await?
        {
            Ok(items) => items,
            Err(error) => return Ok(Err(error)),
        };
        if items.is_empty() {
            return Ok(Err(CollectionUserError::new(
                "itemIds",
                CollectionErrorCode::NotAddable,
            )));
        }
        let added = collections(context)?
            .add(collection.id, &items, now_epoch())
            .await?;
        Ok::<_, async_graphql::Error>(reread(context, collection.id).await?.map(|c| (c, added)))
    }
    .await?;
    Ok(match outcome {
        Ok((collection, added_count)) => AddToCollectionPayload {
            client_mutation_id: input.client_mutation_id,
            collection: Some(CollectionObject(collection)),
            added_count,
            user_errors: Vec::new(),
        },
        Err(error) => AddToCollectionPayload {
            client_mutation_id: input.client_mutation_id,
            collection: None,
            added_count: 0,
            user_errors: vec![error],
        },
    })
}

/// An item to take out of a collection of chosen items.
#[derive(Debug, InputObject)]
pub struct RemoveFromCollectionInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The collection, as returned in `Collection.id`.
    pub collection_id: ID,
    /// The item, as returned in `Item.id`. One that is not in it changes nothing.
    pub item_id: ID,
}

/// A collection after an item was taken out.
#[derive(Debug, SimpleObject)]
pub struct RemoveFromCollectionPayload {
    /// As given in the input.
    pub client_mutation_id: Option<String>,
    /// The collection as it now stands. Null when the request was refused.
    pub collection: Option<CollectionObject>,
    /// Whether the item was in it.
    pub removed: bool,
    /// Why the request was refused. Empty on success.
    pub user_errors: Vec<CollectionUserError>,
}

pub(crate) async fn remove(
    context: &Context<'_>,
    input: RemoveFromCollectionInput,
) -> Result<RemoveFromCollectionPayload> {
    let outcome = async {
        let collection = match target(context, &input.collection_id, "collectionId").await? {
            Ok(collection) => collection,
            Err(error) => return Ok(Err(error)),
        };
        let Some(item) = item_key(&input.item_id) else {
            return Ok(Err(CollectionUserError::new(
                "itemId",
                CollectionErrorCode::ItemNotFound,
            )));
        };
        // A smart collection holds no entry to take out: nothing changes, as on the reference.
        let removed = !collection.is_smart()
            && collections(context)?
                .remove(collection.id, item, now_epoch())
                .await?;
        Ok::<_, async_graphql::Error>(reread(context, collection.id).await?.map(|c| (c, removed)))
    }
    .await?;
    Ok(match outcome {
        Ok((collection, removed)) => RemoveFromCollectionPayload {
            client_mutation_id: input.client_mutation_id,
            collection: Some(CollectionObject(collection)),
            removed,
            user_errors: Vec::new(),
        },
        Err(error) => RemoveFromCollectionPayload {
            client_mutation_id: input.client_mutation_id,
            collection: None,
            removed: false,
            user_errors: vec![error],
        },
    })
}

/// An item to move within a collection's custom order.
#[derive(Debug, InputObject)]
pub struct MoveCollectionItemInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The collection, as returned in `Collection.id`.
    pub collection_id: ID,
    /// The item to move, as returned in `Item.id`.
    pub item_id: ID,
    /// The item to put it after, of the same collection. Omitted, it goes first.
    pub after_item_id: Option<ID>,
}

pub(crate) async fn move_item(
    context: &Context<'_>,
    input: MoveCollectionItemInput,
) -> Result<CollectionPayload> {
    let outcome = async {
        let collection = match target(context, &input.collection_id, "collectionId").await? {
            Ok(collection) => collection,
            Err(error) => return Ok(Err(error)),
        };
        if collection.is_smart() {
            return Ok(Err(CollectionUserError::new(
                "collectionId",
                CollectionErrorCode::SmartCollection,
            )));
        }
        let not_in = |field| {
            Ok(Err(CollectionUserError::new(
                field,
                CollectionErrorCode::NotInCollection,
            )))
        };
        let Some(item) = item_key(&input.item_id) else {
            return not_in("itemId");
        };
        let destination = match &input.after_item_id {
            None => Destination::First,
            Some(after) => match item_key(after) {
                Some(after) if after == item => return not_in("afterItemId"),
                Some(after) => Destination::After(after),
                None => return not_in("afterItemId"),
            },
        };
        match collections(context)?
            .move_item(collection.id, item, destination)
            .await
        {
            Ok(()) => reread(context, collection.id).await,
            Err(CollectionError::NotInCollection(missing)) => not_in(if missing == item {
                "itemId"
            } else {
                "afterItemId"
            }),
            Err(error) => Err(error.into()),
        }
    }
    .await?;
    Ok(CollectionPayload::of(input.client_mutation_id, outcome))
}

/// A collection of chosen items to empty.
#[derive(Debug, InputObject)]
pub struct ClearCollectionInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The collection, as returned in `Collection.id`.
    pub id: ID,
}

pub(crate) async fn clear(
    context: &Context<'_>,
    input: ClearCollectionInput,
) -> Result<CollectionPayload> {
    let outcome = async {
        let collection = match target(context, &input.id, "id").await? {
            Ok(collection) => collection,
            Err(error) => return Ok(Err(error)),
        };
        if collection.is_smart() {
            return Ok(Err(CollectionUserError::new(
                "id",
                CollectionErrorCode::SmartCollection,
            )));
        }
        collections(context)?.clear(collection.id).await?;
        reread(context, collection.id).await
    }
    .await?;
    Ok(CollectionPayload::of(input.client_mutation_id, outcome))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rule_round_trips_through_its_stored_form() {
        let rule = SmartCollectionRuleInput {
            filter: Some(ItemFilterInput {
                kind: Some(ItemType::Episode),
                year_from: Some(2021),
                ..ItemFilterInput::default()
            }),
            order_by: Some(ItemOrder::ReleasedOn),
            direction: Some(SortDirection::Descending),
        };
        let stored = checked_rule(&rule).ok();
        let read = stored
            .as_deref()
            .and_then(|stored| SmartCollectionRuleInput::read(stored).ok());
        assert_eq!(
            read.and_then(|rule| rule.order_by),
            Some(ItemOrder::ReleasedOn)
        );
    }
}
