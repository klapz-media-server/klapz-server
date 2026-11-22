//! The works each viewer made a favorite.
//!
//! Keyed like `user_rating`, by viewer and `guid`, so that every copy of a work is a favorite
//! at once. A row is a favorite; taking it back deletes the row. As for `watch_state`, no
//! foreign key reaches `users`, which is rebuilt in place by migrations; deleting a user
//! deletes its rows explicitly.

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
                    .table(UserFavorite::Table)
                    .if_not_exists()
                    .col(big_integer(UserFavorite::Viewer))
                    .col(string(UserFavorite::Guid))
                    .col(big_integer(UserFavorite::AddedAt))
                    .primary_key(
                        Index::create()
                            .col(UserFavorite::Viewer)
                            .col(UserFavorite::Guid),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(UserFavorite::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum UserFavorite {
    Table,
    Viewer,
    Guid,
    AddedAt,
}
