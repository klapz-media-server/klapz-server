//! Every language a track or a library stores becomes the canonical tag of
//! [`klapz_core::language::LanguageTag`]: `eng` and `fre` are `en` and `fr`, `fr_FR` is `fr-FR`,
//! so that one language is one value to filter on. What names no language (`unknown`) is
//! cleared from a track; a library keeps a value that does not read, since it needs one. What a
//! file stated stays as written beside it (`declared_language`).

use sea_orm::{ConnectionTrait, Statement, Value};
use sea_orm_migration::prelude::*;

use crate::language::canonical;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// The tables and columns, and whether a value that does not read is cleared.
const COLUMNS: [(&str, &str, bool); 3] = [
    ("media_stream", "language", true),
    ("extra_track", "language", true),
    ("library_section", "language", false),
];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        let backend = connection.get_database_backend();
        for (table, column, clear) in COLUMNS {
            let stored = connection
                .query_all(Statement::from_string(
                    backend,
                    format!("SELECT DISTINCT {column} FROM {table} WHERE {column} IS NOT NULL"),
                ))
                .await?;
            for row in stored {
                let written: String = row.try_get_by_index(0)?;
                let rewritten = match canonical(Some(&written)) {
                    Some(tag) if tag == written => continue,
                    Some(tag) => Value::from(tag),
                    None if clear => Value::String(None),
                    None => continue,
                };
                let placeholder = |position: u32| match backend {
                    sea_orm::DatabaseBackend::Postgres => format!("${position}"),
                    _ => "?".to_owned(),
                };
                connection
                    .execute(Statement::from_sql_and_values(
                        backend,
                        format!(
                            "UPDATE {table} SET {column} = {} WHERE {column} = {}",
                            placeholder(1),
                            placeholder(2)
                        ),
                        [rewritten, Value::from(written)],
                    ))
                    .await?;
            }
        }
        Ok(())
    }

    async fn down(&self, _: &SchemaManager) -> Result<(), DbErr> {
        // The tags written before are equivalent to the ones written now.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::super::Migrator;
    use sea_orm::{ConnectionTrait, Database, Statement};
    use sea_orm_migration::MigratorTrait;

    #[tokio::test]
    async fn rewrites_every_stored_language_as_its_canonical_tag() {
        let connection = Database::connect("sqlite::memory:").await.unwrap();
        let before = Migrator::migrations()
            .iter()
            .position(|migration| migration.name() == "m0051_canonical_languages")
            .expect("registered");
        Migrator::up(&connection, Some(u32::try_from(before).unwrap()))
            .await
            .unwrap();
        connection
            .execute_unprepared(
                "INSERT INTO library_section (id, kind, title, language, uuid, created_at, \
                 updated_at, content_changed_at, hidden) \
                 VALUES (4, 'movie', 'Films', 'fr_FR', 'u', 0, 0, 0, false); \
                 INSERT INTO metadata_item (id, library_section_id, kind, guid, sort_key, \
                 added_at, updated_at) VALUES (1, 4, 'movie', 'a', 'a', 0, 0); \
                 INSERT INTO media (id, metadata_item_id, interlaced, optimized_for_streaming, \
                 created_at, updated_at) VALUES (1, 1, false, false, 0, 0); \
                 INSERT INTO media_part (id, media_id, position, path, created_at, updated_at) \
                 VALUES (1, 1, 0, 'a.mkv', 0, 0); \
                 INSERT INTO media_stream (id, media_part_id, kind, position, language, \
                 declared_language, is_default, is_forced) VALUES \
                 (1, 1, 'audio', 1, 'eng', 'eng', false, false), \
                 (2, 1, 'audio', 2, 'fr-CA', NULL, false, false), \
                 (3, 1, 'audio', 3, 'french', NULL, false, false), \
                 (4, 1, 'audio', 4, 'unknown', 'unknown', false, false), \
                 (5, 1, 'subtitle', 5, 'zh-hant', NULL, false, false)",
            )
            .await
            .unwrap();
        Migrator::up(&connection, None).await.unwrap();

        let rows = connection
            .query_all(Statement::from_string(
                connection.get_database_backend(),
                "SELECT language, declared_language FROM media_stream ORDER BY id",
            ))
            .await
            .unwrap();
        let stored: Vec<(Option<String>, Option<String>)> = rows
            .iter()
            .map(|row| {
                (
                    row.try_get_by_index(0).unwrap(),
                    row.try_get_by_index(1).unwrap(),
                )
            })
            .collect();
        let some = |text: &str| Some(text.to_owned());
        assert_eq!(
            stored,
            [
                (some("en"), some("eng")),
                (some("fr-CA"), None),
                (some("fr"), None),
                (None, some("unknown")),
                (some("zh-Hant"), None),
            ]
        );
        let library = connection
            .query_one(Statement::from_string(
                connection.get_database_backend(),
                "SELECT language FROM library_section",
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(library.try_get_by_index::<String>(0).unwrap(), "fr-FR");
    }
}
