//! Schema roots, Relay plumbing, and the per-request context.

use std::sync::Arc;

use async_graphql::connection;
use async_graphql::dataloader::DataLoader;
use async_graphql::{Context, EmptySubscription, Interface, Object, Result, Schema, ID};
use open_plex_core::library::LibraryRepository;
use open_plex_core::metadata::MetadataRepository;
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
    pub collections: Arc<dyn open_plex_core::collection::CollectionRepository>,
    pub access: Arc<dyn open_plex_core::access::AccessRepository>,
    pub directory: Arc<dyn open_plex_core::access::AccountDirectory>,
    pub watch: Arc<dyn open_plex_core::watch::WatchRepository>,
    pub streams: Arc<dyn open_plex_core::stream::StreamEngine>,
    /// Shared with whatever else ends sessions outside a request: the sweep, the stream routes.
    pub session_locks: Arc<open_plex_core::lifecycle::SessionLocks>,
}

impl ApiContext {
    pub(crate) fn lifecycle(&self) -> open_plex_core::lifecycle::Lifecycle<'_> {
        open_plex_core::lifecycle::Lifecycle {
            sessions: self.playback.as_ref(),
            streams: self.streams.as_ref(),
            locks: &self.session_locks,
        }
    }
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
    /// One viewing in a history.
    HistoryEntry(crate::history::HistoryEntryObject),
    PlayQueue(crate::play_queue::PlayQueueObject),
    PlayQueueEntry(crate::play_queue::PlayQueueEntryObject),
    /// A list of what to play, kept by its owner.
    Playlist(crate::playlist::PlaylistObject),
    /// One chosen item at its place in a playlist.
    PlaylistEntry(crate::playlist::PlaylistEntryObject),
    /// Items of one library grouped under a name.
    Collection(crate::collection::CollectionObject),
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
    /// An item's intro, credits or commercial, or a place the caller bookmarked.
    Marker(crate::marker::MarkerObject),
    /// A trailer, a deleted scene or a featurette that comes with an item.
    Extra(crate::extra::ExtraObject),
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
            TAG_TYPE => Ok(crate::tag::readable_tag(context, TagId::new(global.key))
                .await?
                .map(|tag| Node::Tag(TagObject(tag)))),
            PERSON_TYPE => Ok(
                crate::tag::readable_person(context, PersonId::new(global.key))
                    .await?
                    .map(|person| Node::Person(PersonObject(person))),
            ),
            crate::play_queue::PLAY_QUEUE_TYPE => {
                Ok(crate::play_queue::queue_node(context, global.key)
                    .await?
                    .map(Node::PlayQueue))
            }
            crate::play_queue::PLAY_QUEUE_ENTRY_TYPE => {
                Ok(crate::play_queue::entry_node(context, global.key)
                    .await?
                    .map(Node::PlayQueueEntry))
            }
            crate::marker::MARKER_TYPE => Ok(crate::marker::marker_node(context, global.key)
                .await?
                .map(Node::Marker)),
            crate::extra::EXTRA_TYPE => Ok(crate::extra::extra_node(context, global.key)
                .await?
                .map(Node::Extra)),
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
            crate::collection::COLLECTION_TYPE => {
                Ok(crate::collection::collection_node(context, global.key)
                    .await?
                    .map(Node::Collection))
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
            crate::history::HISTORY_ENTRY_TYPE => Ok(crate::history::visible_entry(
                context,
                open_plex_core::HistoryId::new(global.key),
            )
            .await?
            .map(Node::HistoryEntry)),
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

    /// Items whose title matches a search term, across every library.
    ///
    /// Matches a *word prefix*: `hell` finds "Good News About Hell", and accents are folded so
    /// `soiree` finds "# Pire Soirée". A term inside a word does not match — the index is a
    /// range scan, not a substring scan. Every word of a term must begin some word of a title.
    ///
    /// Polymorphic and cross-library by design: results mix films, series and episodes from
    /// different libraries, which is what a single search box needs. Narrow with `kinds`.
    ///
    /// Ranked by default, each edge carrying its `score` and `titleMatch`: the best match
    /// first, then the audience rating, then the sort title and the id, so that the order is
    /// total and a cursor resumes exactly where its page ended. A blank term matches nothing
    /// rather than everything.
    #[allow(clippy::too_many_arguments)]
    async fn search(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Term to match a prefix of any title word against.")] query: String,
        #[graphql(desc = "Restrict to these kinds. Omit to search every kind.")] kinds: Option<
            Vec<crate::filter::ItemType>,
        >,
        #[graphql(desc = "How to order the matches.", default)]
        order: crate::search::SearchOrderInput,
        #[graphql(
            desc = "Also find an episode through its series' titles. It ranks as a match anywhere.",
            default = false
        )]
        episodes_by_series: bool,
        #[graphql(desc = "Return items after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return items before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many items from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many items from the end of the range.")] last: Option<
            i32,
        >,
    ) -> Result<crate::search::ItemSearchConnection> {
        crate::search::search(
            context,
            query,
            kinds,
            (order, episodes_by_series),
            (after, before, first, last),
        )
        .await
    }

    /// Films or series found through the people a search term names, rather than through
    /// their titles: what a search box offers for a name.
    ///
    /// The first actor, then the first director, of the people `searchTags` would offer for
    /// the term — ranked by how well the name answers it, then by library title, then the
    /// most credited, and cut to `first` — whose library holds `kind`. Of the `first` items
    /// their credit was put on last in that library, those of `kind`: the actor's, then the
    /// director's not already listed, `first` in all; then ranked by `score`. Empty for a
    /// kind other than films and series.
    ///
    /// A bounded list rather than a range to page through: which items it holds depends on
    /// `first`.
    async fn search_credits(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Term to match a prefix of any word of a person's name against.")]
        query: String,
        #[graphql(desc = "What to list: films or series.")] kind: crate::filter::ItemType,
        #[graphql(desc = "Return at most this many items.")] first: Option<i32>,
    ) -> Result<crate::search::CreditedItemConnection> {
        crate::search::credited(context, query, kind, first).await
    }

    /// Every tag of one kind that an item of the caller's libraries carries — a film, a
    /// series, a season or an episode, hidden libraries included — in the order the tags
    /// were first recorded. A tag nothing carries any more is left out, and so is one only
    /// libraries the caller cannot read carry.
    #[allow(clippy::too_many_arguments)]
    async fn tags(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Which kind of tag to list.")] kind: crate::tag::TagType,
        #[graphql(desc = "Return tags after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return tags before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many tags from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many tags from the end of the range.")] last: Option<
            i32,
        >,
        #[graphql(
            desc = "Discard this many tags before the page begins. Prefer cursors: an offset costs a traversal of every discarded row."
        )]
        offset: Option<u32>,
    ) -> Result<crate::tag::TagListConnection> {
        crate::tag::tags_of_kind(context, kind, (after, before, first, last), offset).await
    }

    /// Genres and people whose name matches a search term, once per library carrying them.
    ///
    /// Matches as `search` does: every word of the term begins a word of the name, accents
    /// folded, so `hanks` and `tom han` both find Tom Hanks. A person credited in two roles
    /// is found twice, as an actor and as a director, and each in every library whose films
    /// or series credit them: one crediting them on episodes alone is left out, since its
    /// listing filtered on them would be empty.
    ///
    /// Ordered by how many items carry the tag, most first: a common first name matches
    /// thousands of people, and the ones a library is full of are the ones a search box
    /// should offer. A blank term matches nothing.
    #[allow(clippy::too_many_arguments)]
    async fn search_tags(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Term to match a prefix of any word of the name against.")] query: String,
        #[graphql(desc = "Restrict to these kinds. Omit to search every kind.")] kinds: Option<
            Vec<crate::tag::TagType>,
        >,
        #[graphql(desc = "Return tags after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return tags before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many tags from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many tags from the end of the range.")] last: Option<
            i32,
        >,
    ) -> Result<crate::tag::TagUseConnection> {
        crate::tag::search_tags(context, query, kinds, after, before, first, last).await
    }

    /// Items whose title is closest to a term, closest first: what a voice assistant
    /// searches with, since the words it hears are rarely the words of a title.
    ///
    /// Ranks by edit distance between the term and each item's sort title, falling back to
    /// its title. Both are folded first: ASCII case, the accents of the Latin-1 letters, the
    /// carons and the lower-case macrons; anything else counts for each byte of its UTF-8
    /// form. One leading article is dropped from each (`the`, `das`, `der`, `a`, `an`, `el`,
    /// `la`), so `the office` is 0 from "The Office", sorted as "Office". Every title is
    /// ranked, however far: the distance says how good a match is. Ties follow by folded
    /// title, then by id. A term is at most 256 bytes long.
    #[allow(clippy::too_many_arguments)]
    async fn closest_titles(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The term, as heard or typed.")] query: String,
        #[graphql(desc = "The kinds of item to rank. Music kinds rank nothing.")] kinds: Vec<
            crate::filter::ItemType,
        >,
        #[graphql(desc = "Return matches after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return matches before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many matches, the closest first.")] first: Option<
            i32,
        >,
        #[graphql(
            desc = "Return at most this many matches from the far end: the farthest, which costs a pass over every title."
        )]
        last: Option<i32>,
    ) -> Result<crate::closest::ClosestTitleConnection> {
        crate::closest::closest_titles(context, query, kinds, after, before, first, last).await
    }

    /// What is being played right now, oldest first, or what an owner terminated. A session
    /// a player has stopped keeping alive disappears by itself.
    async fn playback_sessions(
        &self,
        context: &Context<'_>,
        #[graphql(
            desc = "Which sessions to list: those playing unless asked otherwise.",
            default
        )]
        status: crate::playback::PlaybackSessionStatusType,
        #[graphql(desc = "Return sessions after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return sessions before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many sessions from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many sessions from the end of the range.")]
        last: Option<i32>,
    ) -> Result<crate::playback::PlaybackConnection> {
        crate::playback::sessions(context, status, after, before, first, last).await
    }

    /// What was watched, one entry per viewing: each playback that reached 90% of the
    /// runtime, and each film or episode marked watched. A position kept is no viewing, and
    /// marking an item unwatched leaves its history as it is.
    ///
    /// A user reads its own; an owner reads everyone's, its own with `mine`. Empty for a
    /// request with no caller. In the order recorded unless asked otherwise.
    // One Rust parameter per field argument; see the note on `Library.items`.
    #[allow(clippy::too_many_arguments)]
    async fn viewing_history(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Narrows the history. Every field is combined with `AND`.")]
        filter: Option<crate::history::HistoryFilterInput>,
        #[graphql(desc = "How to order the history.")] order_by: Option<
            crate::history::HistoryOrderType,
        >,
        #[graphql(desc = "Which direction to order in.")] direction: Option<
            crate::filter::SortDirection,
        >,
        #[graphql(desc = "Return entries after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return entries before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many entries from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many entries from the end of the range.")]
        last: Option<i32>,
        #[graphql(
            desc = "Discard this many entries before the page begins, for a client that pages by position. It costs the entries it discards."
        )]
        offset: Option<u32>,
    ) -> Result<crate::history::HistoryConnection> {
        crate::history::viewing_history(
            context, filter, order_by, direction, after, before, first, last, offset,
        )
        .await
    }

    /// The caller's own playlists. Empty for a request with no caller.
    // One Rust parameter per field argument; see the note on `Library.items`.
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
        let request = crate::playlist::ListRequest {
            kind,
            smart,
            library,
            order_by,
            direction,
        };
        crate::playlist::list(context, request, (after, before, first, last), offset).await
    }

    /// Collections across every library the caller reads; `Library.collections` lists one
    /// library's.
    // One Rust parameter per field argument; see the note on `Library.items`.
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
            desc = "Only the collections carried under this `Collection.tagNumber`: one per library at most."
        )]
        tag_number: Option<String>,
        #[graphql(desc = "How to order them.")] order_by: Option<
            crate::collection::CollectionListOrder,
        >,
        #[graphql(desc = "Which direction to order in.")] direction: Option<
            crate::filter::SortDirection,
        >,
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
            with_entries: None,
            tag_number,
            matching,
            order_by,
            direction,
        };
        let sections = crate::collection::readable_sections(context).await?;
        crate::collection::list(
            context,
            sections,
            request,
            (after, before, first, last),
            offset,
        )
        .await
    }

    /// Who is asking: a user, or no one.
    async fn viewer(&self, context: &Context<'_>) -> crate::access::Viewer {
        crate::access::viewer(context)
    }

    /// The account the caller's connection is paired with on the extension's own service.
    /// Null when it is not paired, or for a request with no caller.
    async fn viewer_account(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "The protocol concerned.")] extension: crate::access::ExtensionType,
    ) -> Result<Option<crate::access::AccountLinkObject>> {
        crate::access::viewer_account(context, extension).await
    }

    /// The people who can be given access to this server. For its owners only.
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

/// A loader whose batches run as tasks of their own.
fn loader<T: async_graphql::dataloader::Loader<K>, K>(batcher: T) -> DataLoader<T>
where
    K: Send + Sync + std::hash::Hash + Eq + Clone + 'static,
{
    DataLoader::new(batcher, tokio::spawn)
}

/// Builds the schema with its context and loaders attached.
pub fn build_schema(context: ApiContext) -> ApiSchema {
    let library = Arc::clone(&context.library);
    let metadata = Arc::clone(&context.metadata);
    let watch = Arc::clone(&context.watch);
    let playlists = Arc::clone(&context.playlists);
    let collections = Arc::clone(&context.collections);

    Schema::build(Query, crate::playback::Mutation, EmptySubscription)
        .data(context)
        .data(loader(RootsLoader::new(Arc::clone(&library))))
        .data(loader(LibraryLoader::new(library)))
        .data(loader(ItemLoader::new(Arc::clone(&metadata))))
        .data(loader(crate::loader::LockedFieldsLoader::new(Arc::clone(
            &metadata,
        ))))
        .data(loader(crate::backdrop::CornerColorsLoader::new(
            Arc::clone(&metadata),
        )))
        .data(loader(crate::marker::MarkersLoader::new(Arc::clone(
            &metadata,
        ))))
        .data(loader(crate::extra::ExtrasLoader::new(Arc::clone(
            &metadata,
        ))))
        .data(loader(crate::extra::ExtraVersionsLoader::new(Arc::clone(
            &metadata,
        ))))
        .data(loader(VersionsLoader::new(Arc::clone(&metadata))))
        .data(loader(FilesLoader::new(Arc::clone(&metadata))))
        .data(loader(StreamsLoader::new(Arc::clone(&metadata))))
        .data(loader(ChildCountLoader::new(Arc::clone(&metadata))))
        .data(loader(LeafCountLoader::new(Arc::clone(&metadata))))
        .data(loader(TagsLoader::new(Arc::clone(&metadata))))
        .data(loader(CreditsBelowLoader::new(Arc::clone(&metadata))))
        .data(loader(crate::loader::PersonCreditsLoader::new(Arc::clone(
            &metadata,
        ))))
        .data(loader(ExternalIdsLoader::new(Arc::clone(&metadata))))
        .data(loader(RatingsLoader::new(metadata)))
        .data(loader(crate::watch::WatchStateLoader::new(Arc::clone(
            &watch,
        ))))
        .data(loader(crate::watch::TrackChoiceLoader::new(Arc::clone(
            &watch,
        ))))
        .data(loader(crate::user_rating::UserRatingLoader::new(
            Arc::clone(&watch),
        )))
        .data(loader(crate::sidecar::SubtitleOffsetLoader::new(
            Arc::clone(&watch),
        )))
        .data(loader(crate::watch::WatchedLeavesLoader::new(watch)))
        .data(loader(crate::playlist::PlaylistTotalsLoader::new(
            playlists,
        )))
        .data(loader(crate::collection::CollectionTotalsLoader::new(
            Arc::clone(&collections),
        )))
        .data(loader(crate::collection::MembershipsLoader::new(
            collections,
        )))
        .finish()
}
