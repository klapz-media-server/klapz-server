//! GraphQL types for libraries.

use async_graphql::connection::{query, Connection, Edge, EmptyFields};
use async_graphql::dataloader::DataLoader;
use async_graphql::{Context, Enum, Object, Result, SimpleObject, ID};
use open_plex_core::library::{LibrarySection, SectionKind, SectionRoot};
use open_plex_core::metadata::{FirstCharacter as DomainFirstCharacter, ItemQuery};
use open_plex_core::page::Cursor;

use crate::cursor::OpaqueCursor;
use crate::filter::{ItemFilterInput, ItemOrder, SortDirection};
use crate::item::{Item, ItemConnection};
use crate::loader::RootsLoader;
use crate::node::GlobalId;
use crate::schema::{self, DEFAULT_PAGE_SIZE};

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

    /// When the root was added to the library, in seconds since the Unix epoch.
    async fn created_at(&self) -> i64 {
        self.0.created_at
    }
}

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

    /// When the library was created, in seconds since the Unix epoch.
    async fn created_at(&self) -> i64 {
        self.section.created_at
    }

    /// When the library's settings or contents last changed, in seconds since the Unix epoch.
    async fn updated_at(&self) -> i64 {
        self.section.updated_at
    }

    /// When the library was last scanned, in seconds since the Unix epoch. Null if it never
    /// has been.
    async fn scanned_at(&self) -> Option<i64> {
        self.section.scanned_at
    }

    /// Monotonic counter that advances whenever the library's contents change.
    ///
    /// A string rather than an `Int` because the value exceeds 32 bits, which is the width
    /// GraphQL's `Int` guarantees. It is a counter, not a timestamp: compare it for
    /// inequality to detect a change, do not read it as a time.
    async fn content_changed_at(&self) -> String {
        self.section.content_changed_at.to_string()
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
            desc = "Discard this many items before the page begins. Prefer cursors: an offset costs the database a traversal of every discarded row, and a concurrent insert shifts it. Provided because a client that renders a scrollbar needs to jump to an absolute position, which no cursor can express."
        )]
        offset: Option<u32>,
    ) -> Result<ItemConnection> {
        let item_query = self.item_query(filter, order_by, direction);
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
                connection.edges.extend(page.items.into_iter().map(|item| {
                    // The cursor carries the ordering key *and* the id: two films can share
                    // a sort title, and without the tiebreaker a page boundary would skip
                    // or repeat one.
                    let cursor = Cursor::new(item_query.cursor_key(&item), item.id.get());
                    Edge::new(OpaqueCursor(cursor), Item::from_domain(item))
                }));
                Ok::<_, async_graphql::Error>(connection)
            },
        )
        .await
    }

    /// Aggregates over the contents of this library.
    async fn stats(
        &self,
        #[graphql(desc = "Count only the items this filter keeps.")] filter: Option<
            ItemFilterInput,
        >,
    ) -> LibraryStats {
        LibraryStats {
            query: self.item_query(filter, None, None),
        }
    }
}

impl Library {
    /// Builds the storage query from the arguments a client supplied.
    fn item_query(
        &self,
        filter: Option<ItemFilterInput>,
        order_by: Option<ItemOrder>,
        direction: Option<SortDirection>,
    ) -> ItemQuery {
        ItemQuery::new(self.section.id)
            .filtered(filter.unwrap_or_default().into())
            .sorted_by(
                order_by.unwrap_or_default().into(),
                direction.unwrap_or_default().into(),
            )
    }
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
            let page = schema::library_repository(context)?
                .sections(window)
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
