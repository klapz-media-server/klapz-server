//! The sessions being played.
//!
//! The reference keeps these in memory. Here they are stored, with an expiry that every sign of
//! life pushes forward, so that what a crash leaves behind goes away by itself.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(PlaybackSession::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(PlaybackSession::Id)
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(PlaybackSession::SessionKey)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PlaybackSession::ItemId)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PlaybackSession::PartId)
                            .big_integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(PlaybackSession::Client).string())
                    .col(ColumnDef::new(PlaybackSession::Product).string())
                    .col(ColumnDef::new(PlaybackSession::State).string().not_null())
                    .col(
                        ColumnDef::new(PlaybackSession::VideoDecision)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PlaybackSession::AudioDecision)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PlaybackSession::PositionMs)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PlaybackSession::StartedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PlaybackSession::UpdatedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PlaybackSession::ExpiresAt)
                            .big_integer()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .unique()
                    .name("idx_playback_session_key")
                    .table(PlaybackSession::Table)
                    .col(PlaybackSession::SessionKey)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_playback_session_expiry")
                    .table(PlaybackSession::Table)
                    .col(PlaybackSession::ExpiresAt)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(PlaybackSession::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum PlaybackSession {
    Table,
    Id,
    SessionKey,
    ItemId,
    PartId,
    Client,
    Product,
    State,
    VideoDecision,
    AudioDecision,
    PositionMs,
    StartedAt,
    UpdatedAt,
    ExpiresAt,
}
