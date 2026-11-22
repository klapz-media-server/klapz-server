//! Source metadata: works as providers describe them, apart from the library items that hold
//! them (`core::source_metadata`).
//!
//! A record is keyed by its provider and its id there, so a refresh writes over it and two
//! libraries matching the same film share one. Its texts are kept in every language the
//! provider has, and its images by address only: which are downloaded is the items' business.
//! An item points at its record through `item_source`, a table rather than a column so that the
//! item's own rows, written by the importer and the scan, are left as they are.

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
                    .table(SourceMetadata::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(SourceMetadata::Id)
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(SourceMetadata::Agent)
                            .string_len(16)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(SourceMetadata::Kind)
                            .string_len(8)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(SourceMetadata::ProviderId)
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(SourceMetadata::ParentId).big_integer())
                    .col(ColumnDef::new(SourceMetadata::Position).integer())
                    .col(ColumnDef::new(SourceMetadata::OriginalLanguage).string_len(16))
                    .col(ColumnDef::new(SourceMetadata::OriginalTitle).text())
                    .col(ColumnDef::new(SourceMetadata::Year).integer())
                    .col(ColumnDef::new(SourceMetadata::ReleasedOn).big_integer())
                    .col(ColumnDef::new(SourceMetadata::DurationMs).big_integer())
                    .col(ColumnDef::new(SourceMetadata::Studio).text())
                    .col(ColumnDef::new(SourceMetadata::Rating).double())
                    .col(
                        ColumnDef::new(SourceMetadata::RefreshedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(SourceMetadata::Table, SourceMetadata::ParentId)
                            .to(SourceMetadata::Table, SourceMetadata::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_source_metadata_provider")
                    .table(SourceMetadata::Table)
                    .col(SourceMetadata::Agent)
                    .col(SourceMetadata::Kind)
                    .col(SourceMetadata::ProviderId)
                    .unique()
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_source_metadata_parent")
                    .table(SourceMetadata::Table)
                    .col(SourceMetadata::ParentId)
                    .col(SourceMetadata::Position)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(SourceMetadataTranslation::Table)
                    .if_not_exists()
                    .col(source_id(SourceMetadataTranslation::SourceMetadataId))
                    .col(
                        ColumnDef::new(SourceMetadataTranslation::Language)
                            .string_len(16)
                            .not_null(),
                    )
                    .col(ColumnDef::new(SourceMetadataTranslation::Title).text())
                    .col(ColumnDef::new(SourceMetadataTranslation::Summary).text())
                    .col(ColumnDef::new(SourceMetadataTranslation::Tagline).text())
                    .primary_key(
                        Index::create()
                            .col(SourceMetadataTranslation::SourceMetadataId)
                            .col(SourceMetadataTranslation::Language),
                    )
                    .foreign_key(&mut owned_by(
                        SourceMetadataTranslation::Table,
                        SourceMetadataTranslation::SourceMetadataId,
                    ))
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(SourceMetadataExternalId::Table)
                    .if_not_exists()
                    .col(source_id(SourceMetadataExternalId::SourceMetadataId))
                    .col(
                        ColumnDef::new(SourceMetadataExternalId::Source)
                            .string_len(16)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(SourceMetadataExternalId::Value)
                            .string()
                            .not_null(),
                    )
                    .primary_key(
                        Index::create()
                            .col(SourceMetadataExternalId::SourceMetadataId)
                            .col(SourceMetadataExternalId::Source)
                            .col(SourceMetadataExternalId::Value),
                    )
                    .foreign_key(&mut owned_by(
                        SourceMetadataExternalId::Table,
                        SourceMetadataExternalId::SourceMetadataId,
                    ))
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_source_metadata_external_id_lookup")
                    .table(SourceMetadataExternalId::Table)
                    .col(SourceMetadataExternalId::Source)
                    .col(SourceMetadataExternalId::Value)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(SourceMetadataCertification::Table)
                    .if_not_exists()
                    .col(source_id(SourceMetadataCertification::SourceMetadataId))
                    .col(
                        ColumnDef::new(SourceMetadataCertification::Country)
                            .string_len(2)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(SourceMetadataCertification::Rating)
                            .string()
                            .not_null(),
                    )
                    .primary_key(
                        Index::create()
                            .col(SourceMetadataCertification::SourceMetadataId)
                            .col(SourceMetadataCertification::Country),
                    )
                    .foreign_key(&mut owned_by(
                        SourceMetadataCertification::Table,
                        SourceMetadataCertification::SourceMetadataId,
                    ))
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(SourceMetadataImage::Table)
                    .if_not_exists()
                    .col(source_id(SourceMetadataImage::SourceMetadataId))
                    .col(
                        ColumnDef::new(SourceMetadataImage::Kind)
                            .string_len(16)
                            .not_null(),
                    )
                    .col(ColumnDef::new(SourceMetadataImage::Url).string().not_null())
                    .col(ColumnDef::new(SourceMetadataImage::Language).string_len(16))
                    .col(ColumnDef::new(SourceMetadataImage::Width).integer())
                    .col(ColumnDef::new(SourceMetadataImage::Height).integer())
                    .col(ColumnDef::new(SourceMetadataImage::VoteAverage).double())
                    .col(ColumnDef::new(SourceMetadataImage::VoteCount).integer())
                    .col(
                        ColumnDef::new(SourceMetadataImage::Position)
                            .integer()
                            .not_null(),
                    )
                    .primary_key(
                        Index::create()
                            .col(SourceMetadataImage::SourceMetadataId)
                            .col(SourceMetadataImage::Kind)
                            .col(SourceMetadataImage::Url),
                    )
                    .foreign_key(&mut owned_by(
                        SourceMetadataImage::Table,
                        SourceMetadataImage::SourceMetadataId,
                    ))
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(ItemSource::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ItemSource::ItemId)
                            .big_integer()
                            .not_null()
                            .primary_key(),
                    )
                    .col(source_id(ItemSource::SourceMetadataId))
                    .col(ColumnDef::new(ItemSource::Score).integer())
                    .col(
                        ColumnDef::new(ItemSource::MatchedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(ItemSource::Table, ItemSource::ItemId)
                            .to(MetadataItem::Table, MetadataItem::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(&mut owned_by(
                        ItemSource::Table,
                        ItemSource::SourceMetadataId,
                    ))
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_item_source_source")
                    .table(ItemSource::Table)
                    .col(ItemSource::SourceMetadataId)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for table in [
            ItemSource::Table.into_iden(),
            SourceMetadataImage::Table.into_iden(),
            SourceMetadataCertification::Table.into_iden(),
            SourceMetadataExternalId::Table.into_iden(),
            SourceMetadataTranslation::Table.into_iden(),
            SourceMetadata::Table.into_iden(),
        ] {
            manager
                .drop_table(Table::drop().table(table).to_owned())
                .await?;
        }
        Ok(())
    }
}

fn source_id(column: impl IntoIden) -> ColumnDef {
    ColumnDef::new(column).big_integer().not_null().to_owned()
}

/// A row that goes with its record.
fn owned_by(
    table: impl IntoIden + 'static,
    column: impl IntoIden + 'static,
) -> ForeignKeyCreateStatement {
    ForeignKey::create()
        .from(table, column)
        .to(SourceMetadata::Table, SourceMetadata::Id)
        .on_delete(ForeignKeyAction::Cascade)
        .to_owned()
}

#[derive(DeriveIden)]
enum SourceMetadata {
    Table,
    Id,
    Agent,
    Kind,
    ProviderId,
    ParentId,
    Position,
    OriginalLanguage,
    OriginalTitle,
    Year,
    ReleasedOn,
    DurationMs,
    Studio,
    Rating,
    RefreshedAt,
}

#[derive(DeriveIden)]
enum SourceMetadataTranslation {
    Table,
    SourceMetadataId,
    Language,
    Title,
    Summary,
    Tagline,
}

#[derive(DeriveIden)]
enum SourceMetadataExternalId {
    Table,
    SourceMetadataId,
    Source,
    Value,
}

#[derive(DeriveIden)]
enum SourceMetadataCertification {
    Table,
    SourceMetadataId,
    Country,
    Rating,
}

#[derive(DeriveIden)]
enum SourceMetadataImage {
    Table,
    SourceMetadataId,
    Kind,
    Url,
    Language,
    Width,
    Height,
    VoteAverage,
    VoteCount,
    Position,
}

#[derive(DeriveIden)]
enum ItemSource {
    Table,
    ItemId,
    SourceMetadataId,
    Score,
    MatchedAt,
}

#[derive(DeriveIden)]
enum MetadataItem {
    Table,
    Id,
}
