//! Stored play queues.
//!
//! An edit reads the queue whole, lets `core` apply it, and writes the entries back in their
//! new order, all in one transaction: a queue holds at most `MAX_ITEMS` entries, and the
//! reference's rules are about positions, which a rewrite keeps simple.

use klapz_core::page::{Page, Window};
use klapz_core::play_queue::{
    ChangeOutcome, NewPlayQueue, PlayQueue, PlayQueueChange, PlayQueueContents, PlayQueueEntry,
    PlayQueueError, PlayQueueKind, PlayQueueRepository, Slot,
};
use klapz_core::RatingKey;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, DatabaseConnection, DbErr,
    EntityTrait, Order, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, TransactionTrait,
};

use crate::entity::{play_queue, play_queue_entry, play_queue_source};

#[derive(Debug, Clone)]
pub struct SqlPlayQueueRepository {
    connection: DatabaseConnection,
}

impl SqlPlayQueueRepository {
    #[must_use]
    pub const fn new(connection: DatabaseConnection) -> Self {
        Self { connection }
    }
}

fn storage(error: DbErr) -> PlayQueueError {
    PlayQueueError::Storage(Box::new(error))
}

fn to_entry(row: play_queue_entry::Model) -> PlayQueueEntry {
    PlayQueueEntry {
        id: row.id,
        queue: row.queue_id,
        position: row.position,
        item: RatingKey::new(row.item_id),
    }
}

fn position(index: usize) -> i64 {
    i64::try_from(index).unwrap_or(i64::MAX)
}

/// `SQLite` refuses a statement with too many bound values.
const INSERT_BATCH: usize = 200;

async fn entry_at<C: ConnectionTrait>(
    connection: &C,
    queue: i64,
    position: Option<i64>,
) -> Result<Option<PlayQueueEntry>, PlayQueueError> {
    let Some(position) = position else {
        return Ok(None);
    };
    Ok(play_queue_entry::Entity::find()
        .filter(play_queue_entry::Column::QueueId.eq(queue))
        .filter(play_queue_entry::Column::Position.eq(position))
        .one(connection)
        .await
        .map_err(storage)?
        .map(to_entry))
}

async fn loaded<C: ConnectionTrait>(
    connection: &C,
    row: play_queue::Model,
) -> Result<PlayQueue, PlayQueueError> {
    let total = play_queue_entry::Entity::find()
        .filter(play_queue_entry::Column::QueueId.eq(row.id))
        .count(connection)
        .await
        .map_err(storage)?;
    let selected = entry_at(connection, row.id, row.selected_position).await?;
    let last_added = entry_at(connection, row.id, row.last_added_position).await?;
    Ok(PlayQueue {
        id: row.id,
        user: klapz_core::UserId::new(row.user_id),
        client: row.client,
        kind: PlayQueueKind::parse(&row.kind).unwrap_or_default(),
        source_uri: row.source_uri,
        shuffled: row.shuffled,
        version: row.version,
        total,
        selected,
        last_added,
        playlist: row.playlist_id.map(klapz_core::PlaylistId::new),
    })
}

/// Replaces a queue's entries with `slots`, in order, keeping the id of those that have one.
async fn write_entries<C: ConnectionTrait>(
    connection: &C,
    queue: i64,
    slots: &[Slot],
) -> Result<(), PlayQueueError> {
    play_queue_entry::Entity::delete_many()
        .filter(play_queue_entry::Column::QueueId.eq(queue))
        .exec(connection)
        .await
        .map_err(storage)?;
    let rows: Vec<play_queue_entry::ActiveModel> = slots
        .iter()
        .zip(0_i64..)
        .map(|(slot, position)| play_queue_entry::ActiveModel {
            id: slot.entry.map_or(ActiveValue::NotSet, ActiveValue::Set),
            queue_id: ActiveValue::Set(queue),
            position: ActiveValue::Set(position),
            item_id: ActiveValue::Set(slot.item.get()),
        })
        .collect();
    // Rows that keep their id and rows that get one cannot share a statement: an INSERT
    // names the same columns for every row.
    let (kept, new): (Vec<_>, Vec<_>) = rows
        .into_iter()
        .partition(|row| matches!(row.id, ActiveValue::Set(_)));
    for group in [kept, new] {
        for batch in group.chunks(INSERT_BATCH) {
            play_queue_entry::Entity::insert_many(batch.to_vec())
                .exec_without_returning(connection)
                .await
                .map_err(storage)?;
        }
    }
    Ok(())
}

async fn delete_queues<C: ConnectionTrait>(
    connection: &C,
    queues: &[i64],
) -> Result<u64, PlayQueueError> {
    let mut deleted = 0;
    for batch in queues.chunks(INSERT_BATCH) {
        let ids = || batch.iter().copied();
        play_queue_entry::Entity::delete_many()
            .filter(play_queue_entry::Column::QueueId.is_in(ids()))
            .exec(connection)
            .await
            .map_err(storage)?;
        play_queue_source::Entity::delete_many()
            .filter(play_queue_source::Column::QueueId.is_in(ids()))
            .exec(connection)
            .await
            .map_err(storage)?;
        deleted += play_queue::Entity::delete_many()
            .filter(play_queue::Column::Id.is_in(ids()))
            .exec(connection)
            .await
            .map_err(storage)?
            .rows_affected;
    }
    Ok(deleted)
}

#[async_trait::async_trait]
impl PlayQueueRepository for SqlPlayQueueRepository {
    async fn create(&self, queue: NewPlayQueue, now: i64) -> Result<PlayQueue, PlayQueueError> {
        let transaction = self.connection.begin().await.map_err(storage)?;
        if let Some(client) = &queue.client {
            let replaced: Vec<i64> = play_queue::Entity::find()
                .select_only()
                .column(play_queue::Column::Id)
                .filter(play_queue::Column::UserId.eq(queue.user.get()))
                .filter(play_queue::Column::Client.eq(client.as_str()))
                .filter(play_queue::Column::Kind.eq(queue.kind.as_str()))
                .into_tuple()
                .all(&transaction)
                .await
                .map_err(storage)?;
            delete_queues(&transaction, &replaced).await?;
        }

        let contents = queue.initial_contents();
        let row = play_queue::ActiveModel {
            id: ActiveValue::NotSet,
            user_id: ActiveValue::Set(queue.user.get()),
            client: ActiveValue::Set(queue.client.clone()),
            kind: ActiveValue::Set(queue.kind.as_str().to_owned()),
            source_uri: ActiveValue::Set(queue.source_uri.clone()),
            shuffled: ActiveValue::Set(contents.shuffled),
            version: ActiveValue::Set(1),
            selected_position: ActiveValue::Set(contents.selected.map(position)),
            last_added_position: ActiveValue::Set(None),
            created_at: ActiveValue::Set(now),
            playlist_id: ActiveValue::Set(queue.playlist.map(klapz_core::PlaylistId::get)),
        }
        .insert(&transaction)
        .await
        .map_err(storage)?;

        write_entries(&transaction, row.id, &contents.slots).await?;
        let source: Vec<play_queue_source::ActiveModel> = queue
            .source
            .iter()
            .take(klapz_core::page::MAX_ITEMS as usize)
            .zip(0_i64..)
            .map(|(item, position)| play_queue_source::ActiveModel {
                queue_id: ActiveValue::Set(row.id),
                position: ActiveValue::Set(position),
                item_id: ActiveValue::Set(item.get()),
            })
            .collect();
        for batch in source.chunks(INSERT_BATCH) {
            play_queue_source::Entity::insert_many(batch.to_vec())
                .exec_without_returning(&transaction)
                .await
                .map_err(storage)?;
        }
        let queue = loaded(&transaction, row).await?;
        transaction.commit().await.map_err(storage)?;
        Ok(queue)
    }

    async fn queue(&self, id: i64) -> Result<Option<PlayQueue>, PlayQueueError> {
        let row = play_queue::Entity::find_by_id(id)
            .one(&self.connection)
            .await
            .map_err(storage)?;
        match row {
            Some(row) => Ok(Some(loaded(&self.connection, row).await?)),
            None => Ok(None),
        }
    }

    async fn entry(&self, id: i64) -> Result<Option<PlayQueueEntry>, PlayQueueError> {
        Ok(play_queue_entry::Entity::find_by_id(id)
            .one(&self.connection)
            .await
            .map_err(storage)?
            .map(to_entry))
    }

    async fn entries(
        &self,
        queue: i64,
        window: Window,
    ) -> Result<Page<PlayQueueEntry>, PlayQueueError> {
        let position = play_queue_entry::Column::Position;
        let mut select =
            play_queue_entry::Entity::find().filter(play_queue_entry::Column::QueueId.eq(queue));
        if let Some(after) = window.after.as_ref() {
            select = select.filter(position.gt(after.id));
        }
        if let Some(before) = window.before.as_ref() {
            select = select.filter(position.lt(before.id));
        }
        let order = if window.is_reversed() {
            Order::Desc
        } else {
            Order::Asc
        };
        let rows = select
            .order_by(position, order)
            .offset(window.skip)
            .limit(window.fetch_limit())
            .all(&self.connection)
            .await
            .map_err(storage)?;
        let mut page = Page::from_overfetch(rows, &window).map(to_entry);
        if window.is_reversed() {
            page.items.reverse();
        }
        Ok(page)
    }

    async fn change(
        &self,
        queue: i64,
        change: &PlayQueueChange,
    ) -> Result<ChangeOutcome, PlayQueueError> {
        let transaction = self.connection.begin().await.map_err(storage)?;
        let Some(row) = play_queue::Entity::find_by_id(queue)
            .one(&transaction)
            .await
            .map_err(storage)?
        else {
            return Ok(None);
        };
        let slots: Vec<Slot> = play_queue_entry::Entity::find()
            .filter(play_queue_entry::Column::QueueId.eq(queue))
            .order_by_asc(play_queue_entry::Column::Position)
            .all(&transaction)
            .await
            .map_err(storage)?
            .into_iter()
            .map(|entry| Slot {
                entry: Some(entry.id),
                item: RatingKey::new(entry.item_id),
            })
            .collect();
        let source: Vec<RatingKey> = play_queue_source::Entity::find()
            .filter(play_queue_source::Column::QueueId.eq(queue))
            .order_by_asc(play_queue_source::Column::Position)
            .all(&transaction)
            .await
            .map_err(storage)?
            .into_iter()
            .map(|item| RatingKey::new(item.item_id))
            .collect();
        let index = |position: Option<i64>| {
            position
                .and_then(|position| usize::try_from(position).ok())
                .filter(|index| *index < slots.len())
        };
        let mut contents = PlayQueueContents {
            selected: index(row.selected_position),
            last_added: index(row.last_added_position),
            shuffled: row.shuffled,
            slots,
        };
        let raised = match change.apply(&mut contents, &source) {
            Ok(raised) => raised,
            Err(refusal) => return Ok(Some(Err(refusal))),
        };
        write_entries(&transaction, queue, &contents.slots).await?;
        let version = if raised { row.version + 1 } else { row.version };
        let mut edited: play_queue::ActiveModel = row.into();
        edited.version = ActiveValue::Set(version);
        edited.shuffled = ActiveValue::Set(contents.shuffled);
        edited.selected_position = ActiveValue::Set(contents.selected.map(position));
        edited.last_added_position = ActiveValue::Set(contents.last_added.map(position));
        let row = edited.update(&transaction).await.map_err(storage)?;
        let queue = loaded(&transaction, row).await?;
        transaction.commit().await.map_err(storage)?;
        Ok(Some(Ok(queue)))
    }

    async fn delete_created_before(&self, before: i64) -> Result<u64, PlayQueueError> {
        let transaction = self.connection.begin().await.map_err(storage)?;
        let stale: Vec<i64> = play_queue::Entity::find()
            .select_only()
            .column(play_queue::Column::Id)
            .filter(play_queue::Column::CreatedAt.lt(before))
            .into_tuple()
            .all(&transaction)
            .await
            .map_err(storage)?;
        let deleted = delete_queues(&transaction, &stale).await?;
        transaction.commit().await.map_err(storage)?;
        Ok(deleted)
    }
}
