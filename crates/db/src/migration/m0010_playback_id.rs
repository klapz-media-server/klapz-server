//! The player's own name for a playback, which timelines report against.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(PlaybackSession::Table)
                    .add_column(ColumnDef::new(PlaybackSession::PlaybackId).string())
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_playback_session_playback_id")
                    .table(PlaybackSession::Table)
                    .col(PlaybackSession::PlaybackId)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx_playback_session_playback_id")
                    .table(PlaybackSession::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(PlaybackSession::Table)
                    .drop_column(PlaybackSession::PlaybackId)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum PlaybackSession {
    Table,
    PlaybackId,
}
