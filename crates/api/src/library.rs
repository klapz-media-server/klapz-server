//! GraphQL types for libraries.

use async_graphql::connection::{
    query, Connection, ConnectionNameType, Edge, EdgeNameType, EmptyFields,
};
use async_graphql::dataloader::DataLoader;
use async_graphql::{Context, Enum, Object, OutputType, Result, SimpleObject, ID};
use open_plex_core::library::{LibrarySection, SectionKind, SectionRoot};
use open_plex_core::metadata::{
    FieldValue, FirstCharacter as DomainFirstCharacter, ItemFilter, ItemQuery, ItemSort, TagScope,
    ValueField,
};
use open_plex_core::page::{Cursor, Direction, Window, MAX_ITEMS};
use open_plex_core::recent::recently_added_episodes;
use open_plex_core::{Listed, RatingKey, SectionId};

use crate::common::CommonFieldsObject;
use crate::cursor::OpaqueCursor;
use crate::filter::{ItemFilterInput, ItemOrder, SortDirection, TrackField};
use crate::item::{Item, ItemConnection};
use crate::loader::{ItemLoader, RootsLoader};
use crate::node::GlobalId;
use crate::schema::{self, ApiContext, DEFAULT_PAGE_SIZE};
use crate::tag::{TagObject, TagType};
use crate::DateTime;

pub const LIBRARY_TYPE: &str = "Library";
pub const ROOT_TYPE: &str = "Root";

/// A page of libraries, with cursors for walking the rest.
pub type LibraryConnection = Connection<OpaqueCursor, Library, EmptyFields, EmptyFields>;

/// The kind of media a library holds.
///
/// Declared here rather than deriving GraphQL traits on the domain enum, so `core` stays free
/// of framework dependencies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum LibraryKind {
    /// Films, each a single playable item.
    Movie,
    /// Television series, browsed as show then season then episode.
    Show,
    /// Music, browsed as artist then album then track.
    Artist,
    /// Photographs and home videos.
    Photo,
}

impl From<LibraryKind> for SectionKind {
    fn from(kind: LibraryKind) -> Self {
        match kind {
            LibraryKind::Movie => Self::Movie,
            LibraryKind::Show => Self::Show,
            LibraryKind::Artist => Self::Artist,
            LibraryKind::Photo => Self::Photo,
        }
    }
}

impl From<SectionKind> for LibraryKind {
    fn from(kind: SectionKind) -> Self {
        match kind {
            SectionKind::Movie => Self::Movie,
            SectionKind::Show => Self::Show,
            SectionKind::Artist => Self::Artist,
            SectionKind::Photo => Self::Photo,
        }
    }
}

/// A filesystem location a library scans for media.
#[derive(Debug, Clone)]
pub struct Root(SectionRoot);

impl Root {
    pub const fn new(root: SectionRoot) -> Self {
        Self(root)
    }
}

#[Object]
impl Root {
    /// Globally unique, opaque identifier. Pass it to `node` to refetch this root.
    pub async fn id(&self) -> ID {
        ID(GlobalId::encode(ROOT_TYPE, self.0.id))
    }

    /// Absolute path on the server's filesystem.
    async fn path(&self) -> &str {
        &self.0.path
    }

    /// When the root was added to the library.
    async fn created_at(&self) -> DateTime {
        self.0.created_at.into()
    }
}

/// What a "recently added" entry says beyond the item.
#[derive(Debug, Clone, SimpleObject)]
pub struct RecentlyAddedEdgeFields {
    /// When the entry was added: the item's own time for a film, its newest episode's for a
    /// series or season gathered from a batch.
    pub added_at: DateTime,
}

/// Names the connection apart from `ItemConnection`, which shares its node type.
#[derive(Debug)]
pub struct RecentlyAddedConnectionName;

impl ConnectionNameType for RecentlyAddedConnectionName {
    fn type_name<T: OutputType>() -> String {
        "RecentlyAddedConnection".to_owned()
    }
}

#[derive(Debug)]
pub struct RecentlyAddedEdgeName;

impl EdgeNameType for RecentlyAddedEdgeName {
    fn type_name<T: OutputType>() -> String {
        "RecentlyAddedEdge".to_owned()
    }
}

/// A page of recently added entries.
pub type RecentlyAddedConnection = Connection<
    OpaqueCursor,
    Item,
    EmptyFields,
    RecentlyAddedEdgeFields,
    RecentlyAddedConnectionName,
    RecentlyAddedEdgeName,
>;

/// A text field whose distinct values a library can be browsed by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum TextField {
    /// The certification, as the metadata provider states it: `PG-13`, `fr/12`.
    ContentRating,
    /// The studio or network that made the item.
    Studio,
}

impl From<TextField> for ValueField {
    fn from(field: TextField) -> Self {
        match field {
            TextField::ContentRating => Self::ContentRating,
            TextField::Studio => Self::Studio,
        }
    }
}

/// Names a connection after what it lists, since its node type is a bare scalar or a type
/// another connection already lists.
macro_rules! connection_names {
    ($connection:ident, $edge:ident) => {
        #[derive(Debug)]
        pub struct $connection;

        impl ConnectionNameType for $connection {
            fn type_name<T: OutputType>() -> String {
                stringify!($connection).to_owned()
            }
        }

        #[derive(Debug)]
        pub struct $edge;

        impl EdgeNameType for $edge {
            fn type_name<T: OutputType>() -> String {
                stringify!($edge).to_owned()
            }
        }
    };
}

connection_names!(LibraryTagConnection, LibraryTagEdge);
connection_names!(YearConnection, YearEdge);
connection_names!(FieldValueConnection, FieldValueEdge);

/// A page of the tags a library's items carry.
pub type LibraryTags = Connection<
    OpaqueCursor,
    TagObject,
    EmptyFields,
    EmptyFields,
    LibraryTagConnection,
    LibraryTagEdge,
>;
/// A page of the years a library's items were released in.
pub type Years = Connection<OpaqueCursor, i32, EmptyFields, EmptyFields, YearConnection, YearEdge>;
/// A page of the values a library's items hold in a text field.
pub type FieldValues = Connection<
    OpaqueCursor,
    String,
    EmptyFields,
    EmptyFields,
    FieldValueConnection,
    FieldValueEdge,
>;

/// A collection of media of a single kind — what the interface calls a library.
#[derive(Debug, Clone)]
pub struct Library {
    section: LibrarySection,
}

impl Library {
    pub const fn new(section: LibrarySection) -> Self {
        Self { section }
    }

    /// The cursor identifying this library in a connection.
    ///
    /// Libraries are ordered by identifier, so the id alone locates a position and no sort
    /// key is needed: inserting a library does not shift the others.
    pub const fn cursor(&self) -> Cursor {
        Cursor::by_id(self.section.id.get())
    }

    pub(crate) const fn section_id(&self) -> SectionId {
        self.section.id
    }

    /// Whether this server scans libraries of this kind: films and series.
    pub(crate) const fn is_scannable(&self) -> bool {
        matches!(self.section.kind, SectionKind::Movie | SectionKind::Show)
    }
}

// Doc comments on these resolvers are the published schema descriptions, so a Rust
// `# Errors` section would leak into the schema.
#[allow(clippy::missing_errors_doc)]
#[Object]
impl Library {
    /// Globally unique, opaque identifier. Pass it to `node` to refetch this library.
    ///
    /// Treat it as a token: its encoding is not part of the contract and may change.
    pub async fn id(&self) -> ID {
        ID(GlobalId::encode(LIBRARY_TYPE, self.section.id.get()))
    }

    /// What kind of media this library holds.
    async fn kind(&self) -> LibraryKind {
        self.section.kind.into()
    }

    /// Display name, as the owner set it.
    async fn title(&self) -> &str {
        &self.section.title
    }

    /// Title to sort by when it differs from `title`, for example with a leading article
    /// removed. Null when `title` already sorts correctly.
    async fn title_sort(&self) -> Option<&str> {
        self.section.title_sort.as_deref()
    }

    /// BCP 47 language tag used when matching metadata, for example `fr-FR`.
    async fn language(&self) -> &str {
        &self.section.language
    }

    /// Stable identifier that survives renaming, used to recognize this library across
    /// servers and backups.
    async fn uuid(&self) -> &str {
        &self.section.uuid
    }

    /// Whether the library is excluded from normal browsing.
    async fn hidden(&self) -> bool {
        self.section.hidden
    }

    /// When the library was created.
    async fn created_at(&self) -> DateTime {
        self.section.created_at.into()
    }

    /// When the library's settings or contents last changed.
    async fn updated_at(&self) -> DateTime {
        self.section.updated_at.into()
    }

    /// When the library was last scanned. Null if it never
    /// has been.
    async fn scanned_at(&self) -> Option<DateTime> {
        self.section.scanned_at.map(DateTime::from)
    }

    /// Whether a scan of this library is running now (`scanLibrary`).
    #[allow(
        clippy::unused_async,
        reason = "async-graphql requires resolvers taking a context to be async"
    )]
    async fn scanning(&self, context: &Context<'_>) -> Result<bool> {
        Ok(context
            .data::<ApiContext>()?
            .scans
            .is_running(self.section.id))
    }

    /// Monotonic counter that advances whenever the library's contents change.
    ///
    /// A string rather than an `Int` because the value exceeds 32 bits, which is the width
    /// GraphQL's `Int` guarantees. It is a counter, not a timestamp: compare it for
    /// inequality to detect a change, do not read it as a time.
    async fn content_changed_at(&self) -> String {
        self.section.content_changed_at.to_string()
    }

    /// What was added to this library most recently, newest first.
    ///
    /// For a film library, its films by date added. For a series library, new episodes
    /// grouped as the reference groups them: a batch of episodes added within twenty hours
    /// of one another stands for its season, or its series when it spans several seasons,
    /// and a lone episode for itself; each item appears once, at its newest batch. So the
    /// list mixes series, seasons and episodes.
    ///
    /// Pages forward only: the grouping is computed from the newest addition down, so there
    /// is no end to page back from.
    async fn recently_added(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Return entries after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return at most this many entries.")] first: Option<i32>,
        #[graphql(
            desc = "Skip this many entries first. A position, for a client that pages by one; it costs the entries it skips."
        )]
        offset: Option<u32>,
    ) -> Result<RecentlyAddedConnection> {
        // Ties in title order, as the library's own listing by date added has them.
        recently_added(
            context,
            vec![self.section.id],
            self.section.kind,
            ItemSort::AddedAt,
            (after, first, offset),
        )
        .await
    }

    /// This library's collections.
    // One Rust parameter per field argument; see the note on `items`.
    #[allow(clippy::too_many_arguments)]
    async fn collections(
        &self,
        context: &Context<'_>,
        #[graphql(
            desc = "Only collections having a word that starts with each word of this text, in their title or sort title, accents folded."
        )]
        matching: Option<String>,
        #[graphql(desc = "Only collections holding items of this kind.")] kind: Option<
            crate::filter::ItemType,
        >,
        #[graphql(
            desc = "Only smart collections (`true`), or only those of chosen items (`false`)."
        )]
        smart: Option<bool>,
        #[graphql(
            desc = "Only collections holding a chosen item (`true`): those a filter on collections can find."
        )]
        with_entries: Option<bool>,
        #[graphql(desc = "Only the collection carried under this `Collection.tagNumber`.")]
        tag_number: Option<String>,
        #[graphql(desc = "How to order them.")] order_by: Option<
            crate::collection::CollectionListOrder,
        >,
        #[graphql(desc = "Which direction to order in.")] direction: Option<SortDirection>,
        #[graphql(desc = "Return collections after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return collections before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many collections from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many collections from the end of the range.")]
        last: Option<i32>,
        #[graphql(desc = "Skip this many collections first.")] offset: Option<u32>,
    ) -> Result<crate::collection::CollectionConnection> {
        let request = crate::collection::ListRequest {
            kind,
            smart,
            with_entries,
            tag_number,
            matching,
            order_by,
            direction,
        };
        crate::collection::list(
            context,
            vec![self.section.id],
            request,
            (after, before, first, last),
            offset,
        )
        .await
    }

    /// What the caller may do in this library.
    async fn viewer_role(&self, context: &Context<'_>) -> Option<crate::access::LibraryRoleType> {
        crate::access::viewer_role(context, self.section.id)
    }

    /// Filesystem locations this library scans.
    ///
    /// Loaded in one batch across every library in the result, so asking for roots on a
    /// large list costs a single storage query.
    async fn roots(&self, context: &Context<'_>) -> Result<Vec<Root>> {
        let loader = context.data::<DataLoader<RootsLoader>>()?;
        Ok(loader
            .load_one(self.section.id)
            .await?
            .unwrap_or_default()
            .into_iter()
            .map(Root::new)
            .collect())
    }

    /// The items this library holds.
    ///
    /// Polymorphic: a movie library yields `Movie`, a show library yields `Show`. Use inline
    /// fragments to read kind-specific fields.
    ///
    /// Filtering, ordering and the page window are all pushed down to storage, so the cost
    /// does not grow with the size of the library.
    // A GraphQL resolver takes one Rust parameter per field argument, so the count is the
    // schema's shape rather than a signature anyone chose. Grouping them into a struct would
    // change the published arguments, which is the opposite of what the lint wants.
    #[allow(clippy::too_many_arguments)]
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
        #[graphql(desc = "Narrows the listing. Every field is combined with `AND`.")]
        filter: Option<ItemFilterInput>,
        #[graphql(desc = "How to order the listing.")] order_by: Option<ItemOrder>,
        #[graphql(desc = "Which direction to order in.")] direction: Option<SortDirection>,
        #[graphql(
            desc = "Which shuffle `RANDOM` gives: the same seed, the same order. Ignored by every other order. Omitted, 0."
        )]
        seed: Option<u32>,
        #[graphql(
            desc = "Discard this many items before the page begins. Prefer cursors: an offset costs the database a traversal of every discarded row, and a concurrent insert shifts it. Provided because a client that renders a scrollbar needs to jump to an absolute position, which no cursor can express."
        )]
        offset: Option<u32>,
    ) -> Result<ItemConnection> {
        let viewer = crate::access::viewer_id(context);
        let sort = order_by.unwrap_or_default().sort(viewer, seed.unwrap_or(0));
        let item_query = self.item_query(context, filter, sort, direction)?;
        let skip = u64::from(offset.unwrap_or(0));

        query(
            after,
            before,
            first,
            last,
            |after, before, first, last| async move {
                let window = schema::window_for(after, before, first, last, DEFAULT_PAGE_SIZE)?
                    .skipping(skip);
                let page = schema::metadata(context)?
                    .items(&item_query, window)
                    .await?;

                let mut connection = Connection::new(page.has_previous, page.has_next);
                connection
                    .edges
                    .extend(page.items.into_iter().map(|listed| {
                        Edge::new(OpaqueCursor(listed.cursor), Item::from_domain(listed.item))
                    }));
                Ok::<_, async_graphql::Error>(connection)
            },
        )
        .await
    }

    /// The tags of one kind that this library's items carry, each once, ordered by name.
    ///
    /// The choices to offer for a tag filter: only the genres the library's films actually
    /// have, not every genre the server knows.
    #[allow(clippy::too_many_arguments)]
    async fn tags(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Which kind of tag to list.")] kind: TagType,
        #[graphql(
            desc = "Only credits billed within an item's first this many positions of their kind: 3 lists a library's leading actors. Omitted, every credit counts."
        )]
        billed_within: Option<u32>,
        #[graphql(desc = "List only the tags of the items this filter keeps.")] filter: Option<
            ItemFilterInput,
        >,
        #[graphql(
            desc = "List only the tags whose name this text completes: one phrase beginning a word of the name, case aside and accents minded, as `ItemFilterInput.titleCompletion` reads a title. `tom h` and `hanks` both complete Tom Hanks."
        )]
        completing: Option<String>,
        #[graphql(desc = "Return tags after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return tags before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many tags from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many tags from the end of the range.")] last: Option<
            i32,
        >,
        #[graphql(desc = "Discard this many tags before the page begins.")] offset: Option<u32>,
    ) -> Result<LibraryTags> {
        let item_query = self.item_query(context, filter, ItemSort::Title, None)?;
        let scope = TagScope {
            kind: kind.into(),
            billed_within,
        };
        let skip = u64::from(offset.unwrap_or(0));
        query(
            after,
            before,
            first,
            last,
            |after, before, first, last| async move {
                let window = schema::window_for(after, before, first, last, DEFAULT_PAGE_SIZE)?
                    .skipping(skip);
                let metadata = schema::metadata(context)?;
                let page = match &completing {
                    Some(text) => {
                        metadata
                            .tags_completing(&item_query, scope, text, window)
                            .await?
                    }
                    None => metadata.tags_in(&item_query, scope, window).await?,
                };
                let mut connection = Connection::new(page.has_previous, page.has_next);
                connection.edges.extend(
                    page.items
                        .into_iter()
                        .map(|tag| Edge::new(OpaqueCursor(tag.name_cursor()), TagObject(tag))),
                );
                Ok::<_, async_graphql::Error>(connection)
            },
        )
        .await
    }

    /// The years this library's items were released in, each once, newest first.
    #[allow(clippy::too_many_arguments)]
    async fn years(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "List only the years of the items this filter keeps.")] filter: Option<
            ItemFilterInput,
        >,
        #[graphql(desc = "Return years after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return years before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many years from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many years from the end of the range.")] last: Option<
            i32,
        >,
        #[graphql(desc = "Discard this many years before the page begins.")] offset: Option<u32>,
    ) -> Result<Years> {
        let item_query = self.item_query(context, filter, ItemSort::Title, None)?;
        let skip = u64::from(offset.unwrap_or(0));
        query(
            after,
            before,
            first,
            last,
            |after, before, first, last| async move {
                let window = schema::window_for(after, before, first, last, DEFAULT_PAGE_SIZE)?
                    .skipping(skip);
                let page = schema::metadata(context)?
                    .values_in(&item_query, ValueField::Year, window)
                    .await?;
                let mut connection = Connection::new(page.has_previous, page.has_next);
                connection
                    .edges
                    .extend(page.items.into_iter().filter_map(|value| {
                        let cursor = OpaqueCursor(value.cursor());
                        match value {
                            FieldValue::Year(year) => Some(Edge::new(cursor, year)),
                            FieldValue::Text(_) => None,
                        }
                    }));
                Ok::<_, async_graphql::Error>(connection)
            },
        )
        .await
    }

    /// The values this library's items hold in a text field, each once, in ascending byte
    /// order. Items holding none are left out.
    #[allow(clippy::too_many_arguments)]
    async fn field_values(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Which field to list the values of.")] field: TextField,
        #[graphql(desc = "List only the values of the items this filter keeps.")] filter: Option<
            ItemFilterInput,
        >,
        #[graphql(desc = "Return values after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return values before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many values from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many values from the end of the range.")]
        last: Option<i32>,
        #[graphql(desc = "Discard this many values before the page begins.")] offset: Option<u32>,
    ) -> Result<FieldValues> {
        let item_query = self.item_query(context, filter, ItemSort::Title, None)?;
        let window = (after, before, first, last, offset);
        text_values(context, item_query, field.into(), window).await
    }

    /// The values the tracks of this library's items hold, each once, in ascending byte
    /// order: every video codec, say, of every version. Series have no tracks: their
    /// episodes', reached with `depth: ALL`, are listed.
    #[allow(clippy::too_many_arguments)]
    async fn track_values(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Which value of the tracks to list.")] field: TrackField,
        #[graphql(desc = "List only the values of the items this filter keeps.")] filter: Option<
            ItemFilterInput,
        >,
        #[graphql(desc = "Return values after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return values before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many values from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many values from the end of the range.")]
        last: Option<i32>,
        #[graphql(desc = "Discard this many values before the page begins.")] offset: Option<u32>,
    ) -> Result<FieldValues> {
        let item_query = self.item_query(context, filter, ItemSort::Title, None)?;
        let window = (after, before, first, last, offset);
        text_values(context, item_query, ValueField::Track(field.into()), window).await
    }

    /// What every item a filter keeps has in common — fields, tags, collections and locks:
    /// what a client starts from to edit several items at once.
    async fn common(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The items compared: those this filter keeps.")] filter: Option<
            ItemFilterInput,
        >,
    ) -> Result<CommonFieldsObject> {
        let item_query = self.item_query(context, filter, ItemSort::Title, None)?;
        Ok(CommonFieldsObject(
            schema::metadata(context)?
                .common_fields(&item_query)
                .await?,
        ))
    }

    /// Aggregates over the contents of this library.
    // async-graphql rejects a synchronous resolver, and this one awaits nothing.
    #[allow(clippy::unused_async)]
    async fn stats(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Count only the items this filter keeps.")] filter: Option<
            ItemFilterInput,
        >,
    ) -> Result<LibraryStats> {
        Ok(LibraryStats {
            query: self.item_query(context, filter, ItemSort::Title, None)?,
        })
    }
}

/// What `sections`, all of `kind`, added most recently: one page by position, as
/// `Library.recentlyAdded` and the root `recentlyAdded` serve it. Films follow `film_order`,
/// which says how ties on the date fall.
pub(crate) async fn recently_added(
    context: &Context<'_>,
    sections: Vec<SectionId>,
    kind: SectionKind,
    film_order: ItemSort,
    (after, first, offset): (Option<String>, Option<i32>, Option<u32>),
) -> Result<RecentlyAddedConnection> {
    let skipped = usize::try_from(offset.unwrap_or(0)).unwrap_or(usize::MAX);
    query(
        after,
        None,
        first,
        None,
        |after: Option<OpaqueCursor>, _: Option<OpaqueCursor>, first, _| async move {
            let start = after.map_or(skipped, |cursor| {
                usize::try_from(cursor.0.id).map_or(0, |position| position + 1)
            });
            let count = first.unwrap_or(DEFAULT_PAGE_SIZE as usize);
            if count > MAX_ITEMS as usize {
                return Err(async_graphql::Error::new(format!(
                    "a page of more than {MAX_ITEMS} is not served"
                )));
            }
            let (items, has_next) =
                recent(context, sections, kind, film_order, start, count).await?;

            let mut connection = Connection::new(start > 0, has_next);
            connection.edges.extend(items.into_iter().enumerate().map(
                |(index, (item, added_at))| {
                    let position = i64::try_from(start + index).unwrap_or(i64::MAX);
                    Edge::with_additional_fields(
                        OpaqueCursor(Cursor::by_id(position)),
                        Item::from_domain(item),
                        RecentlyAddedEdgeFields {
                            added_at: added_at.into(),
                        },
                    )
                },
            ));
            Ok::<_, async_graphql::Error>(connection)
        },
    )
    .await
}

/// The `count` recent entries of `sections` from position `start`, and whether more follow.
///
/// Series libraries are grouped one by one and then merged by date: an item belongs to one
/// library, so a batch never spans two.
async fn recent(
    context: &Context<'_>,
    sections: Vec<SectionId>,
    kind: SectionKind,
    film_order: ItemSort,
    start: usize,
    count: usize,
) -> Result<(Vec<(open_plex_core::MetadataItem, i64)>, bool)> {
    let metadata = schema::metadata(context)?;
    if kind != SectionKind::Show {
        let query = ItemQuery::across(sections).sorted_by(film_order, Direction::Descending);
        let window = Window::new(
            None,
            None,
            Some(u32::try_from(count).unwrap_or(MAX_ITEMS)),
            None,
        )?
        .skipping(u64::try_from(start).unwrap_or(u64::MAX));
        let page = metadata.items(&query, window).await?;
        let items = page
            .items
            .into_iter()
            .map(|Listed { item, .. }| {
                let added_at = item.added_at;
                (item, added_at)
            })
            .collect();
        return Ok((items, page.has_next));
    }

    // One more than asked, to learn whether a further page exists.
    let mut entries = Vec::new();
    for section in sections {
        entries
            .extend(recently_added_episodes(metadata.as_ref(), section, start + count + 1).await?);
    }
    // Stable, so one library's order is kept within a shared second.
    entries.sort_by(|left, right| right.added_at.cmp(&left.added_at));
    let has_next = entries.len() > start + count;
    let wanted: Vec<(RatingKey, i64)> = entries
        .iter()
        .skip(start)
        .take(count)
        .map(|addition| (addition.entry.item(), addition.added_at))
        .collect();
    // Batched, then put back in the grouping's order.
    let loaded = context
        .data::<DataLoader<ItemLoader>>()?
        .load_many(wanted.iter().map(|(id, _)| *id))
        .await?;
    let items = wanted
        .iter()
        .filter_map(|(id, added_at)| Some((loaded.get(id).cloned()?, *added_at)))
        .collect();
    Ok((items, has_next))
}

/// The libraries of `kind` the caller can see.
pub(crate) async fn visible_of_kind(
    context: &Context<'_>,
    kind: SectionKind,
) -> Result<Vec<SectionId>> {
    let visible = crate::access::visible_sections(context);
    // Libraries are a handful; one window holds them all.
    let window = Window::new(None, None, Some(MAX_ITEMS), None)?;
    let page = schema::library_repository(context)?
        .sections(visible.as_deref(), window)
        .await?;
    Ok(page
        .items
        .into_iter()
        .filter(|section| section.kind == kind)
        .map(|section| section.id)
        .collect())
}

impl Library {
    /// Builds the storage query from the arguments a client supplied.
    ///
    /// Fails on a tag condition that cannot be honored — a malformed identifier, or one
    /// naming nothing — rather than dropping it and answering with more than was asked.
    fn item_query(
        &self,
        context: &Context<'_>,
        filter: Option<ItemFilterInput>,
        sort: ItemSort,
        direction: Option<SortDirection>,
    ) -> Result<ItemQuery> {
        listing_query(context, self.section.id, filter, sort, direction)
    }
}

/// A connection's `after`, `before`, `first`, `last` and `offset` arguments.
type PageArguments = (
    Option<String>,
    Option<String>,
    Option<i32>,
    Option<i32>,
    Option<u32>,
);

/// A page of the distinct text values a query's items hold in a field.
async fn text_values(
    context: &Context<'_>,
    item_query: ItemQuery,
    field: ValueField,
    (after, before, first, last, offset): PageArguments,
) -> Result<FieldValues> {
    let skip = u64::from(offset.unwrap_or(0));
    query(
        after,
        before,
        first,
        last,
        |after, before, first, last| async move {
            let window =
                schema::window_for(after, before, first, last, DEFAULT_PAGE_SIZE)?.skipping(skip);
            let page = schema::metadata(context)?
                .values_in(&item_query, field, window)
                .await?;
            let mut connection = Connection::new(page.has_previous, page.has_next);
            connection
                .edges
                .extend(page.items.into_iter().filter_map(|value| {
                    let cursor = OpaqueCursor(value.cursor());
                    match value {
                        FieldValue::Text(text) => Some(Edge::new(cursor, text)),
                        FieldValue::Year(_) => None,
                    }
                }));
            Ok::<_, async_graphql::Error>(connection)
        },
    )
    .await
}

/// One library's listing as the caller reads it: `Library.items` and a smart playlist both
/// list through here.
pub(crate) fn listing_query(
    context: &Context<'_>,
    section: SectionId,
    filter: Option<ItemFilterInput>,
    sort: ItemSort,
    direction: Option<SortDirection>,
) -> Result<ItemQuery> {
    let mut filter: ItemFilter = filter.unwrap_or_default().try_into()?;
    // Watch filters read the caller's own state.
    filter.viewer = crate::access::viewer_id(context);
    Ok(ItemQuery::new(section)
        .filtered(filter)
        .sorted_by(sort, direction.unwrap_or_default().into()))
}

/// One letter of a library's A–Z jump bar.
#[derive(Debug, Clone, SimpleObject)]
pub struct FirstCharacter {
    /// The bucket: an uppercase letter, or `#` for everything that does not begin with one.
    ///
    /// Folded before bucketing, so `Çélèbre` counts under `C`. Ligatures are not folded and
    /// get their own bucket, which is why `Œ` can appear as a letter of its own.
    pub character: String,
    /// How many items fall under it.
    pub count: u64,
}

impl From<DomainFirstCharacter> for FirstCharacter {
    fn from(bucket: DomainFirstCharacter) -> Self {
        Self {
            character: bucket.character,
            count: bucket.count,
        }
    }
}

/// Counts over a library, grouped so a client opts into the cost of computing them.
#[derive(Debug, Clone)]
pub struct LibraryStats {
    query: ItemQuery,
}

#[allow(clippy::missing_errors_doc)]
#[Object]
impl LibraryStats {
    /// How many items match, without paging through them.
    async fn item_count(&self, context: &Context<'_>) -> Result<u64> {
        Ok(schema::metadata(context)?.item_count(&self.query).await?)
    }

    /// How many distinct tags of one kind the items carry: the size of `Library.tags`.
    async fn tag_count(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Which kind of tag to count.")] kind: TagType,
        #[graphql(
            desc = "Count only credits billed within this many positions, as `Library.tags` does."
        )]
        billed_within: Option<u32>,
    ) -> Result<u64> {
        let scope = TagScope {
            kind: kind.into(),
            billed_within,
        };
        Ok(schema::metadata(context)?
            .tag_count_in(&self.query, scope)
            .await?)
    }

    /// How many distinct release years the items have: the size of `Library.years`.
    async fn year_count(&self, context: &Context<'_>) -> Result<u64> {
        Ok(schema::metadata(context)?
            .value_count_in(&self.query, ValueField::Year)
            .await?)
    }

    /// How many distinct values the items hold in a field: the size of
    /// `Library.fieldValues`.
    async fn field_value_count(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Which field to count the values of.")] field: TextField,
    ) -> Result<u64> {
        Ok(schema::metadata(context)?
            .value_count_in(&self.query, field.into())
            .await?)
    }

    /// How many distinct values the items' tracks hold in a field: the size of
    /// `Library.trackValues`.
    async fn track_value_count(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Which value of the tracks to count.")] field: TrackField,
    ) -> Result<u64> {
        Ok(schema::metadata(context)?
            .value_count_in(&self.query, ValueField::Track(field.into()))
            .await?)
    }

    /// The first-letter buckets, with a count each, ordered `#` then A–Z.
    ///
    /// What a client draws as a jump bar beside a long list. One row per letter, so this is
    /// 28 numbers rather than a walk over the library.
    async fn first_characters(&self, context: &Context<'_>) -> Result<Vec<FirstCharacter>> {
        Ok(schema::metadata(context)?
            .first_characters(&self.query)
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }
}

/// Resolves the `libraries` root field.
///
/// Lives here rather than in `schema` so the connection and the type it carries stay
/// together.
///
/// # Errors
///
/// Fails on a malformed cursor, a page size past the cap, or a storage failure.
pub async fn libraries(
    context: &Context<'_>,
    after: Option<String>,
    before: Option<String>,
    first: Option<i32>,
    last: Option<i32>,
) -> Result<LibraryConnection> {
    query(
        after,
        before,
        first,
        last,
        |after, before, first, last| async move {
            let window = schema::window_for(after, before, first, last, DEFAULT_PAGE_SIZE)?;
            let visible = crate::access::visible_sections(context);
            let page = schema::library_repository(context)?
                .sections(visible.as_deref(), window)
                .await?;

            let mut connection = Connection::new(page.has_previous, page.has_next);
            connection
                .edges
                .extend(page.items.into_iter().map(|section| {
                    let library = Library::new(section);
                    Edge::new(OpaqueCursor(library.cursor()), library)
                }));
            Ok::<_, async_graphql::Error>(connection)
        },
    )
    .await
}
