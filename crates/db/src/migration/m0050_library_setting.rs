//! The values owners chose for one library in place of the server's, for the settings
//! `core::settings` declares per library: one row per library and setting, none while the
//! library follows the server. A row goes with its library; as in `server_setting`, no
//! foreign key reaches `users`.

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
                    .table(LibrarySetting::Table)
                    .if_not_exists()
                    .col(big_integer(LibrarySetting::LibrarySectionId))
                    .col(string(LibrarySetting::Name))
                    .col(string(LibrarySetting::Value))
                    .col(big_integer(LibrarySetting::UpdatedAt))
                    .col(big_integer_null(LibrarySetting::UpdatedBy))
                    .primary_key(
                        Index::create()
                            .col(LibrarySetting::LibrarySectionId)
                            .col(LibrarySetting::Name),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(LibrarySetting::Table, LibrarySetting::LibrarySectionId)
                            .to(LibrarySection::Table, LibrarySection::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        // Every library's value of a setting, as the analysis reads them.
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_library_setting_name")
                    .table(LibrarySetting::Table)
                    .col(LibrarySetting::Name)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(LibrarySetting::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum LibrarySetting {
    Table,
    LibrarySectionId,
    Name,
    Value,
    UpdatedAt,
    UpdatedBy,
}

#[derive(DeriveIden)]
enum LibrarySection {
    Table,
    Id,
}
