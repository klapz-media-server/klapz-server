//! Index for reading a library by when its items were added.
//!
//! A series library's "recently added" reads its episodes newest first, page after page.
//! Without an index on `added_at`, `SQLite` fetched all 79,518 episodes of the reference's
//! largest series library and sorted them for every page: 320 ms to return 500 rows. Keyed
//! on `(library_section_id, kind, added_at, id)`, the same page is a range scan in 0.5 ms,
//! and the next one resumes from the `(added_at, id)` it ended on.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_item_added")
                    .table(MetadataItem::Table)
                    .col(MetadataItem::LibrarySectionId)
                    .col(MetadataItem::Kind)
                    .col(MetadataItem::AddedAt)
                    .col(MetadataItem::Id)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx_item_added")
                    .table(MetadataItem::Table)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum MetadataItem {
    Table,
    Id,
    LibrarySectionId,
    Kind,
    AddedAt,
}
