//! Play queues on the official API.
//!
//! A queue is its user's, and an owner of the server reaches everyone's; anyone else's
//! answers as nothing. Its entries stay under their libraries' rights. Every edit follows the
//! rules `core::play_queue` holds, measured on the reference.

// Resolvers are `async` even when they only read a field: async-graphql requires that shape,
// and `unused_async` does not know it.
#![allow(clippy::unused_async)]

use async_graphql::connection::{query, Connection, Edge, EmptyFields};
use async_graphql::dataloader::DataLoader;
use async_graphql::{Context, Enum, InputObject, Object, Result, SimpleObject, ID};
use open_plex_core::page::Cursor;
use open_plex_core::play_queue::{
    ChangeRefusal, NewPlayQueue, PlayQueue, PlayQueueChange, PlayQueueEntry, PlayQueueKind,
    PlayQueueRepository,
};
use open_plex_core::{now_epoch, RatingKey};

use crate::cursor::OpaqueCursor;
use crate::item::{Item, ITEM_TYPE};
use crate::loader::ItemLoader;
use crate::node::GlobalId;
use crate::schema::ApiContext;

pub const PLAY_QUEUE_TYPE: &str = "PlayQueue";
pub const PLAY_QUEUE_ENTRY_TYPE: &str = "PlayQueueEntry";
const DEFAULT_PAGE_SIZE: u32 = 20;

pub type PlayQueueEntryConnection =
    Connection<OpaqueCursor, PlayQueueEntryObject, EmptyFields, EmptyFields>;

pub(crate) fn queues<'c>(context: &Context<'c>) -> Result<&'c dyn PlayQueueRepository> {
    Ok(context.data::<ApiContext>()?.play_queues.as_ref())
}

/// The caller's queue with this number: another viewer's is not there.
pub(crate) async fn queue_node(context: &Context<'_>, key: i64) -> Result<Option<PlayQueueObject>> {
    let caller = crate::access::require_caller(context)?;
    Ok(queues(context)?
        .queue(key)
        .await?
        .filter(|queue| caller.owns(queue.user))
        .map(PlayQueueObject))
}

/// An entry of one of the caller's queues.
pub(crate) async fn entry_node(
    context: &Context<'_>,
    key: i64,
) -> Result<Option<PlayQueueEntryObject>> {
    let caller = crate::access::require_caller(context)?;
    let queues = queues(context)?;
    let Some(entry) = queues.entry(key).await? else {
        return Ok(None);
    };
    let owned = queues
        .queue(entry.queue)
        .await?
        .is_some_and(|queue| caller.owns(queue.user));
    Ok(owned.then_some(PlayQueueEntryObject(entry)))
}

/// What a queue plays. A player keeps one queue of each kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
#[graphql(name = "PlayQueueKind")]
pub enum PlayQueueKindType {
    /// Films and episodes.
    Video,
    /// Music. Nothing here holds any.
    Audio,
    /// Photographs. Nothing here holds any.
    Photo,
}

impl From<PlayQueueKind> for PlayQueueKindType {
    fn from(kind: PlayQueueKind) -> Self {
        match kind {
            PlayQueueKind::Video => Self::Video,
            PlayQueueKind::Audio => Self::Audio,
            PlayQueueKind::Photo => Self::Photo,
        }
    }
}

impl From<PlayQueueKindType> for PlayQueueKind {
    fn from(kind: PlayQueueKindType) -> Self {
        match kind {
            PlayQueueKindType::Video => Self::Video,
            PlayQueueKindType::Audio => Self::Audio,
            PlayQueueKindType::Photo => Self::Photo,
        }
    }
}

/// A list of what to play next: built from a source, then edited.
#[derive(Debug, Clone)]
pub struct PlayQueueObject(pub PlayQueue);

#[Object(name = "PlayQueue")]
impl PlayQueueObject {
    /// Globally unique, opaque identifier. Pass it to `node` to refetch this queue.
    pub async fn id(&self) -> ID {
        ID(GlobalId::encode(PLAY_QUEUE_TYPE, self.0.id))
    }

    /// Where the items came from, as the client named it.
    async fn source_uri(&self) -> &str {
        &self.0.source_uri
    }

    /// What it plays.
    async fn kind(&self) -> PlayQueueKindType {
        self.0.kind.into()
    }

    /// The player it was made for, as it identifies itself. Null for none in particular.
    async fn client_identifier(&self) -> Option<&str> {
        self.0.client.as_deref()
    }

    /// Whether the entries are in random order.
    async fn shuffled(&self) -> bool {
        self.0.shuffled
    }

    /// Raised each time the queue changes, except by a reset.
    async fn version(&self) -> i32 {
        self.0.version
    }

    /// How many entries the queue holds.
    async fn total_count(&self) -> u64 {
        self.0.total
    }

    /// The playlist it plays. Null when it plays none, or one deleted since.
    async fn playlist(
        &self,
        context: &Context<'_>,
    ) -> Result<Option<crate::playlist::PlaylistObject>> {
        match self.0.playlist {
            Some(playlist) => crate::playlist::playlist_node(context, playlist.get()).await,
            None => Ok(None),
        }
    }

    /// The entry playing. Null for an empty queue.
    async fn selected_entry(&self) -> Option<PlayQueueEntryObject> {
        self.0.selected.map(PlayQueueEntryObject)
    }

    /// The last entry queued by hand, after which the next addition goes. Null when there is
    /// none: only then can the queue be shuffled.
    async fn last_added_entry(&self) -> Option<PlayQueueEntryObject> {
        self.0.last_added.map(PlayQueueEntryObject)
    }

    /// The entries, in play order.
    async fn entries(
        &self,
        context: &Context<'_>,
        #[graphql(desc = "Return entries after this cursor.")] after: Option<String>,
        #[graphql(desc = "Return entries before this cursor.")] before: Option<String>,
        #[graphql(desc = "Return at most this many entries from the start of the range.")]
        first: Option<i32>,
        #[graphql(desc = "Return at most this many entries from the end of the range.")]
        last: Option<i32>,
        #[graphql(desc = "Skip this many entries first, to land in the middle of the queue.")]
        offset: Option<u32>,
    ) -> Result<PlayQueueEntryConnection> {
        let queue = self.0.id;
        query(
            after,
            before,
            first,
            last,
            |after, before, first, last| async move {
                let window =
                    crate::schema::window_for(after, before, first, last, DEFAULT_PAGE_SIZE)?
                        .skipping(u64::from(offset.unwrap_or(0)));
                let page = queues(context)?.entries(queue, window).await?;
                let mut connection = Connection::new(page.has_previous, page.has_next);
                connection.edges.extend(page.items.into_iter().map(|entry| {
                    Edge::new(
                        OpaqueCursor(Cursor::by_id(entry.position)),
                        PlayQueueEntryObject(entry),
                    )
                }));
                Ok::<_, async_graphql::Error>(connection)
            },
        )
        .await
    }
}

/// One position in a play queue.
#[derive(Debug, Clone, Copy)]
pub struct PlayQueueEntryObject(pub PlayQueueEntry);

#[Object(name = "PlayQueueEntry")]
impl PlayQueueEntryObject {
    /// Globally unique, opaque identifier. Pass it to `node` to refetch this entry.
    pub async fn id(&self) -> ID {
        ID(GlobalId::encode(PLAY_QUEUE_ENTRY_TYPE, self.0.id))
    }

    /// The queue this entry belongs to.
    async fn queue(&self, context: &Context<'_>) -> Result<Option<PlayQueueObject>> {
        queue_node(context, self.0.queue).await
    }

    /// Its place in the queue, from zero.
    async fn position(&self) -> i64 {
        self.0.position
    }

    /// What it plays. Null if the item has left the library since.
    async fn item(&self, context: &Context<'_>) -> Result<Option<Item>> {
        Ok(context
            .data::<DataLoader<ItemLoader>>()?
            .load_one(self.0.item)
            .await?
            .filter(|item| crate::access::can_read(context, item.section_id))
            .map(Item::from_domain))
    }
}

/// A play queue to create.
#[derive(Debug, InputObject)]
pub struct CreatePlayQueueInput {
    /// Where the items come from, as the client named it.
    pub source_uri: String,
    /// What to play, in natural order, as returned in `Item.id`. May be empty. Shuffling
    /// rebuilds the queue from these; at most 500 are kept.
    pub item_ids: Vec<ID>,
    /// The position of the entry to play first.
    pub selected_position: u32,
    /// Put the entries in random order, the selected one first.
    pub shuffle: bool,
    /// The caller's playlist these items come from, as returned in `Playlist.id`: the queue
    /// names it, and it counts one more play.
    pub playlist_id: Option<ID>,
    /// The player the queue is for, as it identifies itself. Its previous queue of the same
    /// kind is deleted. Omitted, the queue replaces nothing.
    pub client_identifier: Option<String>,
    /// What the queue plays.
    #[graphql(default_with = "PlayQueueKindType::Video")]
    pub kind: PlayQueueKindType,
}

fn item_key(id: &ID) -> Option<RatingKey> {
    GlobalId::decode(id)
        .ok()
        .filter(|decoded| decoded.type_name == ITEM_TYPE)
        .map(|decoded| RatingKey::new(decoded.key))
}

/// The items an input names, each one the caller can read, or `None` at the first that is not.
async fn readable_items(context: &Context<'_>, ids: &[ID]) -> Result<Option<Vec<RatingKey>>> {
    let mut items = Vec::with_capacity(ids.len());
    for id in ids {
        let Some(key) = item_key(id) else {
            return Ok(None);
        };
        if crate::schema::readable_item(context, key).await?.is_none() {
            return Ok(None);
        }
        items.push(key);
    }
    Ok(Some(items))
}

pub(crate) async fn create(
    context: &Context<'_>,
    input: CreatePlayQueueInput,
) -> Result<PlayQueueObject> {
    let caller = crate::access::require_caller(context)?;
    let items = readable_items(context, &input.item_ids)
        .await?
        .ok_or_else(crate::access::denied)?;
    let playlist = match &input.playlist_id {
        Some(id) => {
            let playlist = GlobalId::decode(id)
                .ok()
                .filter(|decoded| decoded.type_name == crate::playlist::PLAYLIST_TYPE)
                .map(|decoded| open_plex_core::PlaylistId::new(decoded.key))
                .ok_or_else(crate::access::denied)?;
            if !crate::playlist::record_play(context, playlist).await? {
                return Err(crate::access::denied());
            }
            Some(playlist)
        }
        None => None,
    };
    let queue = NewPlayQueue {
        playlist,
        user: caller.user,
        client: input.client_identifier,
        kind: input.kind.into(),
        source_uri: input.source_uri,
        selected: usize::try_from(input.selected_position).unwrap_or(0),
        source: items,
        shuffle: input.shuffle,
    };
    Ok(PlayQueueObject(
        queues(context)?.create(queue, now_epoch()).await?,
    ))
}

/// Why a play queue edit did nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum PlayQueueErrorCode {
    /// No queue of the caller's carries this id.
    PlayQueueNotFound,
    /// No entry of a queue of the caller's carries this id.
    EntryNotFound,
    /// No item the caller can read carries this id.
    ItemNotFound,
    /// The entry to move after is not in the same queue.
    AfterNotFound,
    /// An entry cannot be moved after itself.
    OntoItself,
    /// Entries were queued by hand (`PlayQueue.lastAddedEntry`): such a queue keeps its
    /// order, and cannot be shuffled or unshuffled.
    UpNext,
    /// The queue is empty: there is nothing to reorder.
    Empty,
}

/// A request a play queue edit refused, and where in the input the reason lies.
#[derive(Debug, Clone, SimpleObject)]
pub struct PlayQueueUserError {
    /// The path to the input field at fault.
    pub field: Vec<String>,
    /// What went wrong, for a person to read.
    pub message: String,
    /// What went wrong, for a program to branch on.
    pub code: PlayQueueErrorCode,
}

impl PlayQueueUserError {
    fn new(field: &str, code: PlayQueueErrorCode) -> Self {
        let message = match code {
            PlayQueueErrorCode::PlayQueueNotFound => "no such play queue",
            PlayQueueErrorCode::EntryNotFound => "no such entry in this play queue",
            PlayQueueErrorCode::ItemNotFound => "no such item",
            PlayQueueErrorCode::AfterNotFound => "the entry to move after is not in this queue",
            PlayQueueErrorCode::OntoItself => "an entry cannot be moved after itself",
            PlayQueueErrorCode::UpNext => "entries queued by hand keep this queue in its order",
            PlayQueueErrorCode::Empty => "this play queue is empty",
        };
        Self {
            field: vec!["input".to_owned(), field.to_owned()],
            message: message.to_owned(),
            code,
        }
    }

    fn of(refusal: ChangeRefusal, field: &str) -> Self {
        match refusal {
            ChangeRefusal::EntryNotFound => Self::new(field, PlayQueueErrorCode::EntryNotFound),
            ChangeRefusal::AfterNotFound => {
                Self::new("afterEntryId", PlayQueueErrorCode::AfterNotFound)
            }
            ChangeRefusal::OntoItself => Self::new("afterEntryId", PlayQueueErrorCode::OntoItself),
            ChangeRefusal::UpNext => Self::new(field, PlayQueueErrorCode::UpNext),
            ChangeRefusal::Empty => Self::new(field, PlayQueueErrorCode::Empty),
        }
    }
}

type Refusal<T> = std::result::Result<T, PlayQueueUserError>;

/// A play queue after an edit.
#[derive(Debug, SimpleObject)]
pub struct PlayQueuePayload {
    /// As given in the input.
    pub client_mutation_id: Option<String>,
    /// The queue as it now stands. Null when the request was refused.
    pub play_queue: Option<PlayQueueObject>,
    /// Why the request was refused. Empty on success.
    pub user_errors: Vec<PlayQueueUserError>,
}

impl PlayQueuePayload {
    fn of(client_mutation_id: Option<String>, outcome: Refusal<PlayQueue>) -> Self {
        match outcome {
            Ok(queue) => Self {
                client_mutation_id,
                play_queue: Some(PlayQueueObject(queue)),
                user_errors: Vec::new(),
            },
            Err(error) => Self {
                client_mutation_id,
                play_queue: None,
                user_errors: vec![error],
            },
        }
    }
}

/// The key of one of the caller's queues, or a refusal naming `field`.
async fn owned_queue(context: &Context<'_>, id: &ID, field: &str) -> Result<Refusal<i64>> {
    let caller = crate::access::require_caller(context)?;
    let not_found = || PlayQueueUserError::new(field, PlayQueueErrorCode::PlayQueueNotFound);
    let Some(key) = GlobalId::decode(id)
        .ok()
        .filter(|decoded| decoded.type_name == PLAY_QUEUE_TYPE)
        .map(|decoded| decoded.key)
    else {
        return Ok(Err(not_found()));
    };
    let owned = queues(context)?
        .queue(key)
        .await?
        .is_some_and(|queue| caller.owns(queue.user));
    Ok(if owned { Ok(key) } else { Err(not_found()) })
}

/// The queue holding an entry of the caller's, and the entry's key.
async fn owned_entry(context: &Context<'_>, id: &ID, field: &str) -> Result<Refusal<(i64, i64)>> {
    let caller = crate::access::require_caller(context)?;
    let not_found = || PlayQueueUserError::new(field, PlayQueueErrorCode::EntryNotFound);
    let Some(key) = GlobalId::decode(id)
        .ok()
        .filter(|decoded| decoded.type_name == PLAY_QUEUE_ENTRY_TYPE)
        .map(|decoded| decoded.key)
    else {
        return Ok(Err(not_found()));
    };
    let repository = queues(context)?;
    let Some(entry) = repository.entry(key).await? else {
        return Ok(Err(not_found()));
    };
    let owned = repository
        .queue(entry.queue)
        .await?
        .is_some_and(|queue| caller.owns(queue.user));
    Ok(if owned {
        Ok((entry.queue, key))
    } else {
        Err(not_found())
    })
}

async fn apply(
    context: &Context<'_>,
    queue: i64,
    change: &PlayQueueChange,
    field: &str,
) -> Result<Refusal<PlayQueue>> {
    Ok(match queues(context)?.change(queue, change).await? {
        Some(Ok(queue)) => Ok(queue),
        Some(Err(refusal)) => Err(PlayQueueUserError::of(refusal, field)),
        // Deleted in between: replaced, or swept.
        None => Err(PlayQueueUserError::new(
            field,
            PlayQueueErrorCode::PlayQueueNotFound,
        )),
    })
}

/// Items to queue by hand.
#[derive(Debug, InputObject)]
pub struct AddToPlayQueueInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The queue, as returned in `PlayQueue.id`.
    pub play_queue_id: ID,
    /// The items, in order, as returned in `Item.id`. A queue may hold an item several
    /// times; what would take it past 500 entries is left out.
    pub item_ids: Vec<ID>,
    /// Play them right after the entry playing. Otherwise they go after the last entry
    /// queued by hand, or right after the entry playing when there is none.
    #[graphql(default)]
    pub next: bool,
}

pub(crate) async fn add(
    context: &Context<'_>,
    input: AddToPlayQueueInput,
) -> Result<PlayQueuePayload> {
    let outcome = async {
        let queue = match owned_queue(context, &input.play_queue_id, "playQueueId").await? {
            Ok(queue) => queue,
            Err(error) => return Ok(Err(error)),
        };
        let Some(items) = readable_items(context, &input.item_ids).await? else {
            return Ok(Err(PlayQueueUserError::new(
                "itemIds",
                PlayQueueErrorCode::ItemNotFound,
            )));
        };
        let change = PlayQueueChange::Add {
            items,
            next: input.next,
        };
        apply(context, queue, &change, "playQueueId").await
    }
    .await?;
    Ok(PlayQueuePayload::of(input.client_mutation_id, outcome))
}

/// An entry to move within its queue.
#[derive(Debug, InputObject)]
pub struct MovePlayQueueEntryInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The entry, as returned in `PlayQueueEntry.id`.
    pub entry_id: ID,
    /// The entry to put it after, of the same queue. Omitted, it goes first. Put right after
    /// the last entry queued by hand, it becomes that last entry.
    pub after_entry_id: Option<ID>,
}

pub(crate) async fn move_entry(
    context: &Context<'_>,
    input: MovePlayQueueEntryInput,
) -> Result<PlayQueuePayload> {
    let outcome = async {
        let (queue, entry) = match owned_entry(context, &input.entry_id, "entryId").await? {
            Ok(found) => found,
            Err(error) => return Ok(Err(error)),
        };
        let after = match &input.after_entry_id {
            Some(id) => {
                let Some(decoded) = GlobalId::decode(id)
                    .ok()
                    .filter(|decoded| decoded.type_name == PLAY_QUEUE_ENTRY_TYPE)
                else {
                    return Ok(Err(PlayQueueUserError::new(
                        "afterEntryId",
                        PlayQueueErrorCode::AfterNotFound,
                    )));
                };
                Some(decoded.key)
            }
            None => None,
        };
        let change = PlayQueueChange::Move { entry, after };
        apply(context, queue, &change, "entryId").await
    }
    .await?;
    Ok(PlayQueuePayload::of(input.client_mutation_id, outcome))
}

/// An entry to take out of its queue.
#[derive(Debug, InputObject)]
pub struct RemoveFromPlayQueueInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The entry, as returned in `PlayQueueEntry.id`. Removing the entry playing makes the
    /// one after it play, or the one before when it was the last.
    pub entry_id: ID,
}

pub(crate) async fn remove(
    context: &Context<'_>,
    input: RemoveFromPlayQueueInput,
) -> Result<PlayQueuePayload> {
    let outcome = async {
        let (queue, entry) = match owned_entry(context, &input.entry_id, "entryId").await? {
            Ok(found) => found,
            Err(error) => return Ok(Err(error)),
        };
        apply(
            context,
            queue,
            &PlayQueueChange::Remove { entry },
            "entryId",
        )
        .await
    }
    .await?;
    Ok(PlayQueuePayload::of(input.client_mutation_id, outcome))
}

/// A whole queue to act on.
#[derive(Debug, InputObject)]
pub struct PlayQueueInput {
    /// Echoed back in the payload, for the client to match the two.
    pub client_mutation_id: Option<String>,
    /// The queue, as returned in `PlayQueue.id`.
    pub play_queue_id: ID,
}

pub(crate) async fn edit_whole(
    context: &Context<'_>,
    input: PlayQueueInput,
    change: PlayQueueChange,
) -> Result<PlayQueuePayload> {
    let outcome = async {
        match owned_queue(context, &input.play_queue_id, "playQueueId").await? {
            Ok(queue) => apply(context, queue, &change, "playQueueId").await,
            Err(error) => Ok(Err(error)),
        }
    }
    .await?;
    Ok(PlayQueuePayload::of(input.client_mutation_id, outcome))
}
