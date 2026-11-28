//! The analysis queue holds tasks rather than stages: one row per task left on a file
//! (`media_task::HEADERS`, `PACKETS`, `SUBTITLES`, `PREVIEW_THUMBNAILS`), so that a task an
//! owner turns off stays queued apart from the others, and is done once turned on.
//!
//! A file whose headers were read keeps its packets to measure, and is given the extractions
//! its stored tracks call for. A file read to its end before has nothing queued: a forced scan
//! queues it again, extractions included. Imported files are left alone, as their paths are
//! another server's.

use sea_orm::ConnectionTrait;
use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::{big_integer, integer};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(MediaTask::Table)
                    .if_not_exists()
                    .col(big_integer(MediaTask::MediaPartId))
                    .col(integer(MediaTask::Task))
                    .col(big_integer(MediaTask::LibrarySectionId))
                    .col(big_integer(MediaTask::QueuedAt))
                    .primary_key(
                        Index::create()
                            .col(MediaTask::MediaPartId)
                            .col(MediaTask::Task),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(MediaTask::Table, MediaTask::MediaPartId)
                            .to(MediaPart::Table, MediaPart::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(MediaTask::Table, MediaTask::LibrarySectionId)
                            .to(LibrarySection::Table, LibrarySection::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        // The order the queue is taken in, and a library's count.
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_media_task_order")
                    .table(MediaTask::Table)
                    .col(MediaTask::Task)
                    .col(MediaTask::QueuedAt)
                    .col(MediaTask::MediaPartId)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_media_task_section")
                    .table(MediaTask::Table)
                    .col(MediaTask::LibrarySectionId)
                    .col(MediaTask::Task)
                    .to_owned(),
            )
            .await?;

        // Stages 0 and 1 are tasks 0 and 1; 2 extracts subtitles, 3 makes thumbnails.
        let connection = manager.get_connection();
        connection
            .execute_unprepared(
                "INSERT INTO media_task (media_part_id, task, library_section_id, queued_at) \
                 SELECT media_part_id, stage, library_section_id, queued_at FROM media_analysis",
            )
            .await?;
        for (task, kind) in [(2, "subtitle"), (3, "video")] {
            connection
                .execute_unprepared(&format!(
                    "INSERT INTO media_task (media_part_id, task, library_section_id, queued_at) \
                     SELECT queued.media_part_id, {task}, queued.library_section_id, \
                     queued.queued_at FROM media_analysis queued \
                     WHERE queued.stage = 1 AND EXISTS (SELECT 1 FROM media_stream \
                     WHERE media_stream.media_part_id = queued.media_part_id \
                     AND media_stream.kind = '{kind}' AND media_stream.external_path IS NULL)"
                ))
                .await?;
        }
        manager
            .drop_table(Table::drop().table(MediaAnalysis::Table).to_owned())
            .await
    }

    /// Back to one stage per file, its first task left; extractions, which stages cannot
    /// hold, are dropped.
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(MediaAnalysis::Table)
                    .if_not_exists()
                    .col(big_integer(MediaAnalysis::MediaPartId).primary_key())
                    .col(big_integer(MediaAnalysis::LibrarySectionId))
                    .col(integer(MediaAnalysis::Stage))
                    .col(big_integer(MediaAnalysis::QueuedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(MediaAnalysis::Table, MediaAnalysis::MediaPartId)
                            .to(MediaPart::Table, MediaPart::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(MediaAnalysis::Table, MediaAnalysis::LibrarySectionId)
                            .to(LibrarySection::Table, LibrarySection::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        for (name, columns) in [
            (
                "idx_media_analysis_order",
                [
                    MediaAnalysis::Stage,
                    MediaAnalysis::QueuedAt,
                    MediaAnalysis::MediaPartId,
                ]
                .as_slice(),
            ),
            (
                "idx_media_analysis_section",
                [MediaAnalysis::LibrarySectionId, MediaAnalysis::Stage].as_slice(),
            ),
        ] {
            let mut index = Index::create();
            index.if_not_exists().name(name).table(MediaAnalysis::Table);
            for column in columns {
                index.col(*column);
            }
            manager.create_index(index).await?;
        }
        manager
            .get_connection()
            .execute_unprepared(
                "INSERT INTO media_analysis (media_part_id, library_section_id, stage, queued_at) \
                 SELECT media_part_id, MIN(library_section_id), MIN(task), MIN(queued_at) \
                 FROM media_task WHERE task <= 1 GROUP BY media_part_id",
            )
            .await?;
        manager
            .drop_table(Table::drop().table(MediaTask::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum MediaTask {
    Table,
    MediaPartId,
    Task,
    LibrarySectionId,
    QueuedAt,
}

#[derive(DeriveIden, Clone, Copy)]
enum MediaAnalysis {
    Table,
    MediaPartId,
    LibrarySectionId,
    Stage,
    QueuedAt,
}

#[derive(DeriveIden)]
enum MediaPart {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum LibrarySection {
    Table,
    Id,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::super::Migrator;
    use sea_orm::{ConnectionTrait, Database, DatabaseConnection, Statement};
    use sea_orm_migration::MigratorTrait;

    async fn rows(connection: &DatabaseConnection, sql: &str) -> Vec<(i64, i32, i64)> {
        connection
            .query_all(Statement::from_string(
                connection.get_database_backend(),
                sql,
            ))
            .await
            .unwrap()
            .iter()
            .map(|row| {
                (
                    row.try_get_by_index(0).unwrap(),
                    row.try_get_by_index(1).unwrap(),
                    row.try_get_by_index(2).unwrap(),
                )
            })
            .collect()
    }

    #[tokio::test]
    async fn stages_become_tasks_and_a_read_file_is_given_its_extractions() {
        let connection = Database::connect("sqlite::memory:").await.unwrap();
        let before = Migrator::migrations()
            .iter()
            .position(|migration| migration.name() == "m0049_media_task")
            .expect("registered");
        Migrator::up(&connection, Some(u32::try_from(before).unwrap()))
            .await
            .unwrap();
        // Part 1 waits for its headers; parts 2 and 3 for their packets, 2 with a subtitle
        // track and a video one, 3 with a sidecar alone.
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
                 VALUES (1, 1, 0, 'a.mkv', 0, 0), (2, 2, 0, 'b.mkv', 0, 0), \
                 (3, 3, 0, 'c.mkv', 0, 0); \
                 INSERT INTO media_stream (id, media_part_id, kind, position, codec, \
                 is_default, is_forced, external_path) \
                 VALUES (1, 2, 'video', 0, 'h264', false, false, NULL), \
                 (2, 2, 'subtitle', 1, 'subrip', false, false, NULL), \
                 (3, 3, 'subtitle', 0, 'srt', false, false, 'c.srt'); \
                 INSERT INTO media_analysis (media_part_id, library_section_id, stage, queued_at) \
                 VALUES (1, 4, 0, 10), (2, 4, 1, 20), (3, 4, 1, 30)",
            )
            .await
            .unwrap();

        Migrator::up(&connection, Some(1)).await.unwrap();
        assert_eq!(
            rows(
                &connection,
                "SELECT media_part_id, task, queued_at FROM media_task \
                 ORDER BY media_part_id, task"
            )
            .await,
            [(1, 0, 10), (2, 1, 20), (2, 2, 20), (2, 3, 20), (3, 1, 30)]
        );

        Migrator::down(&connection, Some(1)).await.unwrap();
        assert_eq!(
            rows(
                &connection,
                "SELECT media_part_id, stage, queued_at FROM media_analysis \
                 ORDER BY media_part_id"
            )
            .await,
            [(1, 0, 10), (2, 1, 20), (3, 1, 30)]
        );
    }
}
