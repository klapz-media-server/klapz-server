//! The fields locked on each item: what a metadata refresh must leave as an editor wrote it.
//!
//! A row per locked field rather than the reference's packed `lockedFields=1|2|9` text, so a
//! lock can be added, removed and read without parsing. The field is stored by our name for
//! it, not the reference's number: the importer is the one place that knows those numbers.

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
                    .table(ItemFieldLock::Table)
                    .if_not_exists()
                    .col(big_integer(ItemFieldLock::ItemId))
                    .col(string_len(ItemFieldLock::Field, 32))
                    .primary_key(
                        Index::create()
                            .col(ItemFieldLock::ItemId)
                            .col(ItemFieldLock::Field),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(ItemFieldLock::Table, ItemFieldLock::ItemId)
                            .to(MetadataItem::Table, MetadataItem::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ItemFieldLock::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum ItemFieldLock {
    Table,
    ItemId,
    Field,
}

#[derive(DeriveIden)]
enum MetadataItem {
    Table,
    Id,
}
