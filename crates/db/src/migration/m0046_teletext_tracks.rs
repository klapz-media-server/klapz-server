//! A teletext track is stored as one track per subtitle page it carries, each stating its own
//! page (`core::teletext`). Tracks stored before are one per stream, with every page or none:
//! their files are queued to have their headers read again, which stores them anew.

use sea_orm::ConnectionTrait;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // `media_analysis::HEADERS` is 0; a file already queued keeps its place.
        manager
            .get_connection()
            .execute_unprepared(
                "INSERT INTO media_analysis (media_part_id, library_section_id, stage, queued_at) \
                 SELECT DISTINCT media_part.id, metadata_item.library_section_id, 0, 0 \
                 FROM media_stream \
                 JOIN media_part ON media_part.id = media_stream.media_part_id \
                 JOIN media ON media.id = media_part.media_id \
                 JOIN metadata_item ON metadata_item.id = media.metadata_item_id \
                 WHERE media_stream.codec = 'dvb_teletext' \
                 AND media_stream.external_path IS NULL \
                 AND NOT EXISTS (SELECT 1 FROM media_analysis queued \
                 WHERE queued.media_part_id = media_part.id)",
            )
            .await
            .map(drop)
    }

    async fn down(&self, _: &SchemaManager) -> Result<(), DbErr> {
        // Reading a file again undoes nothing.
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
    async fn queues_the_files_of_teletext_tracks_stored_before_it() {
        let connection = Database::connect("sqlite::memory:").await.unwrap();
        let before = Migrator::migrations()
            .iter()
            .position(|migration| migration.name() == "m0046_teletext_tracks")
            .expect("registered");
        Migrator::up(&connection, Some(u32::try_from(before).unwrap()))
            .await
            .unwrap();
        connection
            .execute_unprepared(
                "INSERT INTO library_section (id, kind, title, language, uuid, created_at, \
                 updated_at, content_changed_at, hidden) \
                 VALUES (4, 'movie', 'Films', 'en', 'u', 0, 0, 0, false); \
                 INSERT INTO metadata_item (id, library_section_id, kind, guid, sort_key, \
                 added_at, updated_at) VALUES (1, 4, 'movie', 'a', 'a', 0, 0), \
                 (2, 4, 'movie', 'b', 'b', 0, 0), (3, 4, 'movie', 'c', 'c', 0, 0); \
                 INSERT INTO media (id, metadata_item_id, interlaced, optimized_for_streaming, \
                 created_at, updated_at) VALUES (1, 1, false, false, 0, 0), \
                 (2, 2, false, false, 0, 0), (3, 3, false, false, 0, 0); \
                 INSERT INTO media_part (id, media_id, position, path, created_at, updated_at) \
                 VALUES (1, 1, 0, 'a.ts', 0, 0), (2, 2, 0, 'b.mkv', 0, 0), \
                 (3, 3, 0, 'c.ts', 0, 0); \
                 INSERT INTO media_stream (id, media_part_id, kind, position, codec, \
                 is_default, is_forced) \
                 VALUES (1, 1, 'subtitle', 0, 'dvb_teletext', false, false), \
                 (2, 2, 'subtitle', 0, 'subrip', false, false), \
                 (3, 3, 'subtitle', 0, 'dvb_teletext', false, false); \
                 INSERT INTO media_analysis (media_part_id, library_section_id, stage, queued_at) \
                 VALUES (3, 4, 1, 99)",
            )
            .await
            .unwrap();

        // This one alone: the queue changes shape after it.
        Migrator::up(&connection, Some(1)).await.unwrap();

        let rows = connection
            .query_all(Statement::from_string(
                connection.get_database_backend(),
                "SELECT media_part_id, library_section_id, stage, queued_at FROM media_analysis \
                 ORDER BY media_part_id",
            ))
            .await
            .unwrap();
        let queued: Vec<(i64, i64, i32, i64)> = rows
            .iter()
            .map(|row| {
                (
                    row.try_get("", "media_part_id").unwrap(),
                    row.try_get("", "library_section_id").unwrap(),
                    row.try_get("", "stage").unwrap(),
                    row.try_get("", "queued_at").unwrap(),
                )
            })
            .collect();
        // The teletext file is read again; the subrip one is not, and the one already
        // queued keeps its place.
        assert_eq!(queued, [(1, 4, 0, 0), (3, 4, 1, 99)]);
    }
}
