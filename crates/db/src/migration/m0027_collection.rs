//! Collections: a library's items grouped under a name, their entries, and the words their
//! titles are searched by.
//!
//! A collection's id is drawn from the items' numbering, not from a sequence of its own: the
//! reference looks collections up at `/library/metadata/{id}` beside items, and imported ones
//! keep the reference's ids. So the column is not auto-incremented; storage draws the next
//! id past both tables.
//!
//! An entry names its item without a foreign key, as a playlist's does: an item that leaves
//! and comes back under its id, as a re-import brings it back, finds its collection again.
//! Every read joins it to the item.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    #[allow(
        clippy::too_many_lines,
        reason = "one table after another: splitting it would only move the list"
    )]
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let big = |column: Collection| ColumnDef::new(column).big_integer().not_null().to_owned();
        let flag = |column: Collection| ColumnDef::new(column).boolean().not_null().to_owned();
        manager
            .create_table(
                Table::create()
                    .table(Collection::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Collection::Id)
                            .big_integer()
                            .not_null()
                            .primary_key(),
                    )
                    .col(big(Collection::LibrarySectionId))
                    .col(ColumnDef::new(Collection::Kind).string_len(16).not_null())
                    .col(ColumnDef::new(Collection::Guid).string_len(255).not_null())
                    .col(big(Collection::TagNumber))
                    .col(ColumnDef::new(Collection::Title).text().not_null())
                    .col(ColumnDef::new(Collection::TitleSort).text())
                    .col(ColumnDef::new(Collection::SortKey).text().not_null())
                    .col(ColumnDef::new(Collection::Summary).text().not_null())
                    .col(ColumnDef::new(Collection::ContentRating).text())
                    .col(ColumnDef::new(Collection::RuleListing).text())
                    .col(ColumnDef::new(Collection::SourceUri).text())
                    .col(ColumnDef::new(Collection::Mode).string_len(16).not_null())
                    .col(
                        ColumnDef::new(Collection::ItemOrder)
                            .string_len(16)
                            .not_null(),
                    )
                    .col(flag(Collection::TitleLocked))
                    .col(flag(Collection::TitleSortLocked))
                    .col(flag(Collection::SummaryLocked))
                    .col(flag(Collection::ContentRatingLocked))
                    .col(flag(Collection::PosterLocked))
                    .col(flag(Collection::BackgroundLocked))
                    .col(ColumnDef::new(Collection::ThumbUrl).text())
                    .col(ColumnDef::new(Collection::ArtUrl).text())
                    .col(big(Collection::AddedAt))
                    .col(big(Collection::UpdatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(Collection::Table, Collection::LibrarySectionId)
                            .to(LibrarySection::Table, LibrarySection::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_collection_section_sort_key")
                    .table(Collection::Table)
                    .col(Collection::LibrarySectionId)
                    .col(Collection::SortKey)
                    .to_owned(),
            )
            .await?;
        // One library numbers each collection apart; two libraries may share a number.
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .unique()
                    .name("idx_collection_section_tag_number")
                    .table(Collection::Table)
                    .col(Collection::LibrarySectionId)
                    .col(Collection::TagNumber)
                    .to_owned(),
            )
            .await?;

        let entry =
            |column: CollectionEntry| ColumnDef::new(column).big_integer().not_null().to_owned();
        manager
            .create_table(
                Table::create()
                    .table(CollectionEntry::Table)
                    .if_not_exists()
                    // The order items joined in, which an item lists its collections by.
                    .col(
                        ColumnDef::new(CollectionEntry::Id)
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(entry(CollectionEntry::CollectionId))
                    .col(entry(CollectionEntry::ItemId))
                    .col(entry(CollectionEntry::Position))
                    .foreign_key(
                        ForeignKey::create()
                            .from(CollectionEntry::Table, CollectionEntry::CollectionId)
                            .to(Collection::Table, Collection::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .unique()
                    .name("idx_collection_entry_item")
                    .table(CollectionEntry::Table)
                    .col(CollectionEntry::CollectionId)
                    .col(CollectionEntry::ItemId)
                    .to_owned(),
            )
            .await?;
        // Not unique, for the reason a playlist's positions are not: a renumbering rewrites
        // them one row at a time.
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_collection_entry_position")
                    .table(CollectionEntry::Table)
                    .col(CollectionEntry::CollectionId)
                    .col(CollectionEntry::Position)
                    .to_owned(),
            )
            .await?;
        // An item's collections, and a listing filtered on one, are read from the item.
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_collection_entry_by_item")
                    .table(CollectionEntry::Table)
                    .col(CollectionEntry::ItemId)
                    .col(CollectionEntry::CollectionId)
                    .to_owned(),
            )
            .await?;

        // The words of a collection's titles, as `m0004_title_word` keeps an item's: folding
        // is `core::search`'s, which no SQL expression reproduces on every engine.
        manager
            .create_table(
                Table::create()
                    .table(CollectionWord::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(CollectionWord::CollectionId)
                            .big_integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(CollectionWord::Word).string().not_null())
                    .primary_key(
                        Index::create()
                            .col(CollectionWord::Word)
                            .col(CollectionWord::CollectionId),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(CollectionWord::Table, CollectionWord::CollectionId)
                            .to(Collection::Table, Collection::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_collection_word_collection")
                    .table(CollectionWord::Table)
                    .col(CollectionWord::CollectionId)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(CollectionWord::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(CollectionEntry::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Collection::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden, Clone, Copy)]
enum Collection {
    Table,
    Id,
    LibrarySectionId,
    Kind,
    Guid,
    TagNumber,
    Title,
    TitleSort,
    SortKey,
    Summary,
    ContentRating,
    RuleListing,
    SourceUri,
    Mode,
    ItemOrder,
    TitleLocked,
    TitleSortLocked,
    SummaryLocked,
    ContentRatingLocked,
    PosterLocked,
    BackgroundLocked,
    ThumbUrl,
    ArtUrl,
    AddedAt,
    UpdatedAt,
}

#[derive(DeriveIden, Clone, Copy)]
enum CollectionEntry {
    Table,
    Id,
    CollectionId,
    ItemId,
    Position,
}

#[derive(DeriveIden)]
enum CollectionWord {
    Table,
    CollectionId,
    Word,
}

#[derive(DeriveIden)]
enum LibrarySection {
    Table,
    Id,
}
