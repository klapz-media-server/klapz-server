//! Stored play queues.

use open_plex_core::page::{Page, Window};
use open_plex_core::play_queue::{
    NewPlayQueue, PlayQueue, PlayQueueEntry, PlayQueueError, PlayQueueRepository,
};
use open_plex_core::RatingKey;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, Order,
    PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, TransactionTrait,
};

use crate::entity::{play_queue, play_queue_entry};

#[derive(Debug, Clone)]
pub struct SqlPlayQueueRepository {
    connection: DatabaseConnection,
}

impl SqlPlayQueueRepository {
    #[must_use]
    pub const fn new(connection: DatabaseConnection) -> Self {
        Self { connection }
    }

    async fn entry_at(
        &self,
        queue: i64,
        position: i64,
    ) -> Result<Option<PlayQueueEntry>, PlayQueueError> {
        Ok(play_queue_entry::Entity::find()
            .filter(play_queue_entry::Column::QueueId.eq(queue))
            .filter(play_queue_entry::Column::Position.eq(position))
            .one(&self.connection)
            .await
            .map_err(storage)?
            .map(to_entry))
    }

    async fn loaded(&self, row: play_queue::Model) -> Result<PlayQueue, PlayQueueError> {
        let total = play_queue_entry::Entity::find()
            .filter(play_queue_entry::Column::QueueId.eq(row.id))
            .count(&self.connection)
            .await
            .map_err(storage)?;
        let selected = match row.selected_position {
            Some(position) => self.entry_at(row.id, position).await?,
            None => None,
        };
        Ok(PlayQueue {
            id: row.id,
            user: row.user_id.map(open_plex_core::UserId::new),
            source_uri: row.source_uri,
            shuffled: row.shuffled,
            version: row.version,
            total,
            selected,
        })
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

/// `SQLite` refuses a statement with too many bound values.
const INSERT_BATCH: usize = 200;

#[async_trait::async_trait]
impl PlayQueueRepository for SqlPlayQueueRepository {
    async fn create(&self, queue: NewPlayQueue, now: i64) -> Result<PlayQueue, PlayQueueError> {
        let transaction = self.connection.begin().await.map_err(storage)?;
        let selected_position =
            (!queue.items.is_empty()).then(|| i64::try_from(queue.selected).unwrap_or(0));
        let row = play_queue::ActiveModel {
            id: ActiveValue::NotSet,
            user_id: ActiveValue::Set(queue.user.map(open_plex_core::UserId::get)),
            source_uri: ActiveValue::Set(queue.source_uri),
            shuffled: ActiveValue::Set(queue.shuffled),
            version: ActiveValue::Set(1),
            selected_position: ActiveValue::Set(selected_position),
            created_at: ActiveValue::Set(now),
        }
        .insert(&transaction)
        .await
        .map_err(storage)?;

        let rows: Vec<play_queue_entry::ActiveModel> = queue
            .items
            .iter()
            .zip(0_i64..)
            .map(|(item, position)| play_queue_entry::ActiveModel {
                id: ActiveValue::NotSet,
                queue_id: ActiveValue::Set(row.id),
                position: ActiveValue::Set(position),
                item_id: ActiveValue::Set(item.get()),
            })
            .collect();
        for batch in rows.chunks(INSERT_BATCH) {
            play_queue_entry::Entity::insert_many(batch.to_vec())
                .exec(&transaction)
                .await
                .map_err(storage)?;
        }
        transaction.commit().await.map_err(storage)?;
        self.loaded(row).await
    }

    async fn queue(&self, id: i64) -> Result<Option<PlayQueue>, PlayQueueError> {
        let row = play_queue::Entity::find_by_id(id)
            .one(&self.connection)
            .await
            .map_err(storage)?;
        match row {
            Some(row) => Ok(Some(self.loaded(row).await?)),
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
        for batch in stale.chunks(INSERT_BATCH) {
            play_queue_entry::Entity::delete_many()
                .filter(play_queue_entry::Column::QueueId.is_in(batch.iter().copied()))
                .exec(&transaction)
                .await
                .map_err(storage)?;
        }
        let deleted = play_queue::Entity::delete_many()
            .filter(play_queue::Column::CreatedAt.lt(before))
            .exec(&transaction)
            .await
            .map_err(storage)?
            .rows_affected;
        transaction.commit().await.map_err(storage)?;
        Ok(deleted)
    }
}
