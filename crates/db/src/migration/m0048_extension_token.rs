//! A connection keeps no token of its own any more.
//!
//! Plex clients present the token plex.tv hands the account a user paired, and Jellyfin's each
//! get one for their device, in `extension_device`: the digest `user_extension` kept opened
//! nothing anyone was given. Undone, the column comes back empty and without its index, so a
//! connection opens nothing through it.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const TOKEN_INDEX: &str = "idx_user_extension_token";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name(TOKEN_INDEX)
                    .table(UserExtension::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(UserExtension::Table)
                    .drop_column(UserExtension::TokenDigest)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(UserExtension::Table)
                    .add_column(
                        ColumnDef::new(UserExtension::TokenDigest)
                            .string()
                            .not_null()
                            .default(""),
                    )
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum UserExtension {
    Table,
    TokenDigest,
}
