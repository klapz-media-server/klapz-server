//! A series' theme music: the file the store keeps for it, where it was copied from, and when
//! the series took it.
//!
//! One theme per item, replaced and never added to. Unlike images, a theme is no shared file
//! with a row of its own: a service's theme is one series', and a local one is named after its
//! content, so two series sharing it point at one file all the same.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ItemTheme::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ItemTheme::ItemId)
                            .big_integer()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(ItemTheme::Source).string_len(16).not_null())
                    .col(ColumnDef::new(ItemTheme::Origin).string().not_null())
                    .col(ColumnDef::new(ItemTheme::File).string().not_null())
                    .col(
                        ColumnDef::new(ItemTheme::ContentType)
                            .string_len(32)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ItemTheme::SizeBytes)
                            .big_integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(ItemTheme::ChosenAt).big_integer().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .from(ItemTheme::Table, ItemTheme::ItemId)
                            .to(MetadataItem::Table, MetadataItem::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ItemTheme::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum ItemTheme {
    Table,
    ItemId,
    Source,
    Origin,
    File,
    ContentType,
    SizeBytes,
    ChosenAt,
}

#[derive(DeriveIden)]
enum MetadataItem {
    Table,
    Id,
}
