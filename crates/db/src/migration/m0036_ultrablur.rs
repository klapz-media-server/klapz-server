//! The four corner colors of an item's blurred backdrop, which Plex calls `UltraBlur`.
//!
//! A row per item, each color an sRGB integer `0xRRGGBB`, where the reference packs four hex
//! strings into `extra_data`. The four always come together, so a row holds all of them or
//! does not exist.

use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::{big_integer, integer};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ItemCornerColors::Table)
                    .if_not_exists()
                    .col(big_integer(ItemCornerColors::ItemId).primary_key())
                    .col(integer(ItemCornerColors::TopLeft))
                    .col(integer(ItemCornerColors::TopRight))
                    .col(integer(ItemCornerColors::BottomRight))
                    .col(integer(ItemCornerColors::BottomLeft))
                    .foreign_key(
                        ForeignKey::create()
                            .from(ItemCornerColors::Table, ItemCornerColors::ItemId)
                            .to(MetadataItem::Table, MetadataItem::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ItemCornerColors::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum ItemCornerColors {
    Table,
    ItemId,
    TopLeft,
    TopRight,
    BottomRight,
    BottomLeft,
}

#[derive(DeriveIden)]
enum MetadataItem {
    Table,
    Id,
}
