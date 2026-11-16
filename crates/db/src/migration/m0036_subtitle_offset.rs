//! The offset each viewer shifts a subtitle by.
//!
//! Per viewer, as the reference keeps it per account (`media_stream_settings`), and per track
//! rather than per work: an offset fits one file's timing, not every copy's. As for
//! `watch_state`, no foreign key reaches `users`; a row goes with its track. Keyed by track
//! first, so that deleting a track finds its rows by the key.

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
                    .table(SubtitleOffset::Table)
                    .if_not_exists()
                    .col(big_integer(SubtitleOffset::MediaStreamId))
                    .col(big_integer(SubtitleOffset::Viewer))
                    .col(integer(SubtitleOffset::OffsetMs))
                    .primary_key(
                        Index::create()
                            .col(SubtitleOffset::MediaStreamId)
                            .col(SubtitleOffset::Viewer),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(SubtitleOffset::Table, SubtitleOffset::MediaStreamId)
                            .to(MediaStream::Table, MediaStream::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(SubtitleOffset::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum SubtitleOffset {
    Table,
    MediaStreamId,
    Viewer,
    OffsetMs,
}

#[derive(DeriveIden)]
enum MediaStream {
    Table,
    Id,
}
