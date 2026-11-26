//! The subtitle pages a DVB teletext track declares, as ffprobe-plex prints them:
//! `888:fra:hearing_impaired,889:fra:subtitles`. A teletext track holds several pages, and the
//! one its subtitles are decoded from is chosen among these (`core::teletext`).
//!
//! Tracks stored before have none, and are decoded from every subtitle page until their file
//! is read again.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(MediaStream::Table)
                    .add_column(ColumnDef::new(MediaStream::TeletextPages).string())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(MediaStream::Table)
                    .drop_column(MediaStream::TeletextPages)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum MediaStream {
    Table,
    TeletextPages,
}
