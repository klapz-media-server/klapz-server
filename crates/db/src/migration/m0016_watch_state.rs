//! What each viewer has watched, and the playbacks under way.
//!
//! A viewer is a user's id, or 0 for the operator, who has no user row: a NULL in a key would
//! not be unique, and the operator watches like anyone else. Hence no foreign key to `users`;
//! deleting a user deletes its rows explicitly.

use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::{big_integer, big_integer_null, boolean, integer, string};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(WatchState::Table)
                    .if_not_exists()
                    .col(big_integer(WatchState::Viewer))
                    // The work, not the item: the same film in several libraries is one row.
                    .col(string(WatchState::Guid))
                    .col(integer(WatchState::ViewCount).default(0))
                    .col(big_integer_null(WatchState::OffsetMs))
                    .col(big_integer_null(WatchState::LastViewedAt))
                    .primary_key(
                        Index::create()
                            .col(WatchState::Viewer)
                            .col(WatchState::Guid),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Viewing::Table)
                    .if_not_exists()
                    .col(big_integer(Viewing::Viewer))
                    // The player's identifier for the playback, under the caller's namespace.
                    .col(string(Viewing::Playback))
                    .col(big_integer(Viewing::ItemId))
                    .col(big_integer(Viewing::PositionMs))
                    .col(boolean(Viewing::Completed))
                    .col(big_integer(Viewing::ExpiresAt))
                    .primary_key(Index::create().col(Viewing::Viewer).col(Viewing::Playback))
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Viewing::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(WatchState::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum WatchState {
    Table,
    Viewer,
    Guid,
    ViewCount,
    OffsetMs,
    LastViewedAt,
}

#[derive(DeriveIden)]
enum Viewing {
    Table,
    Viewer,
    Playback,
    ItemId,
    PositionMs,
    Completed,
    ExpiresAt,
}
