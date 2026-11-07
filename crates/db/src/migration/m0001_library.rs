use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::{big_integer, big_integer_null, boolean, string, string_null};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(LibrarySection::Table)
                    .if_not_exists()
                    // Not auto-incremented: ids are part of the API surface and are
                    // preserved across an import so client links keep resolving.
                    .col(big_integer(LibrarySection::Id).primary_key())
                    .col(string(LibrarySection::Kind))
                    .col(string(LibrarySection::Title))
                    .col(string_null(LibrarySection::TitleSort))
                    .col(string(LibrarySection::Language))
                    .col(string(LibrarySection::Uuid).unique_key())
                    .col(big_integer(LibrarySection::CreatedAt))
                    .col(big_integer(LibrarySection::UpdatedAt))
                    .col(big_integer_null(LibrarySection::ScannedAt))
                    .col(big_integer(LibrarySection::ContentChangedAt))
                    .col(boolean(LibrarySection::Hidden))
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(SectionRoot::Table)
                    .if_not_exists()
                    .col(big_integer(SectionRoot::Id).primary_key())
                    .col(big_integer(SectionRoot::SectionId))
                    .col(string(SectionRoot::Path))
                    .col(big_integer(SectionRoot::CreatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_section_root_section")
                            .from(SectionRoot::Table, SectionRoot::SectionId)
                            .to(LibrarySection::Table, LibrarySection::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // Roots are looked up by section on every listing, and scanned against on every
        // file request.
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_section_root_section")
                    .table(SectionRoot::Table)
                    .col(SectionRoot::SectionId)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(SectionRoot::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(LibrarySection::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum LibrarySection {
    Table,
    Id,
    Kind,
    Title,
    TitleSort,
    Language,
    Uuid,
    CreatedAt,
    UpdatedAt,
    ScannedAt,
    ContentChangedAt,
    Hidden,
}

#[derive(DeriveIden)]
enum SectionRoot {
    Table,
    Id,
    SectionId,
    Path,
    CreatedAt,
}
