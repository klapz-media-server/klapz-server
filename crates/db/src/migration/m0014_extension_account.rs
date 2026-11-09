//! The account a user's connection is paired with on the extension's own service.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ExtensionAccount::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ExtensionAccount::Id)
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(ExtensionAccount::UserId)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ExtensionAccount::Extension)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ExtensionAccount::AccountId)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ExtensionAccount::AccountName)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ExtensionAccount::AccountToken)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ExtensionAccount::ConnectionUri)
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(ExtensionAccount::PublishedAt).big_integer())
                    .col(
                        ColumnDef::new(ExtensionAccount::CreatedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ExtensionAccount::UpdatedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;
        for (name, columns) in [
            (
                "idx_extension_account_user",
                [ExtensionAccount::UserId, ExtensionAccount::Extension],
            ),
            (
                "idx_extension_account_account",
                [ExtensionAccount::Extension, ExtensionAccount::AccountId],
            ),
        ] {
            let mut index = Index::create();
            index
                .if_not_exists()
                .unique()
                .name(name)
                .table(ExtensionAccount::Table);
            for column in columns {
                index.col(column);
            }
            manager.create_index(index).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ExtensionAccount::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum ExtensionAccount {
    Table,
    Id,
    UserId,
    Extension,
    AccountId,
    AccountName,
    AccountToken,
    ConnectionUri,
    PublishedAt,
    CreatedAt,
    UpdatedAt,
}
