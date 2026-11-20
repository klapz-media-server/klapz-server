//! Word index behind the search over tags: people and genres found by name.
//!
//! The same design as `m0004_title_word`, for the same reason: a name is matched by a prefix
//! of any of its words (`hanks` finds Tom Hanks), accents folded, and folding has to be the
//! one in `core::search` — which no SQL expression can reproduce on every engine. One row per
//! (tag, distinct word): 292,549 tags give 614,479 rows on the imported reference library.
//!
//! Filled here from the tags already imported, so a database migrated in place finds people
//! without a new import. From then on `write::insert_tags` is its only writer.

use klapz_core::search;
use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::{big_integer, string};

#[derive(DeriveMigrationName)]
pub struct Migration;

/// Tags read per statement while filling the index.
const FILL_PAGE: u64 = 5_000;

/// Word rows per insert, well under `SQLite`'s bound-parameter cap.
const INSERT_CHUNK: usize = 500;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(TagWord::Table)
                    .if_not_exists()
                    .col(big_integer(TagWord::TagId))
                    .col(string(TagWord::Word))
                    .primary_key(Index::create().col(TagWord::Word).col(TagWord::TagId))
                    .foreign_key(
                        ForeignKey::create()
                            .from(TagWord::Table, TagWord::TagId)
                            .to(Tag::Table, Tag::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_tag_word_tag")
                    .table(TagWord::Table)
                    .col(TagWord::TagId)
                    .col(TagWord::Word)
                    .to_owned(),
            )
            .await?;

        fill(manager).await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(TagWord::Table).to_owned())
            .await
    }
}

async fn fill(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    let connection = manager.get_connection();
    let backend = manager.get_database_backend();
    let mut last_id = i64::MIN;
    loop {
        let page = Query::select()
            .columns([Tag::Id, Tag::Name])
            .from(Tag::Table)
            .and_where(Expr::col(Tag::Id).gt(last_id))
            .order_by(Tag::Id, Order::Asc)
            .limit(FILL_PAGE)
            .to_owned();
        let rows = connection.query_all(backend.build(&page)).await?;
        let Some(last) = rows.last() else {
            return Ok(());
        };
        last_id = last.try_get("", "id")?;

        let mut words = Vec::new();
        for row in &rows {
            let id: i64 = row.try_get("", "id")?;
            let name: String = row.try_get("", "name")?;
            words.extend(search::name_words(&name).into_iter().map(|word| (id, word)));
        }
        for chunk in words.chunks(INSERT_CHUNK) {
            let mut insert = Query::insert()
                .into_table(TagWord::Table)
                .columns([TagWord::TagId, TagWord::Word])
                .to_owned();
            for (id, word) in chunk {
                insert
                    .values([(*id).into(), word.as_str().into()])
                    .map_err(|error| DbErr::Migration(error.to_string()))?;
            }
            connection.execute(backend.build(&insert)).await?;
        }
    }
}

#[derive(DeriveIden)]
enum TagWord {
    Table,
    TagId,
    Word,
}

#[derive(DeriveIden)]
enum Tag {
    Table,
    Id,
    Name,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use sea_orm::{ConnectionTrait, Database};
    use sea_orm_migration::MigratorTrait;

    use crate::migration::Migrator;

    #[tokio::test]
    async fn indexes_the_tags_imported_before_it() {
        let connection = Database::connect("sqlite::memory:").await.unwrap();
        let before = Migrator::migrations()
            .iter()
            .position(|migration| migration.name() == "m0025_tag_word")
            .expect("registered");
        Migrator::up(&connection, Some(u32::try_from(before).unwrap()))
            .await
            .unwrap();
        connection
            .execute_unprepared(
                "INSERT INTO tag (id, kind, name) VALUES (38, 'genre', 'Action'), \
                 (473528, 'director', 'Luc  Besson Besson')",
            )
            .await
            .unwrap();

        Migrator::up(&connection, None).await.unwrap();

        let rows = connection
            .query_all(sea_orm::Statement::from_string(
                connection.get_database_backend(),
                "SELECT tag_id, word FROM tag_word ORDER BY tag_id, word",
            ))
            .await
            .unwrap();
        let words: Vec<(i64, String)> = rows
            .iter()
            .map(|row| {
                (
                    row.try_get("", "tag_id").unwrap(),
                    row.try_get("", "word").unwrap(),
                )
            })
            .collect();
        assert_eq!(
            words,
            [
                (38, "action".to_owned()),
                (473_528, "besson".to_owned()),
                (473_528, "luc".to_owned())
            ]
        );
    }
}
