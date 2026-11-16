//! What the reference's item tree publishes and nothing else had read yet: when an item's
//! metadata was last refreshed, the two digests a file is recognized by, and the bandwidths a
//! track's analysis found it needs.
//!
//! The bandwidths are kept as the source writes them, eight kilobit counts joined by commas:
//! one value read and written whole, never queried by element.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // SQLite adds one column per statement.
        manager
            .alter_table(
                Table::alter()
                    .table(MetadataItem::Table)
                    .add_column(ColumnDef::new(MetadataItem::RefreshedAt).big_integer())
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(MediaPart::Table)
                    .add_column(ColumnDef::new(MediaPart::ContentHash).string())
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(MediaPart::Table)
                    .add_column(ColumnDef::new(MediaPart::OpenSubtitlesHash).string())
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(MediaStream::Table)
                    .add_column(ColumnDef::new(MediaStream::RequiredBandwidths).string())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(MediaStream::Table)
                    .drop_column(MediaStream::RequiredBandwidths)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(MediaPart::Table)
                    .drop_column(MediaPart::OpenSubtitlesHash)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(MediaPart::Table)
                    .drop_column(MediaPart::ContentHash)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(MetadataItem::Table)
                    .drop_column(MetadataItem::RefreshedAt)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum MetadataItem {
    Table,
    RefreshedAt,
}

#[derive(DeriveIden)]
enum MediaPart {
    Table,
    ContentHash,
    OpenSubtitlesHash,
}

#[derive(DeriveIden)]
enum MediaStream {
    Table,
    RequiredBandwidths,
}
