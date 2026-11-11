//! A session the operator terminated: when, and what its player is told.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // One column per statement: SQLite cannot add several at once.
        manager
            .alter_table(
                Table::alter()
                    .table(PlaybackSession::Table)
                    .add_column(ColumnDef::new(PlaybackSession::TerminatedAt).big_integer())
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(PlaybackSession::Table)
                    .add_column(ColumnDef::new(PlaybackSession::TerminationReason).text())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for column in [
            PlaybackSession::TerminatedAt,
            PlaybackSession::TerminationReason,
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
    TerminatedAt,
    TerminationReason,
}
