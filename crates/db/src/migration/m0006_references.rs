//! What external providers say about an item: its identifiers there, and their ratings.
//!
//! Plex stores both as tags (`tag_type` 314 and 316), the identifier or the badge as the tag
//! and the score in the attachment's text. They are not labels anyone filters a library by,
//! so they get tables of their own with typed columns: a source, a value, a kind.

use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::{big_integer, double, integer, string};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ItemExternalId::Table)
                    .if_not_exists()
                    .col(big_integer(ItemExternalId::ItemId))
                    // `imdb`, `tmdb`, `tvdb`: the three the reference library uses.
                    .col(string(ItemExternalId::Source))
                    .col(string(ItemExternalId::Value))
                    .col(integer(ItemExternalId::Position))
                    // Not `(item, source)`: 65 items carry two identifiers from one source.
                    .primary_key(
                        Index::create()
                            .col(ItemExternalId::ItemId)
                            .col(ItemExternalId::Source)
                            .col(ItemExternalId::Value),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(ItemExternalId::Table, ItemExternalId::ItemId)
                            .to(MetadataItem::Table, MetadataItem::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // "Which item is tt0293662" is the lookup splitting source from value exists for.
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_external_id_lookup")
                    .table(ItemExternalId::Table)
                    .col(ItemExternalId::Source)
                    .col(ItemExternalId::Value)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(ItemRating::Table)
                    .if_not_exists()
                    .col(big_integer(ItemRating::ItemId))
                    .col(string(ItemRating::Source))
                    // `critic` or `audience`.
                    .col(string(ItemRating::Kind))
                    .col(double(ItemRating::Value))
                    .col(string(ItemRating::Image))
                    .col(integer(ItemRating::Position))
                    .primary_key(
                        Index::create()
                            .col(ItemRating::ItemId)
                            .col(ItemRating::Source)
                            .col(ItemRating::Kind),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(ItemRating::Table, ItemRating::ItemId)
                            .to(MetadataItem::Table, MetadataItem::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ItemRating::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(ItemExternalId::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum ItemExternalId {
    Table,
    ItemId,
    Source,
    Value,
    Position,
}

#[derive(DeriveIden)]
enum ItemRating {
    Table,
    ItemId,
    Source,
    Kind,
    Value,
    Image,
    Position,
}

#[derive(DeriveIden)]
enum MetadataItem {
    Table,
    Id,
}
