//! What a session sends to its player: the bitrates and the picture size.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // One column per statement: SQLite cannot add several at once.
        for column in [
            PlaybackSession::VideoBitrate,
            PlaybackSession::AudioBitrate,
            PlaybackSession::Width,
            PlaybackSession::Height,
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(PlaybackSession::Table)
                        .add_column(ColumnDef::new(column).integer())
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for column in [
            PlaybackSession::VideoBitrate,
            PlaybackSession::AudioBitrate,
            PlaybackSession::Width,
            PlaybackSession::Height,
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(PlaybackSession::Table)
                        .drop_column(column)
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}

#[derive(DeriveIden)]
enum PlaybackSession {
    Table,
    VideoBitrate,
    AudioBitrate,
    Width,
    Height,
}
