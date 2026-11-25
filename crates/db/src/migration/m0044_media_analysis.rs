//! The files whose media is still to be read, and how far they are.
//!
//! A scan lists a file from its name, size and time, and queues it here; the background
//! analysis reads its headers (`stage` 0), then measures its packets (`stage` 1), and the row
//! goes once both are done. No row is a file read as far as it can be, which every file
//! already stored is: imported, or read by the scans that analyzed before listing. A row goes
//! with its file and with its library.

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
        // The order the queue is read in, and a library's count.
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_media_analysis_order")
                    .table(MediaAnalysis::Table)
                    .col(MediaAnalysis::Stage)
                    .col(MediaAnalysis::QueuedAt)
                    .col(MediaAnalysis::MediaPartId)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_media_analysis_section")
                    .table(MediaAnalysis::Table)
                    .col(MediaAnalysis::LibrarySectionId)
                    .col(MediaAnalysis::Stage)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(MediaAnalysis::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
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
