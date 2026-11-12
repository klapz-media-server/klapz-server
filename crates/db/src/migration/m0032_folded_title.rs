//! The titles the voice search compares, and their lengths.
//!
//! `GET /hubs/search/voice` ranks every title by its edit distance to a term. A distance is
//! at least the difference of the two lengths, so a search only has to read the titles of
//! about the term's length, widening until the closest ones found are nearer than anything
//! longer or shorter could be: `dune` reads the 1,608 films and series of three to five
//! units, of 22,337. That needs the length indexed, and the length is the folded title's
//! (`core::closest`), which no SQL expression computes — hence a table of its own, one row
//! per item, written by `write::insert_items` and `write::update_item` alone.
//!
//! The section and kind are copied beside it, neither of which ever changes for an item, and
//! the index covers every column a search reads: a band of length is one range of the index
//! per kind, and neither the items nor this table's rows are visited. Filled here from the
//! items already imported.

use open_plex_core::closest;
use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::{big_integer, integer, string};

#[derive(DeriveMigrationName)]
pub struct Migration;

/// Items read per statement while filling the table.
const FILL_PAGE: u64 = 5_000;

/// Rows per insert, five values each: well under `SQLite`'s bound-parameter cap.
const INSERT_CHUNK: usize = 500;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ItemFoldedTitle::Table)
                    .if_not_exists()
                    .col(big_integer(ItemFoldedTitle::ItemId).primary_key())
                    .col(big_integer(ItemFoldedTitle::LibrarySectionId))
                    .col(string(ItemFoldedTitle::Kind))
                    .col(string(ItemFoldedTitle::FoldedTitle))
                    .col(integer(ItemFoldedTitle::FoldedLength))
                    .foreign_key(
                        ForeignKey::create()
                            .from(ItemFoldedTitle::Table, ItemFoldedTitle::ItemId)
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
                    .name("idx_folded_title_kind_length")
                    .table(ItemFoldedTitle::Table)
                    .col(ItemFoldedTitle::Kind)
                    .col(ItemFoldedTitle::FoldedLength)
                    .col(ItemFoldedTitle::LibrarySectionId)
                    .col(ItemFoldedTitle::ItemId)
                    .col(ItemFoldedTitle::FoldedTitle)
                    .to_owned(),
            )
            .await?;

        fill(manager).await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ItemFoldedTitle::Table).to_owned())
            .await
    }
}

async fn fill(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    let connection = manager.get_connection();
    let backend = manager.get_database_backend();
    let mut last_id = i64::MIN;
    loop {
        let page = Query::select()
            .columns([
                MetadataItem::Id,
                MetadataItem::LibrarySectionId,
                MetadataItem::Kind,
                MetadataItem::Title,
                MetadataItem::TitleSort,
            ])
            .from(MetadataItem::Table)
            .and_where(Expr::col(MetadataItem::Id).gt(last_id))
            .order_by(MetadataItem::Id, Order::Asc)
            .limit(FILL_PAGE)
            .to_owned();
        let rows = connection.query_all(backend.build(&page)).await?;
        let Some(last) = rows.last() else {
            return Ok(());
        };
        last_id = last.try_get("", "id")?;

        let mut folded: Vec<(i64, i64, String, i64, String)> = Vec::with_capacity(rows.len());
        for row in &rows {
            let title: Option<String> = row.try_get("", "title")?;
            let title_sort: Option<String> = row.try_get("", "title_sort")?;
            let key = closest::title_key(title_sort.as_deref(), title.as_deref());
            folded.push((
                row.try_get::<i64>("", "id")?,
                row.try_get::<i64>("", "library_section_id")?,
                row.try_get::<String>("", "kind")?,
                <i64 as TryFrom<usize>>::try_from(closest::key_length(&key)).unwrap_or(i64::MAX),
                key,
            ));
        }
        for chunk in folded.chunks(INSERT_CHUNK) {
            let mut insert = Query::insert()
                .into_table(ItemFoldedTitle::Table)
                .columns([
                    ItemFoldedTitle::ItemId,
                    ItemFoldedTitle::LibrarySectionId,
                    ItemFoldedTitle::Kind,
                    ItemFoldedTitle::FoldedLength,
                    ItemFoldedTitle::FoldedTitle,
                ])
                .to_owned();
            for (id, section, kind, length, key) in chunk {
                insert
                    .values([
                        (*id).into(),
                        (*section).into(),
                        kind.as_str().into(),
                        (*length).into(),
                        key.as_str().into(),
                    ])
                    .map_err(|error| DbErr::Migration(error.to_string()))?;
            }
            connection.execute(backend.build(&insert)).await?;
        }
    }
}

#[derive(DeriveIden)]
enum ItemFoldedTitle {
    Table,
    ItemId,
    LibrarySectionId,
    Kind,
    FoldedTitle,
    FoldedLength,
}

#[derive(DeriveIden)]
enum MetadataItem {
    Table,
    Id,
    LibrarySectionId,
    Kind,
    Title,
    TitleSort,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use sea_orm::{ConnectionTrait, Database};
    use sea_orm_migration::MigratorTrait;

    use crate::migration::Migrator;

    #[tokio::test]
    async fn folds_the_titles_imported_before_it() {
        let connection = Database::connect("sqlite::memory:").await.unwrap();
        let before = Migrator::migrations()
            .iter()
            .position(|migration| migration.name() == "m0032_folded_title")
            .expect("registered");
        Migrator::up(&connection, Some(u32::try_from(before).unwrap()))
            .await
            .unwrap();
        connection
            .execute_unprepared(
                "INSERT INTO library_section (id, kind, title, language, uuid, created_at, \
                 updated_at, content_changed_at, hidden) \
                 VALUES (5, 'show', 'Series', 'en', 'u', 0, 0, 0, false); \
                 INSERT INTO metadata_item (id, library_section_id, kind, guid, title, \
                 title_sort, sort_key, added_at, updated_at) VALUES \
                 (1, 5, 'show', 'g1', 'The Office', 'Office', 'office', 0, 0), \
                 (2, 5, 'show', 'g2', 'Boże Ciało', NULL, 'boże ciało', 0, 0)",
            )
            .await
            .unwrap();

        Migrator::up(&connection, None).await.unwrap();

        let rows = connection
            .query_all(sea_orm::Statement::from_string(
                connection.get_database_backend(),
                "SELECT item_id, library_section_id, kind, folded_title, folded_length \
                 FROM item_folded_title ORDER BY item_id",
            ))
            .await
            .unwrap();
        let folded: Vec<(i64, i64, String, String, i64)> = rows
            .iter()
            .map(|row| {
                (
                    row.try_get("", "item_id").unwrap(),
                    row.try_get("", "library_section_id").unwrap(),
                    row.try_get("", "kind").unwrap(),
                    row.try_get("", "folded_title").unwrap(),
                    row.try_get("", "folded_length").unwrap(),
                )
            })
            .collect();
        assert_eq!(
            folded,
            [
                (1, 5, "show".to_owned(), "office".to_owned(), 6),
                (2, 5, "show".to_owned(), "boże ciało".to_owned(), 12),
            ]
        );
    }
}
