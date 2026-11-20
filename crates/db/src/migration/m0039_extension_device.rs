//! The clients a user signed in from through an extension, each with a token of its own.
//!
//! Jellyfin's clients sign in with a password and keep the token they are given, one per
//! device; Plex's are handed the connection's own token, in `user_extension`. A device belongs
//! to its user's connection: switching the connection off or deleting the user removes it.

use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::{big_integer, string};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ExtensionDevice::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ExtensionDevice::Id)
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(big_integer(ExtensionDevice::UserId))
                    .col(string(ExtensionDevice::Extension))
                    .col(string(ExtensionDevice::DeviceId))
                    .col(string(ExtensionDevice::DeviceName))
                    .col(string(ExtensionDevice::Client))
                    .col(string(ExtensionDevice::ClientVersion))
                    .col(string(ExtensionDevice::TokenDigest))
                    .col(big_integer(ExtensionDevice::CreatedAt))
                    .col(big_integer(ExtensionDevice::UpdatedAt))
                    .to_owned(),
            )
            .await?;
        for (name, columns) in [
            (
                "idx_extension_device_device",
                vec![
                    ExtensionDevice::UserId,
                    ExtensionDevice::Extension,
                    ExtensionDevice::DeviceId,
                ],
            ),
            (
                "idx_extension_device_token",
                vec![ExtensionDevice::Extension, ExtensionDevice::TokenDigest],
            ),
        ] {
            let mut index = Index::create();
            index
                .if_not_exists()
                .unique()
                .name(name)
                .table(ExtensionDevice::Table);
            for column in columns {
                index.col(column);
            }
            manager.create_index(index).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ExtensionDevice::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum ExtensionDevice {
    Table,
    Id,
    UserId,
    Extension,
    DeviceId,
    DeviceName,
    Client,
    ClientVersion,
    TokenDigest,
    CreatedAt,
    UpdatedAt,
}
