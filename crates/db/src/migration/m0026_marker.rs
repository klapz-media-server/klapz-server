//! Markers: an item's intro, credits and commercials, and the places its viewers bookmark.
//!
//! One row per marker, where the reference packs what it knows into `taggings.extra_data`
//! (`pv:final=1&pv:version=4`) and keeps bookmarks in a table of their own: whether a marker
//! is final and which detection found it become columns, and the owner says whose it is —
//! null for the work's, a user's id or 0 for the operator's bookmark.
//!
//! No foreign key to the item, as for playlist entries: a re-import replaces every item, and a
//! viewer's bookmarks must outlive that. The importer deletes the work's markers itself.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ItemMarker::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ItemMarker::Id)
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(ItemMarker::ItemId).big_integer().not_null())
                    .col(ColumnDef::new(ItemMarker::Owner).big_integer())
                    .col(ColumnDef::new(ItemMarker::Kind).string_len(16).not_null())
                    .col(ColumnDef::new(ItemMarker::StartMs).big_integer().not_null())
                    .col(ColumnDef::new(ItemMarker::EndMs).big_integer())
                    .col(ColumnDef::new(ItemMarker::IsFinal).boolean().not_null())
                    .col(ColumnDef::new(ItemMarker::Version).integer())
                    .col(ColumnDef::new(ItemMarker::Title).text())
                    .col(ColumnDef::new(ItemMarker::Color).string_len(32))
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_item_marker_item")
                    .table(ItemMarker::Table)
                    .col(ItemMarker::ItemId)
                    .col(ItemMarker::Id)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ItemMarker::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum ItemMarker {
    Table,
    Id,
    ItemId,
    Owner,
    Kind,
    StartMs,
    EndMs,
    IsFinal,
    Version,
    Title,
    Color,
}
