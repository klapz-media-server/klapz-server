//! Play queues on the official API.

// Resolvers are `async` even when they only read a field: async-graphql requires that shape,
// and `unused_async` does not know it.
#![allow(clippy::unused_async)]

use async_graphql::connection::{query, Connection, Edge, EmptyFields};
use async_graphql::dataloader::DataLoader;
use async_graphql::{Context, InputObject, Object, Result, ID};
use open_plex_core::page::Cursor;
use open_plex_core::play_queue::{NewPlayQueue, PlayQueue, PlayQueueEntry, PlayQueueRepository};
use open_plex_core::{now_epoch, RatingKey};

use crate::cursor::OpaqueCursor;
use crate::item::Item;
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

/// A list of what to play, as it was when it was created.
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

    /// Whether the entries are in random order.
    async fn shuffled(&self) -> bool {
        self.0.shuffled
    }

    /// Raised each time the queue changes.
    async fn version(&self) -> i32 {
        self.0.version
    }

    /// How many entries the queue holds.
    async fn total_count(&self) -> u64 {
        self.0.total
    }

    /// The entry to play first. Null for an empty queue.
    async fn selected_entry(&self) -> Option<PlayQueueEntryObject> {
        self.0.selected.map(PlayQueueEntryObject)
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

    /// The queue this entry belongs to, as returned in `PlayQueue.id`.
    async fn queue_id(&self) -> ID {
        ID(GlobalId::encode(PLAY_QUEUE_TYPE, self.0.queue))
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
    /// What to play, in order, as returned in `Item.id`. May be empty.
    pub item_ids: Vec<ID>,
    /// The position of the entry to play first.
    pub selected_position: u32,
    /// Put the entries in random order, playing whichever lands first.
    pub shuffle: bool,
}

pub(crate) async fn create(
    context: &Context<'_>,
    input: CreatePlayQueueInput,
) -> Result<PlayQueueObject> {
    let caller = crate::access::require_caller(context)?;
    let items = input
        .item_ids
        .iter()
        .map(|id| Ok(RatingKey::new(GlobalId::decode(id)?.key)))
        .collect::<Result<Vec<_>>>()?;
    for item in &items {
        if crate::schema::readable_item(context, *item)
            .await?
            .is_none()
        {
            return Err(crate::access::denied());
        }
    }
    let queue = NewPlayQueue {
        user: caller.user,
        source_uri: input.source_uri,
        selected: usize::try_from(input.selected_position).unwrap_or(0),
        items,
        shuffled: false,
    };
    let queue = if input.shuffle {
        queue.shuffle()
    } else {
        queue
    };
    Ok(PlayQueueObject(
        queues(context)?.create(queue, now_epoch()).await?,
    ))
}
