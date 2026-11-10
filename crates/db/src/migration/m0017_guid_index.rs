//! Finding an item by the work it is: every copy of a film, for watch state and Continue
//! Watching, which keep state per work.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_metadata_item_guid")
                    .table(MetadataItem::Table)
                    .col(MetadataItem::Guid)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx_metadata_item_guid")
                    .table(MetadataItem::Table)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum MetadataItem {
    Table,
    Guid,
}
