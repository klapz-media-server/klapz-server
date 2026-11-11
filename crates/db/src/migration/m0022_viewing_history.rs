//! What each viewer watched, one row per viewing.
//!
//! The row names the copy watched, not the work: watch state is per `guid`, but a history
//! says which library a viewing happened in. Nothing about the item is copied into the row —
//! the reference keeps titles and thumbnails there, which go stale on every edit — so an
//! entry goes with its item. The viewer is a number as in `watch_state`, 0 being the
//! operator, with no foreign key for the same reason.

use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::{big_integer, string_len};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ViewingHistory::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ViewingHistory::Id)
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(big_integer(ViewingHistory::Viewer))
                    .col(big_integer(ViewingHistory::ItemId))
                    // `played` or `marked`.
                    .col(string_len(ViewingHistory::Kind, 16))
                    .col(big_integer(ViewingHistory::ViewedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(ViewingHistory::Table, ViewingHistory::ItemId)
                            .to(MetadataItem::Table, MetadataItem::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("viewing_history_viewer_viewed_at")
                    .table(ViewingHistory::Table)
                    .col(ViewingHistory::Viewer)
                    .col(ViewingHistory::ViewedAt)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("viewing_history_item")
                    .table(ViewingHistory::Table)
                    .col(ViewingHistory::ItemId)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ViewingHistory::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum ViewingHistory {
    Table,
    Id,
    Viewer,
    ItemId,
    Kind,
    ViewedAt,
}

#[derive(DeriveIden)]
enum MetadataItem {
    Table,
    Id,
}
