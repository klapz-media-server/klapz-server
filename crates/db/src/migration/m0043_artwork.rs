//! Artwork kept on disk: one row per file the store wrote, and the image of each kind each
//! item shows.
//!
//! A file is shared: two items, or two libraries, showing one poster point at one row. An
//! item's image is replaced, never added to, and nothing reaches the file from the item but
//! its name, so a file no item shows any more can be found and removed later.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ImageFile::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ImageFile::File)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(ImageFile::SourceUrl).string().not_null())
                    .col(
                        ColumnDef::new(ImageFile::ContentType)
                            .string_len(16)
                            .not_null(),
                    )
                    .col(ColumnDef::new(ImageFile::Width).integer().not_null())
                    .col(ColumnDef::new(ImageFile::Height).integer().not_null())
                    .col(
                        ColumnDef::new(ImageFile::SizeBytes)
                            .big_integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(ImageFile::BlurHash).string().not_null())
                    .col(ColumnDef::new(ImageFile::StoredAt).big_integer().not_null())
                    .to_owned(),
            )
            .await?;
        manager
            .create_table(
                Table::create()
                    .table(ItemImage::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(ItemImage::ItemId).big_integer().not_null())
                    .col(ColumnDef::new(ItemImage::Kind).string_len(16).not_null())
                    .col(ColumnDef::new(ItemImage::File).string().not_null())
                    .col(ColumnDef::new(ItemImage::ChosenAt).big_integer().not_null())
                    .primary_key(Index::create().col(ItemImage::ItemId).col(ItemImage::Kind))
                    .foreign_key(
                        ForeignKey::create()
                            .from(ItemImage::Table, ItemImage::ItemId)
                            .to(MetadataItem::Table, MetadataItem::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(ItemImage::Table, ItemImage::File)
                            .to(ImageFile::Table, ImageFile::File)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ItemImage::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(ImageFile::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum ImageFile {
    Table,
    File,
    SourceUrl,
    ContentType,
    Width,
    Height,
    SizeBytes,
    BlurHash,
    StoredAt,
}

#[derive(DeriveIden)]
enum ItemImage {
    Table,
    ItemId,
    Kind,
    File,
    ChosenAt,
}

#[derive(DeriveIden)]
enum MetadataItem {
    Table,
    Id,
}
