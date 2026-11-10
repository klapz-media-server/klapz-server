//! The audio and subtitle track each viewer chose on a file.
//!
//! A viewer is a user's id or 0 for the operator, as in `watch_state`, so there is no foreign
//! key to `users`. A row goes with its file. `subtitle_off` says "no subtitles" apart from "no
//! choice", which a NULL `subtitle_stream_id` alone cannot.

use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::{big_integer, big_integer_null, boolean};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(TrackChoice::Table)
                    .if_not_exists()
                    .col(big_integer(TrackChoice::Viewer))
                    .col(big_integer(TrackChoice::MediaPartId))
                    .col(big_integer_null(TrackChoice::AudioStreamId))
                    .col(big_integer_null(TrackChoice::SubtitleStreamId))
                    .col(boolean(TrackChoice::SubtitleOff).default(false))
                    .primary_key(
                        Index::create()
                            .col(TrackChoice::Viewer)
                            .col(TrackChoice::MediaPartId),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(TrackChoice::Table, TrackChoice::MediaPartId)
                            .to(MediaPart::Table, MediaPart::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(TrackChoice::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum TrackChoice {
    Table,
    Viewer,
    MediaPartId,
    AudioStreamId,
    SubtitleStreamId,
    SubtitleOff,
}

#[derive(DeriveIden)]
enum MediaPart {
    Table,
    Id,
}
