//! Play queues: a queue, and its entries in play order.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(PlayQueue::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(PlayQueue::Id)
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(PlayQueue::SourceUri).text().not_null())
                    .col(ColumnDef::new(PlayQueue::Shuffled).boolean().not_null())
                    .col(ColumnDef::new(PlayQueue::Version).integer().not_null())
                    .col(ColumnDef::new(PlayQueue::SelectedPosition).big_integer())
                    .col(
                        ColumnDef::new(PlayQueue::CreatedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_table(
                Table::create()
                    .table(PlayQueueEntry::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(PlayQueueEntry::Id)
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(PlayQueueEntry::QueueId)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PlayQueueEntry::Position)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PlayQueueEntry::ItemId)
                            .big_integer()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .unique()
                    .name("idx_play_queue_entry_position")
                    .table(PlayQueueEntry::Table)
                    .col(PlayQueueEntry::QueueId)
                    .col(PlayQueueEntry::Position)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_play_queue_created")
                    .table(PlayQueue::Table)
                    .col(PlayQueue::CreatedAt)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(PlayQueueEntry::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(PlayQueue::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum PlayQueue {
    Table,
    Id,
    SourceUri,
    Shuffled,
    Version,
    SelectedPosition,
    CreatedAt,
}

#[derive(DeriveIden)]
enum PlayQueueEntry {
    Table,
    Id,
    QueueId,
    Position,
    ItemId,
}
