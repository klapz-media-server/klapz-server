//! Tags: genres, countries, and the people credited on an item.
//!
//! Tag values are global and attachment is per item, ordered — the shape of Plex's
//! `tags`/`taggings`, minus what we reject there: no packed `extra_data`, no tag pointing at
//! one item, no artwork or external identifier disguised as a tag type.
//!
//! No uniqueness on `(kind, name)`. It looks natural and is wrong: two different people can
//! share a name, and the reference library has 11,879 actor names belonging to more than
//! one person, each told apart by the provider key. Identity is the id.
//!
//! `name_key` is the name with ASCII case folded, which is the form a filter by name compares
//! in. Stored rather than computed in the query because `LOWER` folds Unicode on `PostgreSQL`
//! and ASCII only on `SQLite`, and the reference behaves like the latter.

use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::{big_integer, integer, string, string_null};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Tag::Table)
                    .if_not_exists()
                    .col(big_integer(Tag::Id).primary_key())
                    .col(string(Tag::Kind))
                    .col(string(Tag::Name))
                    .col(string(Tag::NameKey))
                    .col(string_null(Tag::ProviderKey))
                    .col(string_null(Tag::ThumbUrl))
                    .to_owned(),
            )
            .await?;

        // A filter by name seeks here; a filter by id uses the primary key.
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_tag_name")
                    .table(Tag::Table)
                    .col(Tag::Kind)
                    .col(Tag::NameKey)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(ItemTag::Table)
                    .if_not_exists()
                    .col(big_integer(ItemTag::ItemId))
                    .col(big_integer(ItemTag::TagId))
                    // Billing order within the item and the tag's kind. The source credits
                    // the same person twice on ten items, at two positions, so the position
                    // belongs to the key.
                    .col(integer(ItemTag::Position))
                    .col(string_null(ItemTag::Role))
                    .primary_key(
                        Index::create()
                            .col(ItemTag::ItemId)
                            .col(ItemTag::TagId)
                            .col(ItemTag::Position),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(ItemTag::Table, ItemTag::ItemId)
                            .to(MetadataItem::Table, MetadataItem::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(ItemTag::Table, ItemTag::TagId)
                            .to(Tag::Table, Tag::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // The primary key serves "the tags of these items"; a filter goes the other way,
        // from a tag to the items carrying it.
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_item_tag_tag")
                    .table(ItemTag::Table)
                    .col(ItemTag::TagId)
                    .col(ItemTag::ItemId)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ItemTag::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Tag::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Tag {
    Table,
    Id,
    Kind,
    Name,
    NameKey,
    ProviderKey,
    ThumbUrl,
}

#[derive(DeriveIden)]
enum ItemTag {
    Table,
    ItemId,
    TagId,
    Position,
    Role,
}

#[derive(DeriveIden)]
enum MetadataItem {
    Table,
    Id,
}
