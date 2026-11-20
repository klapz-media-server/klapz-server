//! Chapters: one row per chapter of an item, where the reference keeps them twice — as tag
//! attachments of tag type 9 on the item, and as JSON on each analyzed file — and names each
//! by a tag shared by every chapter of the same title. The title becomes a column, and so does
//! the version whose frame is the chapter's image, which the reference spells out in a URL.
//!
//! Where an item's chapters came from (`media`, `agent`, `mixed`) is the item's, as on the
//! reference.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ItemChapter::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ItemChapter::Id)
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(ItemChapter::ItemId).big_integer().not_null())
                    .col(ColumnDef::new(ItemChapter::Position).integer().not_null())
                    .col(
                        ColumnDef::new(ItemChapter::StartMs)
                            .big_integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(ItemChapter::EndMs).big_integer().not_null())
                    .col(ColumnDef::new(ItemChapter::Title).text())
                    .col(ColumnDef::new(ItemChapter::ImageMediaId).big_integer())
                    .foreign_key(
                        ForeignKey::create()
                            .from(ItemChapter::Table, ItemChapter::ItemId)
                            .to(MetadataItem::Table, MetadataItem::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(ItemChapter::Table, ItemChapter::ImageMediaId)
                            .to(Media::Table, Media::Id)
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_item_chapter_item")
                    .table(ItemChapter::Table)
                    .col(ItemChapter::ItemId)
                    .col(ItemChapter::Position)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(MetadataItem::Table)
                    .add_column(ColumnDef::new(MetadataItem::ChapterSource).string_len(8))
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(MetadataItem::Table)
                    .drop_column(MetadataItem::ChapterSource)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(ItemChapter::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum ItemChapter {
    Table,
    Id,
    ItemId,
    Position,
    StartMs,
    EndMs,
    Title,
    ImageMediaId,
}

#[derive(DeriveIden)]
enum MetadataItem {
    Table,
    Id,
    ChapterSource,
}

#[derive(DeriveIden)]
enum Media {
    Table,
    Id,
}
