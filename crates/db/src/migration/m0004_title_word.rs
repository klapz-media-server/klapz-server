//! Word index behind the title search.
//!
//! `/library/sections/{id}/all?title=le` is a **word-prefix** match, not a column prefix: the
//! reference server returns the 75 films from 2022 having a word starting with `le` anywhere
//! in their title, sort title or original title. A prefix on the sort title finds 45 of them.
//!
//! One row per (item, distinct word) — 736,261 rows for 171,018 items, 4.3 words each. The
//! alternative, full-text search, is the least portable feature across database engines, and
//! the engine is meant to stay swappable; a plain table with a range scan on `word` behaves
//! the same everywhere.
//!
//! Two indexes because the planner needs both directions. `(word, item_id)` turns a prefix
//! into one range scan producing candidate items; `(item_id, word)` lets the same predicate be
//! checked as a seek when the planner would rather drive from the item side, which it prefers
//! when the other filters are already narrow.

use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::{big_integer, string};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ItemTitleWord::Table)
                    .if_not_exists()
                    .col(big_integer(ItemTitleWord::ItemId))
                    .col(string(ItemTitleWord::Word))
                    .primary_key(
                        Index::create()
                            .col(ItemTitleWord::Word)
                            .col(ItemTitleWord::ItemId),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(ItemTitleWord::Table, ItemTitleWord::ItemId)
                            .to(MetadataItem::Table, MetadataItem::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_title_word_item")
                    .table(ItemTitleWord::Table)
                    .col(ItemTitleWord::ItemId)
                    .col(ItemTitleWord::Word)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ItemTitleWord::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum ItemTitleWord {
    Table,
    ItemId,
    Word,
}

#[derive(DeriveIden)]
enum MetadataItem {
    Table,
    Id,
}
