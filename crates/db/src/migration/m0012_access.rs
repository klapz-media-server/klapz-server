//! Users, their permissions on libraries, and the extensions they connect through. Sessions and
//! play queues gain an owner, which stays empty for the server's own operator.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    #[allow(
        clippy::too_many_lines,
        reason = "one table after another: splitting it would only move the list"
    )]
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Users::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Users::Id)
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Users::Email).string().not_null())
                    .col(ColumnDef::new(Users::CreatedAt).big_integer().not_null())
                    .col(ColumnDef::new(Users::UpdatedAt).big_integer().not_null())
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .unique()
                    .name("idx_users_email")
                    .table(Users::Table)
                    .col(Users::Email)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(LibraryPermission::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(LibraryPermission::Id)
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(LibraryPermission::UserId)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(LibraryPermission::SectionId)
                            .big_integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(LibraryPermission::Role).string().not_null())
                    .col(
                        ColumnDef::new(LibraryPermission::CreatedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(LibraryPermission::UpdatedAt)
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
                    .name("idx_library_permission_user_section")
                    .table(LibraryPermission::Table)
                    .col(LibraryPermission::UserId)
                    .col(LibraryPermission::SectionId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(UserExtension::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(UserExtension::Id)
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(UserExtension::UserId)
                            .big_integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(UserExtension::Extension).string().not_null())
                    .col(
                        ColumnDef::new(UserExtension::ExternalId)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(UserExtension::TokenDigest)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(UserExtension::CreatedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(UserExtension::UpdatedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;
        for (name, columns) in [
            (
                "idx_user_extension_user",
                vec![UserExtension::UserId, UserExtension::Extension],
            ),
            (
                "idx_user_extension_external_id",
                vec![UserExtension::Extension, UserExtension::ExternalId],
            ),
            (
                "idx_user_extension_token",
                vec![UserExtension::Extension, UserExtension::TokenDigest],
            ),
        ] {
            let mut index = Index::create();
            index
                .if_not_exists()
                .unique()
                .name(name)
                .table(UserExtension::Table);
            for column in columns {
                index.col(column);
            }
            manager.create_index(index).await?;
        }

        manager
            .alter_table(
                Table::alter()
                    .table(PlaybackSession::Table)
                    .add_column(ColumnDef::new(PlaybackSession::UserId).big_integer())
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(PlayQueue::Table)
                    .add_column(ColumnDef::new(PlayQueue::UserId).big_integer())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(PlayQueue::Table)
                    .drop_column(PlayQueue::UserId)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(PlaybackSession::Table)
                    .drop_column(PlaybackSession::UserId)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(UserExtension::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(LibraryPermission::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Users::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Users {
    Table,
    Id,
    Email,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum LibraryPermission {
    Table,
    Id,
    UserId,
    SectionId,
    Role,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum UserExtension {
    Table,
    Id,
    UserId,
    Extension,
    ExternalId,
    TokenDigest,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum PlaybackSession {
    Table,
    UserId,
}

#[derive(DeriveIden)]
enum PlayQueue {
    Table,
    UserId,
}
