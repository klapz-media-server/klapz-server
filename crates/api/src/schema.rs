//! Schema roots, Relay plumbing, and the per-request context.

use std::sync::Arc;

use async_graphql::connection;
use async_graphql::dataloader::DataLoader;
use async_graphql::{Context, EmptySubscription, Interface, Object, Result, Schema, ID};
use open_plex_core::library::LibraryRepository;
use open_plex_core::metadata::{MetadataRepository, SearchQuery};
use open_plex_core::page::{Cursor, Window, WindowError};
use open_plex_core::{MediaId, PartId, PersonId, RatingKey, SectionId, StreamId, TagId};

use crate::cursor::OpaqueCursor;
use crate::item::{Episode, Item, Movie, Season, Show, ITEM_TYPE};
use crate::library::{Library, Root, LIBRARY_TYPE, ROOT_TYPE};
use crate::loader::{
    ChildCountLoader, CreditsBelowLoader, ExternalIdsLoader, FilesLoader, ItemLoader,
    LeafCountLoader, LibraryLoader, RatingsLoader, RootsLoader, StreamsLoader, TagsLoader,
    VersionsLoader,
};
use crate::media::{File, Track, Version, FILE_TYPE, STREAM_TYPE, VERSION_TYPE};
use crate::node::GlobalId;
use crate::server::{Server, SERVER_TYPE};
use crate::tag::{PersonObject, TagObject, PERSON_TYPE, TAG_TYPE};

pub type ApiSchema = Schema<Query, crate::playback::Mutation, EmptySubscription>;

/// Everything a resolver may reach.
///
/// Holds the repository ports rather than a database handle: this crate does not depend on
/// storage.
#[derive(Debug, Clone)]
pub struct ApiContext {
    /// What this server reports about itself. Fixed for the life of the process, so it is
    /// carried rather than queried.
    pub capabilities: open_plex_core::ServerCapabilities,
    pub library: Arc<dyn LibraryRepository>,
    pub metadata: Arc<dyn MetadataRepository>,
    pub playback: Arc<dyn open_plex_core::playback::PlaybackRepository>,
    pub play_queues: Arc<dyn open_plex_core::play_queue::PlayQueueRepository>,
    pub playlists: Arc<dyn open_plex_core::playlist::PlaylistRepository>,
    pub access: Arc<dyn open_plex_core::access::AccessRepository>,
    pub directory: Arc<dyn open_plex_core::access::AccountDirectory>,
    pub watch: Arc<dyn open_plex_core::watch::WatchRepository>,
    pub streams: Arc<dyn open_plex_core::stream::StreamEngine>,
}

/// Anything that can be refetched by its global identifier.
///
/// Relay's object identification contract. Every object with an identity implements it:
/// "some objects are refetchable and some are not" is harder to explain than "all of them
/// are", and a uniform rule costs only a routing arm per type.
#[derive(Debug, Interface)]
#[graphql(field(name = "id", ty = "ID", desc = "Globally unique, opaque identifier."))]
pub enum Node {
    /// Someone who can be given access to libraries.
    User(crate::access::UserObject),
    PlaybackSession(crate::playback::PlaybackSessionObject),
    PlayQueue(crate::play_queue::PlayQueueObject),
    PlayQueueEntry(crate::play_queue::PlayQueueEntryObject),
    /// A list of what to play, kept by its owner.
    Playlist(crate::playlist::PlaylistObject),
    /// One chosen item at its place in a playlist.
    PlaylistEntry(crate::playlist::PlaylistEntryObject),
    /// This server.
    Server(Server),
    /// A media library.
    Library(Library),
    /// A filesystem location a library scans.
    Root(Root),
    /// A film.
    Movie(Movie),
    /// A television series.
    Show(Show),
    /// A season of a series.
    Season(Season),
    /// An episode of a series.
    Episode(Episode),
    /// One encoding of an item.
    MediaVersion(Version),
    /// One file of an encoding.
    MediaFile(File),
    /// One track inside a file.
    MediaStream(Track),
    /// A genre, a country, or a person in one credit role.
    Tag(TagObject),
    /// Someone credited on an item.
    Person(PersonObject),
}

/// Default page size when the client requests neither end of a range.
///
/// Relay permits an unbounded connection; a server honoring that would have no cap on result
/// size, so one is applied here instead of refusing the query.
pub const DEFAULT_PAGE_SIZE: u32 = 50;

/// How many objects one `nodes` call may refetch.
///
/// The same reasoning as the page-size cap: a list supplied by the client needs a ceiling,
/// or one request can make the server do unbounded work.
const MAX_NODES: usize = open_plex_core::page::MAX_ITEMS as usize;

#[derive(Debug, Default)]
pub struct Query;

// Doc comments on these resolvers are the published schema descriptions, so a Rust
// `# Errors` section would leak into the schema. Failure modes are documented in the
// schema's own terms instead.
//
// Resolvers are `async` even when they only read context: async-graphql requires that shape.
#[allow(clippy::missing_errors_doc, clippy::unused_async)]
#[Object]
impl Query {
    /// Refetches any object by its global identifier.
    ///
    /// The single entry point Relay clients use to refresh cached objects. Returns null when
    /// nothing carries that identifier; a malformed identifier is an error rather than null,
    /// because it signals a client bug rather than a deleted object.
    async fn node(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "A global identifier previously returned in an `id` field.")] id: ID,
    ) -> Result<Option<Node>> {
        let global = GlobalId::decode(&id.0)?;
        match global.type_name.as_str() {
            SERVER_TYPE => Ok(Some(Node::Server(Server(
                context.data::<ApiContext>()?.capabilities.clone(),
            )))),
            crate::access::USER_TYPE => Ok(crate::access::user_node(context, global.key)
                .await?
                .map(Node::User)),
            LIBRARY_TYPE => {
                let section = SectionId::new(global.key);
                if !crate::access::can_read(context, section) {
                    return Ok(None);
                }
                Ok(load_library(context, section).await?.map(Node::Library))
            }
            ROOT_TYPE => Ok(load_root(context, global.key).await?.map(Node::Root)),
            // One tag for all four item kinds: the kind comes from storage, not the id.
            ITEM_TYPE => Ok(readable_item(context, RatingKey::new(global.key))
                .await?
                .map(|item| match Item::from_domain(item) {
                    Item::Movie(movie) => Node::Movie(movie),
                    Item::Show(show) => Node::Show(show),
                    Item::Season(season) => Node::Season(season),
                    Item::Episode(episode) => Node::Episode(episode),
                })),
            VERSION_TYPE => Ok(load_version(context, MediaId::new(global.key))
                .await?
                .map(Node::MediaVersion)),
            FILE_TYPE => Ok(load_file(context, PartId::new(global.key))
                .await?
                .map(Node::MediaFile)),
            STREAM_TYPE => Ok(load_track(context, StreamId::new(global.key))
                .await?
                .map(Node::MediaStream)),
            TAG_TYPE => Ok(metadata(context)?
                .tag(TagId::new(global.key))
                .await?
                .map(|tag| Node::Tag(TagObject(tag)))),
            PERSON_TYPE => Ok(metadata(context)?
                .person(PersonId::new(global.key))
                .await?
                .map(|person| Node::Person(PersonObject(person)))),
            crate::play_queue::PLAY_QUEUE_TYPE => {
                let caller = crate::access::require_caller(context)?;
                Ok(crate::play_queue::queues(context)?
                    .queue(global.key)
                    .await?
                    .filter(|queue| caller.owns(queue.user))
                    .map(|queue| Node::PlayQueue(crate::play_queue::PlayQueueObject(queue))))
            }
            crate::play_queue::PLAY_QUEUE_ENTRY_TYPE => {
                let caller = crate::access::require_caller(context)?;
                let queues = crate::play_queue::queues(context)?;
                let Some(entry) = queues.entry(global.key).await? else {
                    return Ok(None);
                };
                let owned = queues
                    .queue(entry.queue)
                    .await?
                    .is_some_and(|queue| caller.owns(queue.user));
                Ok(owned.then_some(Node::PlayQueueEntry(
                    crate::play_queue::PlayQueueEntryObject(entry),
                )))
            }
            crate::playlist::PLAYLIST_TYPE => {
                Ok(crate::playlist::playlist_node(context, global.key)
                    .await?
                    .map(Node::Playlist))
            }
            crate::playlist::PLAYLIST_ENTRY_TYPE => {
                Ok(crate::playlist::entry_node(context, global.key)
                    .await?
                    .map(Node::PlaylistEntry))
            }
            crate::playback::PLAYBACK_TYPE => {
                let caller = crate::access::require_caller(context)?;
                Ok(crate::playback::playback(context)?
                    .session(global.key, open_plex_core::now_epoch())
                    .await?
                    .filter(|session| caller.owns(session.user))
                    .map(|session| {
                        Node::PlaybackSession(crate::playback::PlaybackSessionObject(session))
                    }))
            }
            other => Err(async_graphql::Error::new(format!(
                "`{other}` is not an identifiable type"
            ))),
        }
    }

    /// Refetches several objects by their global identifiers, in the order given.
    ///
    /// The batched form of `node`. A client refreshing a screenful of cached objects would
    /// otherwise send one query per object, or an aliased query whose *text* grows with the
    /// request; here the count is a value. Entries are null where nothing carries that
    /// identifier, so positions line up with the input and a missing object does not shift
    /// the rest.
    ///
    /// Identifiers are deduplicated before loading, so repeating one costs nothing.
    async fn nodes(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Global identifiers previously returned in `id` fields.")] ids: Vec<ID>,
    ) -> Result<Vec<Option<Node>>> {
        if ids.len() > MAX_NODES {
            return Err(async_graphql::Error::new(format!(
                "a request for {} objects exceeds the maximum of {MAX_NODES}",
                ids.len()
            )));
        }

        // Resolved one at a time, but the loaders batch across the whole set: a lookup of
        // fifty items costs one storage query, not fifty.
        let mut resolved = Vec::with_capacity(ids.len());
        for id in ids {
            resolved.push(self.node(context, id).await?);
        }
        Ok(resolved)
    }

    /// What this server is and what it can do.
    ///
    /// The first thing a client should ask: it decides from the answer what to offer. Every
    /// optional feature reports as available, because there is no paid tier here.
    async fn server(&self, context: &Context<'_>) -> Result<Server> {
        Ok(Server(context.data::<ApiContext>()?.capabilities.clone()))
    }

    /// Items whose title matches a search term, across every library.
    ///
    /// What the caller has started and what comes next, newest first: every film in
    /// progress, and for every series the episode in progress or the one after the last
    /// watched. Played within the last sixteen weeks; every copy of a work is listed.
    ///
    /// Empty for a caller with no watch state. Pages forward only, by position: the list is
    /// built whole from the caller's recent viewing, which is bounded by those sixteen weeks.
    async fn continue_watching(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Only this library's items, as returned in `Library.id`.")]
        library: Option<ID>,
        #[graphql(desc = "Return entries after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return at most this many entries.")] first: Option<i32>,
    ) -> Result<crate::watch::ContinueWatchingConnection> {
        crate::watch::continue_watching(context, library, after, first).await
    }

    /// Matches a *word prefix*: `hell` finds "Good News About Hell", and accents are folded so
    /// `soiree` finds "# Pire Soirée". A term inside a word does not match — the index is a
    /// range scan, not a substring scan.
    ///
    /// Polymorphic and cross-library by design: results mix films, series and episodes from
    /// different libraries, which is what a single search box needs. Narrow with `kinds`.
    ///
    /// Ordered by sort title. Not by relevance: ranking needs a score this server does not
    /// compute, and a title ordering at least gives the cursor a stable position to resume
    /// from. A blank term matches nothing rather than everything.
    // One Rust parameter per field argument; see the note on `Library.items`.
    #[allow(clippy::too_many_arguments)]
    async fn search(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Term to match a prefix of any title word against.")] query: String,
        #[graphql(desc = "Restrict to these kinds. Omit to search every kind.")] kinds: Option<
            Vec<crate::filter::ItemType>,
        >,
        #[graphql(desc = "Return items after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return items before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many items from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many items from the end of the range.")] last: Option<
            i32,
        >,
    ) -> Result<crate::item::ItemConnection> {
        let mut search = SearchQuery::new(query);
        search.sections = crate::access::visible_sections(context);
        let kinds = kinds.unwrap_or_default();
        search.kinds = kinds.iter().filter_map(|kind| kind.held()).collect();
        // Every kind asked for is one this server holds none of. Not passed on as an empty
        // list, which storage reads as every kind.
        if !kinds.is_empty() && search.kinds.is_empty() {
            return Ok(connection::Connection::new(false, false));
        }

        connection::query(
            after,
            before,
            first,
            last,
            |after, before, first, last| async move {
                let window = window_for(after, before, first, last, DEFAULT_PAGE_SIZE)?;
                let page = metadata(context)?.search(&search, window).await?;

                let mut connection = connection::Connection::new(page.has_previous, page.has_next);
                connection.edges.extend(page.items.into_iter().map(|item| {
                    let cursor = Cursor::new(search.cursor_key(&item), item.id.get());
                    connection::Edge::new(OpaqueCursor(cursor), Item::from_domain(item))
                }));
                Ok::<_, async_graphql::Error>(connection)
            },
        )
        .await
    }

    /// What is being played right now, oldest first. A session a player has stopped keeping
    /// alive disappears by itself.
    async fn playback_sessions(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Return sessions after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return sessions before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many sessions from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many sessions from the end of the range.")]
        last: Option<i32>,
    ) -> Result<crate::playback::PlaybackConnection> {
        crate::playback::sessions(context, after, before, first, last).await
    }

    /// The caller's own playlists. Empty for a caller who is neither the operator nor a user.
    #[allow(clippy::too_many_arguments)]
    async fn playlists(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Only playlists of this kind.")] kind: Option<
            crate::playlist::PlaylistKindType,
        >,
        #[graphql(
            desc = "Only smart playlists (`true`), or only those of chosen items (`false`)."
        )]
        smart: Option<bool>,
        #[graphql(
            desc = "Only the playlists a library's page shows, as returned in `Library.id`: those of the kind it plays, a smart one only if its rule lists that library."
        )]
        library: Option<ID>,
        #[graphql(desc = "How to order them.")] order_by: Option<crate::playlist::PlaylistOrder>,
        #[graphql(desc = "Which direction to order in.")] direction: Option<
            crate::filter::SortDirection,
        >,
        #[graphql(desc = "Return playlists after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return playlists before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many playlists from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many playlists from the end of the range.")]
        last: Option<i32>,
        #[graphql(desc = "Skip this many playlists first.")] offset: Option<u32>,
    ) -> Result<crate::playlist::PlaylistConnection> {
        crate::playlist::list(
            context,
            kind,
            smart,
            library,
            order_by,
            direction,
            (after, before, first, last, offset),
        )
        .await
    }

    /// Who is asking: the operator, a user, or no one.
    async fn viewer(&self, context: &Context<'_>) -> crate::access::Viewer {
        crate::access::viewer(context)
    }

    /// The account the caller's connection is paired with on the extension's own service.
    /// Null for the operator, who connects through no extension.
    async fn viewer_account(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The protocol concerned.")] extension: crate::access::ExtensionType,
    ) -> Result<Option<crate::access::AccountLinkObject>> {
        crate::access::viewer_account(context, extension).await
    }

    /// The people who can be given access to this server. For its operator only.
    async fn users(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Return users after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return users before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many users from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many users from the end of the range.")] last: Option<
            i32,
        >,
    ) -> Result<
        connection::Connection<
            OpaqueCursor,
            crate::access::UserObject,
            connection::EmptyFields,
            connection::EmptyFields,
        >,
    > {
        crate::access::users(context, after, before, first, last).await
    }

    /// What the caller's libraries of one kind added most recently, newest first: the films
    /// of every film library by date added, ties in the order they arrived, or every series
    /// library's new episodes grouped as `Library.recentlyAdded` groups them, merged by date.
    ///
    /// Pages forward only, by position.
    async fn recently_added(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Which libraries: those of this kind.")] kind: crate::library::LibraryKind,
        #[graphql(
            desc = "Only this library, as returned in `Library.id`. One of another kind lists nothing."
        )]
        library: Option<ID>,
        #[graphql(desc = "Return entries after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return at most this many entries.")] first: Option<i32>,
        #[graphql(
            desc = "Skip this many entries first. A position, for a client that pages by one; it costs the entries it skips."
        )]
        offset: Option<u32>,
    ) -> Result<crate::library::RecentlyAddedConnection> {
        let kind = kind.into();
        let mut sections = crate::library::visible_of_kind(context, kind).await?;
        if let Some(id) = library {
            let only = GlobalId::decode(&id)
                .ok()
                .filter(|decoded| decoded.type_name == crate::library::LIBRARY_TYPE)
                .ok_or_else(|| async_graphql::Error::new("not a library id"))?;
            // A library not given, or of another kind, reads as one with nothing in it.
            sections.retain(|section| section.get() == only.key);
        }
        // Ties by arrival, as the reference's cross-library list has them.
        crate::library::recently_added(
            context,
            sections,
            kind,
            open_plex_core::ItemSort::Arrival,
            (after, first, offset),
        )
        .await
    }

    /// Every library the caller can see, ordered by identifier.
    ///
    /// Paginated as a Relay connection, with the window pushed down to storage rather than
    /// applied after loading. Ordering by the identifier is what makes cursors stable: adding
    /// a library never shifts the position of the others.
    async fn libraries(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Return items after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return items before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many items from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many items from the end of the range.")] last: Option<
            i32,
        >,
    ) -> Result<crate::library::LibraryConnection> {
        crate::library::libraries(context, after, before, first, last).await
    }
}

/// Translates Relay's arguments into a storage window.
///
/// # Errors
///
/// Fails when the requested page size is past the server's cap, or when both ends of the
/// range are given.
pub fn window_for(
    after: Option<OpaqueCursor>,
    before: Option<OpaqueCursor>,
    first: Option<usize>,
    last: Option<usize>,
    default_page: u32,
) -> Result<Window, WindowError> {
    // `usize` counts become `u32` because the window caps page size well below either
    // ceiling; a count that does not fit is already past the maximum and reported as such.
    let narrow = |count: Option<usize>| count.map(|value| u32::try_from(value).unwrap_or(u32::MAX));

    let (first, last) = match (narrow(first), narrow(last)) {
        (None, None) => (Some(default_page), None),
        bounds => bounds,
    };

    Window::new(
        after.map(|cursor| cursor.0),
        before.map(|cursor| cursor.0),
        first,
        last,
    )
}

/// A window over a collection ordered by identifier.
///
/// # Errors
///
/// Fails when the requested page size is past the server's cap, or when both ends of the
/// range are given.
pub fn id_window(
    after: Option<Cursor>,
    before: Option<Cursor>,
    first: Option<usize>,
    last: Option<usize>,
) -> Result<Window, WindowError> {
    let narrow = |count: Option<usize>| count.map(|value| u32::try_from(value).unwrap_or(u32::MAX));
    let (first, last) = match (narrow(first), narrow(last)) {
        (None, None) => (Some(DEFAULT_PAGE_SIZE), None),
        bounds => bounds,
    };
    Window::new(after, before, first, last)
}

pub(crate) async fn load_library(context: &Context<'_>, id: SectionId) -> Result<Option<Library>> {
    let loader = context.data::<DataLoader<LibraryLoader>>()?;
    Ok(loader.load_one(id).await?.map(Library::new))
}

/// Resolves a root by walking its library's roots.
///
/// Roots have no lookup of their own in the port: there are twenty of them in the reference
/// library, and adding a by-id query to serve a refetch nobody makes would be the wrong
/// trade. If that changes, the port grows a method.
async fn load_root(context: &Context<'_>, id: i64) -> Result<Option<Root>> {
    let library = library_repository(context)?;
    let window = id_window(None, None, Some(500), None)?;
    let visible = crate::access::visible_sections(context);
    let sections = library.sections(visible.as_deref(), window).await?;
    let ids: Vec<SectionId> = sections.items.iter().map(|section| section.id).collect();

    Ok(library
        .roots_for(&ids)
        .await?
        .into_iter()
        .find(|root| root.id == id)
        .map(Root::new))
}

async fn load_item(
    context: &Context<'_>,
    id: RatingKey,
) -> Result<Option<open_plex_core::MetadataItem>> {
    let loader = context.data::<DataLoader<ItemLoader>>()?;
    Ok(loader.load_one(id).await?)
}

/// An item, if it lies in a library the caller can see.
pub(crate) async fn readable_item(
    context: &Context<'_>,
    id: RatingKey,
) -> Result<Option<open_plex_core::MetadataItem>> {
    Ok(load_item(context, id)
        .await?
        .filter(|item| crate::access::can_read(context, item.section_id)))
}

async fn readable_version(
    context: &Context<'_>,
    id: MediaId,
) -> Result<Option<open_plex_core::MediaVersion>> {
    let Some(version) = metadata(context)?.version(id).await? else {
        return Ok(None);
    };
    Ok(readable_item(context, version.item_id)
        .await?
        .map(|_| version))
}

pub(crate) async fn readable_file(
    context: &Context<'_>,
    id: PartId,
) -> Result<Option<open_plex_core::MediaFile>> {
    let Some(file) = metadata(context)?.file(id).await? else {
        return Ok(None);
    };
    Ok(readable_version(context, file.version_id)
        .await?
        .map(|_| file))
}

async fn load_version(context: &Context<'_>, id: MediaId) -> Result<Option<Version>> {
    Ok(readable_version(context, id).await?.map(Version))
}

async fn load_file(context: &Context<'_>, id: PartId) -> Result<Option<File>> {
    Ok(readable_file(context, id).await?.map(File))
}

async fn load_track(context: &Context<'_>, id: StreamId) -> Result<Option<Track>> {
    let Some(stream) = metadata(context)?.stream(id).await? else {
        return Ok(None);
    };
    Ok(readable_file(context, stream.file_id)
        .await?
        .map(|_| Track(stream)))
}

/// The library port, for resolvers that need it directly rather than through a loader.
///
/// # Errors
///
/// Fails if the schema was built without a context, which is a wiring mistake.
pub fn library_repository<'a>(context: &Context<'a>) -> Result<&'a Arc<dyn LibraryRepository>> {
    Ok(&context.data::<ApiContext>()?.library)
}

/// The metadata port.
///
/// # Errors
///
/// Fails if the schema was built without a context, which is a wiring mistake.
pub fn metadata<'a>(context: &Context<'a>) -> Result<&'a Arc<dyn MetadataRepository>> {
    Ok(&context.data::<ApiContext>()?.metadata)
}

/// Builds the schema with its context and loaders attached.
pub fn build_schema(context: ApiContext) -> ApiSchema {
    let library = Arc::clone(&context.library);
    let metadata = Arc::clone(&context.metadata);
    let watch = Arc::clone(&context.watch);
    let playlists = Arc::clone(&context.playlists);

    Schema::build(Query, crate::playback::Mutation, EmptySubscription)
        .data(context)
        .data(DataLoader::new(
            RootsLoader::new(Arc::clone(&library)),
            tokio::spawn,
        ))
        .data(DataLoader::new(LibraryLoader::new(library), tokio::spawn))
        .data(DataLoader::new(
            ItemLoader::new(Arc::clone(&metadata)),
            tokio::spawn,
        ))
        .data(DataLoader::new(
            crate::loader::LockedFieldsLoader::new(Arc::clone(&metadata)),
            tokio::spawn,
        ))
        .data(DataLoader::new(
            VersionsLoader::new(Arc::clone(&metadata)),
            tokio::spawn,
        ))
        .data(DataLoader::new(
            FilesLoader::new(Arc::clone(&metadata)),
            tokio::spawn,
        ))
        .data(DataLoader::new(
            StreamsLoader::new(Arc::clone(&metadata)),
            tokio::spawn,
        ))
        .data(DataLoader::new(
            ChildCountLoader::new(Arc::clone(&metadata)),
            tokio::spawn,
        ))
        .data(DataLoader::new(
            LeafCountLoader::new(Arc::clone(&metadata)),
            tokio::spawn,
        ))
        .data(DataLoader::new(
            TagsLoader::new(Arc::clone(&metadata)),
            tokio::spawn,
        ))
        .data(DataLoader::new(
            CreditsBelowLoader::new(Arc::clone(&metadata)),
            tokio::spawn,
        ))
        .data(DataLoader::new(
            ExternalIdsLoader::new(Arc::clone(&metadata)),
            tokio::spawn,
        ))
        .data(DataLoader::new(RatingsLoader::new(metadata), tokio::spawn))
        .data(DataLoader::new(
            crate::watch::WatchStateLoader::new(Arc::clone(&watch)),
            tokio::spawn,
        ))
        .data(DataLoader::new(
            crate::watch::TrackChoiceLoader::new(Arc::clone(&watch)),
            tokio::spawn,
        ))
        .data(DataLoader::new(
            crate::watch::WatchedLeavesLoader::new(watch),
            tokio::spawn,
        ))
        .data(DataLoader::new(
            crate::playlist::PlaylistTotalsLoader::new(playlists),
            tokio::spawn,
        ))
        .finish()
}
