//! Extras: the trailers, deleted scenes and featurettes that come with an item.
//!
//! The reference makes each one a metadata item of its own, outside any library, tied to its
//! item by `metadata_relations` and typed in its packed `extra_data` (`ex:extraType=1`). Here an
//! extra is a row of its own with its item as a foreign key, so that it is never listed with a
//! library's items, and goes with its item.
//!
//! Its versions hold their one file in the same row: one part per version on every extra of
//! the reference library, 85,969 of 85,969. A file is either on disk (`path`) or streamed by a
//! provider (`stream_url`), which is what the reference's packed `at:key` holds. Tracks keep
//! what the source knows of them, which is little: codec, language, channels, bitrate, size.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        Box::pin(create_extra(manager)).await?;
        Box::pin(create_extra_media(manager)).await?;
        Box::pin(create_extra_track(manager)).await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ExtraTrack::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(ExtraMedia::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Extra::Table).to_owned())
            .await
    }
}

async fn create_extra(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(
            Table::create()
                .table(Extra::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(Extra::Id)
                        .big_integer()
                        .not_null()
                        .primary_key(),
                )
                .col(ColumnDef::new(Extra::ItemId).big_integer().not_null())
                .col(ColumnDef::new(Extra::Kind).string_len(24).not_null())
                .col(ColumnDef::new(Extra::Position).integer())
                .col(ColumnDef::new(Extra::Guid).text().not_null())
                .col(ColumnDef::new(Extra::Title).text())
                .col(ColumnDef::new(Extra::TitleSort).text())
                .col(ColumnDef::new(Extra::Summary).text())
                .col(ColumnDef::new(Extra::Year).integer())
                .col(ColumnDef::new(Extra::ReleasedOn).big_integer())
                .col(ColumnDef::new(Extra::DurationMs).big_integer())
                .col(ColumnDef::new(Extra::AddedAt).big_integer())
                .col(ColumnDef::new(Extra::UpdatedAt).big_integer())
                .foreign_key(
                    ForeignKey::create()
                        .from(Extra::Table, Extra::ItemId)
                        .to(MetadataItem::Table, MetadataItem::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .to_owned(),
        )
        .await?;
    manager
        .create_index(
            Index::create()
                .if_not_exists()
                .name("idx_extra_item")
                .table(Extra::Table)
                .col(Extra::ItemId)
                .col(Extra::Position)
                .col(Extra::Id)
                .to_owned(),
        )
        .await
}

async fn create_extra_media(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(
            Table::create()
                .table(ExtraMedia::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(ExtraMedia::Id)
                        .big_integer()
                        .not_null()
                        .primary_key(),
                )
                .col(ColumnDef::new(ExtraMedia::ExtraId).big_integer().not_null())
                .col(
                    ColumnDef::new(ExtraMedia::PartId)
                        .big_integer()
                        .not_null()
                        .unique_key(),
                )
                .col(ColumnDef::new(ExtraMedia::Width).integer())
                .col(ColumnDef::new(ExtraMedia::Height).integer())
                .col(ColumnDef::new(ExtraMedia::AspectRatio).double())
                .col(ColumnDef::new(ExtraMedia::DurationMs).big_integer())
                .col(ColumnDef::new(ExtraMedia::Bitrate).integer())
                .col(ColumnDef::new(ExtraMedia::Container).string_len(32))
                .col(ColumnDef::new(ExtraMedia::VideoCodec).string_len(32))
                .col(ColumnDef::new(ExtraMedia::AudioCodec).string_len(32))
                .col(ColumnDef::new(ExtraMedia::AudioChannels).integer())
                .col(
                    ColumnDef::new(ExtraMedia::OptimizedForStreaming)
                        .boolean()
                        .not_null(),
                )
                .col(ColumnDef::new(ExtraMedia::Path).text())
                .col(ColumnDef::new(ExtraMedia::SizeBytes).big_integer())
                .col(ColumnDef::new(ExtraMedia::StreamUrl).text())
                .col(ColumnDef::new(ExtraMedia::FileOptimizedForStreaming).boolean())
                .foreign_key(
                    ForeignKey::create()
                        .from(ExtraMedia::Table, ExtraMedia::ExtraId)
                        .to(Extra::Table, Extra::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .to_owned(),
        )
        .await?;
    manager
        .create_index(
            Index::create()
                .if_not_exists()
                .name("idx_extra_media_extra")
                .table(ExtraMedia::Table)
                .col(ExtraMedia::ExtraId)
                .to_owned(),
        )
        .await
}

async fn create_extra_track(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(
            Table::create()
                .table(ExtraTrack::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(ExtraTrack::Id)
                        .big_integer()
                        .not_null()
                        .primary_key(),
                )
                .col(
                    ColumnDef::new(ExtraTrack::ExtraMediaId)
                        .big_integer()
                        .not_null(),
                )
                .col(ColumnDef::new(ExtraTrack::Kind).string_len(8).not_null())
                .col(ColumnDef::new(ExtraTrack::Position).integer().not_null())
                .col(ColumnDef::new(ExtraTrack::Codec).string_len(32))
                .col(ColumnDef::new(ExtraTrack::Language).string_len(16))
                .col(ColumnDef::new(ExtraTrack::Channels).integer())
                .col(ColumnDef::new(ExtraTrack::Bitrate).integer())
                .col(ColumnDef::new(ExtraTrack::Width).integer())
                .col(ColumnDef::new(ExtraTrack::Height).integer())
                .col(ColumnDef::new(ExtraTrack::IsDefault).boolean().not_null())
                .foreign_key(
                    ForeignKey::create()
                        .from(ExtraTrack::Table, ExtraTrack::ExtraMediaId)
                        .to(ExtraMedia::Table, ExtraMedia::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .to_owned(),
        )
        .await?;
    manager
        .create_index(
            Index::create()
                .if_not_exists()
                .name("idx_extra_track_media")
                .table(ExtraTrack::Table)
                .col(ExtraTrack::ExtraMediaId)
                .col(ExtraTrack::Position)
                .to_owned(),
        )
        .await
}

#[derive(DeriveIden)]
enum MetadataItem {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum Extra {
    Table,
    Id,
    ItemId,
    Kind,
    Position,
    Guid,
    Title,
    TitleSort,
    Summary,
    Year,
    ReleasedOn,
    DurationMs,
    AddedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum ExtraMedia {
    Table,
    Id,
    ExtraId,
    PartId,
    Width,
    Height,
    AspectRatio,
    DurationMs,
    Bitrate,
    Container,
    VideoCodec,
    AudioCodec,
    AudioChannels,
    OptimizedForStreaming,
    Path,
    SizeBytes,
    StreamUrl,
    FileOptimizedForStreaming,
}

#[derive(DeriveIden)]
enum ExtraTrack {
    Table,
    Id,
    ExtraMediaId,
    Kind,
    Position,
    Codec,
    Language,
    Channels,
    Bitrate,
    Width,
    Height,
    IsDefault,
}
