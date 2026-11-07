//! Index for the default library listing.
//!
//! `/library/sections/{id}/all` lists a library's top level, which is `parent_id IS NULL`.
//! `idx_item_listing` keys on `(library_section_id, kind, sort_key, id)` and so cannot serve
//! that predicate: on the reference library's series section, `SQLite` walked all 86,655 index
//! entries in sort order and checked `parent_id` against the table for each, which cost
//! 228 ms to return 50 rows. The same listing on a film section, where every row is top
//! level, cost 39 ms.
//!
//! Keying on `parent_id` instead of `kind` turns that into a range scan of exactly the 1,980
//! series. Both indexes are kept: a listing narrowed by `type=` still wants the other one.

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
                    .name("idx_item_top_level")
                    .table(MetadataItem::Table)
                    .col(MetadataItem::LibrarySectionId)
                    .col(MetadataItem::ParentId)
                    .col(MetadataItem::SortKey)
                    .col(MetadataItem::Id)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx_item_top_level")
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
    ParentId,
    SortKey,
}
