//! Each viewer's own rating of a work.
//!
//! Keyed like `watch_state`, by viewer and `guid`, so that every copy of a work shares one
//! rating. A cleared rating keeps its row with a NULL `rating` and the time it was cleared, so
//! that an older change arriving later is recognized as older. As for `watch_state`, no foreign
//! key reaches `users`, which is rebuilt in place by migrations; deleting a user deletes its
//! rows explicitly.

use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::{big_integer, double_null, string};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(UserRating::Table)
                    .if_not_exists()
                    .col(big_integer(UserRating::Viewer))
                    .col(string(UserRating::Guid))
                    .col(double_null(UserRating::Rating))
                    .col(big_integer(UserRating::RatedAt))
                    .primary_key(
                        Index::create()
                            .col(UserRating::Viewer)
                            .col(UserRating::Guid),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(UserRating::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum UserRating {
    Table,
    Viewer,
    Guid,
    Rating,
    RatedAt,
}
