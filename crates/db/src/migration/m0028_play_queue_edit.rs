//! Editing play queues: the player and kind a queue was made for, the end of what was queued by
//! hand, and the source shuffling rebuilds it from.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // One column per statement: SQLite cannot add several at once.
        let columns = [
            ColumnDef::new(PlayQueue::Client).text().to_owned(),
            ColumnDef::new(PlayQueue::Kind)
                .text()
                .not_null()
                .default("video")
                .to_owned(),
            ColumnDef::new(PlayQueue::LastAddedPosition)
                .big_integer()
                .to_owned(),
        ];
        for mut column in columns {
            manager
                .alter_table(
                    Table::alter()
                        .table(PlayQueue::Table)
                        .add_column(&mut column)
                        .to_owned(),
                )
                .await?;
        }
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_play_queue_client")
                    .table(PlayQueue::Table)
                    .col(PlayQueue::UserId)
                    .col(PlayQueue::Client)
                    .col(PlayQueue::Kind)
                    .to_owned(),
            )
            .await?;
        manager
            .create_table(
                Table::create()
                    .table(PlayQueueSource::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(PlayQueueSource::QueueId)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PlayQueueSource::Position)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PlayQueueSource::ItemId)
                            .big_integer()
                            .not_null(),
                    )
                    .primary_key(
                        Index::create()
                            .col(PlayQueueSource::QueueId)
                            .col(PlayQueueSource::Position),
                    )
                    .to_owned(),
            )
            .await?;
        // A queue stored before has never been edited: its entries are its source.
        let backfill = Query::insert()
            .into_table(PlayQueueSource::Table)
            .columns([
                PlayQueueSource::QueueId,
                PlayQueueSource::Position,
                PlayQueueSource::ItemId,
            ])
            .select_from(
                Query::select()
                    .columns([
                        PlayQueueEntry::QueueId,
                        PlayQueueEntry::Position,
                        PlayQueueEntry::ItemId,
                    ])
                    .from(PlayQueueEntry::Table)
                    .to_owned(),
            )
            .map_err(|error| DbErr::Migration(error.to_string()))?
            .to_owned();
        manager.exec_stmt(backfill).await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(PlayQueueSource::Table).to_owned())
            .await?;
        manager
            .drop_index(
                Index::drop()
                    .name("idx_play_queue_client")
                    .table(PlayQueue::Table)
                    .to_owned(),
            )
            .await?;
        for column in [
            PlayQueue::Client,
            PlayQueue::Kind,
            PlayQueue::LastAddedPosition,
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(PlayQueue::Table)
                        .drop_column(column)
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}

#[derive(DeriveIden)]
enum PlayQueue {
    Table,
    UserId,
    Client,
    Kind,
    LastAddedPosition,
}

#[derive(DeriveIden)]
enum PlayQueueEntry {
    Table,
    QueueId,
    Position,
    ItemId,
}

#[derive(DeriveIden)]
enum PlayQueueSource {
    Table,
    QueueId,
    Position,
    ItemId,
}
