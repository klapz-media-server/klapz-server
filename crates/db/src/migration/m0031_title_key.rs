//! Whole titles beside the word index, for ranking a search.
//!
//! The word index says whether an item matches; how well it matches depends on the title as
//! a whole — equal to the term, or opening with its first word — which distinct words cannot
//! tell. So each item keeps its title, sort title and original title, folded by
//! `core::search::title_keys` and flagged with what each counts for, and storage ranks the
//! rows already matched by seeking their keys. Up to three rows per item, one per distinct
//! text: 171,021 items give 271,653 on the imported reference library.
//!
//! Filled here from the items already imported. From then on `write::insert_items` and
//! `write::update_item` are its only writers.

use klapz_core::search;
use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::{big_integer, boolean, string};

#[derive(DeriveMigrationName)]
pub struct Migration;

/// Items read per statement while filling the table.
const FILL_PAGE: u64 = 5_000;

/// Key rows per insert, well under `SQLite`'s bound-parameter cap.
const INSERT_CHUNK: usize = 250;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ItemTitleKey::Table)
                    .if_not_exists()
                    .col(big_integer(ItemTitleKey::ItemId))
                    .col(string(ItemTitleKey::Key))
                    .col(boolean(ItemTitleKey::Exact))
                    .col(boolean(ItemTitleKey::Leading))
                    .primary_key(
                        Index::create()
                            .col(ItemTitleKey::ItemId)
                            .col(ItemTitleKey::Key),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(ItemTitleKey::Table, ItemTitleKey::ItemId)
                            .to(MetadataItem::Table, MetadataItem::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        fill(manager).await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ItemTitleKey::Table).to_owned())
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
                MetadataItem::Title,
                MetadataItem::TitleSort,
                MetadataItem::OriginalTitle,
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

        let mut keys = Vec::new();
        for row in &rows {
            let id: i64 = row.try_get("", "id")?;
            let text = |column: &str| row.try_get::<Option<String>>("", column);
            let (title, title_sort, original) =
                (text("title")?, text("title_sort")?, text("original_title")?);
            keys.extend(
                search::title_keys(title.as_deref(), title_sort.as_deref(), original.as_deref())
                    .into_iter()
                    .map(|key| (id, key)),
            );
        }
        for chunk in keys.chunks(INSERT_CHUNK) {
            let mut insert = Query::insert()
                .into_table(ItemTitleKey::Table)
                .columns([
                    ItemTitleKey::ItemId,
                    ItemTitleKey::Key,
                    ItemTitleKey::Exact,
                    ItemTitleKey::Leading,
                ])
                .to_owned();
            for (id, key) in chunk {
                insert
                    .values([
                        (*id).into(),
                        key.key.as_str().into(),
                        key.exact.into(),
                        key.leading.into(),
                    ])
                    .map_err(|error| DbErr::Migration(error.to_string()))?;
            }
            connection.execute(backend.build(&insert)).await?;
        }
    }
}

#[derive(DeriveIden)]
enum ItemTitleKey {
    Table,
    ItemId,
    Key,
    Exact,
    Leading,
}

#[derive(DeriveIden)]
enum MetadataItem {
    Table,
    Id,
    Title,
    TitleSort,
    OriginalTitle,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use sea_orm::{ConnectionTrait, Database};
    use sea_orm_migration::MigratorTrait;

    use crate::migration::Migrator;

    #[tokio::test]
    async fn keys_the_items_imported_before_it() {
        let connection = Database::connect("sqlite::memory:").await.unwrap();
        let before = Migrator::migrations()
            .iter()
            .position(|migration| migration.name() == "m0031_title_key")
            .expect("registered");
        Migrator::up(&connection, Some(u32::try_from(before).unwrap()))
            .await
            .unwrap();
        connection
            .execute_unprepared(
                "INSERT INTO library_section (id, kind, title, language, uuid, created_at, \
                 updated_at, content_changed_at, hidden) \
                 VALUES (4, 'movie', 'Films', 'en', 'u', 0, 0, 0, false)",
            )
            .await
            .unwrap();
        connection
            .execute_unprepared(
                "INSERT INTO metadata_item (id, library_section_id, kind, guid, title, \
                 title_sort, sort_key, original_title, added_at, updated_at) VALUES \
                 (1, 4, 'movie', 'g1', 'The Batman', 'Batman', 'batman', 'The  Batman', 0, 0)",
            )
            .await
            .unwrap();

        Migrator::up(&connection, None).await.unwrap();

        let rows = connection
            .query_all(sea_orm::Statement::from_string(
                connection.get_database_backend(),
                "SELECT item_id, key, exact, leading FROM item_title_key ORDER BY key",
            ))
            .await
            .unwrap();
        let keys: Vec<(i64, String, bool, bool)> = rows
            .iter()
            .map(|row| {
                (
                    row.try_get("", "item_id").unwrap(),
                    row.try_get("", "key").unwrap(),
                    row.try_get("", "exact").unwrap(),
                    row.try_get("", "leading").unwrap(),
                )
            })
            .collect();
        assert_eq!(
            keys,
            [
                (1, "batman".to_owned(), true, false),
                (1, "the batman".to_owned(), true, true)
            ]
        );
    }
}
