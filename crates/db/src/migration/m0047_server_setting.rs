//! The values owners chose for the server's settings.
//!
//! One row per setting an owner set, named by its dotted key (`transcoder.h264.preset`);
//! a setting back to its default has no row, its default living in `core::settings`. The key
//! is stored as `name`, `key` being a reserved word in `MySQL`. As for `watch_state`, no foreign
//! key reaches `users`, which migrations rebuild in place: a user deleted since is still named
//! in `updated_by`.

use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::{big_integer, big_integer_null, string};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ServerSetting::Table)
                    .if_not_exists()
                    .col(string(ServerSetting::Name).primary_key())
                    .col(string(ServerSetting::Value))
                    .col(big_integer(ServerSetting::UpdatedAt))
                    .col(big_integer_null(ServerSetting::UpdatedBy))
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ServerSetting::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum ServerSetting {
    Table,
    Name,
    Value,
    UpdatedAt,
    UpdatedBy,
}
