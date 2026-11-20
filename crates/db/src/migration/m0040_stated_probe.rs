//! What a file states about itself, as ffprobe printed it, beside the values the scan writes in
//! the reference's vocabulary: a track's language before any default, its time base, codec
//! tag, handler and declared rates, a picture subtitle's canvas; a file's format names,
//! declared duration and bitrate. The Jellyfin surface publishes these as its reference does.
//!
//! And what a file carries besides what plays — cover pictures, data tracks, attached files —
//! in a table of their own, so that nothing reading tracks meets them.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // SQLite adds one column per statement.
        for column in [
            ColumnDef::new(MediaStream::DeclaredLanguage)
                .string()
                .to_owned(),
            ColumnDef::new(MediaStream::HandlerName).string().to_owned(),
            ColumnDef::new(MediaStream::TimeBase).string().to_owned(),
            ColumnDef::new(MediaStream::CodecTag).string().to_owned(),
            ColumnDef::new(MediaStream::DeclaredBitrate)
                .big_integer()
                .to_owned(),
            ColumnDef::new(MediaStream::TaggedBitrate)
                .big_integer()
                .to_owned(),
            ColumnDef::new(MediaStream::AverageFrameRate)
                .string()
                .to_owned(),
            ColumnDef::new(MediaStream::RealFrameRate)
                .string()
                .to_owned(),
            ColumnDef::new(MediaStream::DisplayAspectRatio)
                .string()
                .to_owned(),
            ColumnDef::new(MediaStream::SampleAspectRatio)
                .string()
                .to_owned(),
            ColumnDef::new(MediaStream::IsCropped).boolean().to_owned(),
            ColumnDef::new(MediaStream::PixelFormat).string().to_owned(),
            ColumnDef::new(MediaStream::IsAvc).boolean().to_owned(),
            ColumnDef::new(MediaStream::NalLengthSize)
                .integer()
                .to_owned(),
            ColumnDef::new(MediaStream::CanvasWidth)
                .integer()
                .to_owned(),
            ColumnDef::new(MediaStream::CanvasHeight)
                .integer()
                .to_owned(),
        ] {
            let mut column = column;
            manager
                .alter_table(
                    Table::alter()
                        .table(MediaStream::Table)
                        .add_column(&mut column)
                        .to_owned(),
                )
                .await?;
        }
        for column in [
            ColumnDef::new(MediaPart::FormatName).string().to_owned(),
            ColumnDef::new(MediaPart::DeclaredDurationUs)
                .big_integer()
                .to_owned(),
            ColumnDef::new(MediaPart::DeclaredBitrate)
                .big_integer()
                .to_owned(),
        ] {
            let mut column = column;
            manager
                .alter_table(
                    Table::alter()
                        .table(MediaPart::Table)
                        .add_column(&mut column)
                        .to_owned(),
                )
                .await?;
        }
        Box::pin(create_side_streams(manager)).await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(PartSideStream::Table).to_owned())
            .await?;
        for column in [
            MediaPart::FormatName,
            MediaPart::DeclaredDurationUs,
            MediaPart::DeclaredBitrate,
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(MediaPart::Table)
                        .drop_column(column)
                        .to_owned(),
                )
                .await?;
        }
        for column in [
            MediaStream::DeclaredLanguage,
            MediaStream::HandlerName,
            MediaStream::TimeBase,
            MediaStream::CodecTag,
            MediaStream::DeclaredBitrate,
            MediaStream::TaggedBitrate,
            MediaStream::AverageFrameRate,
            MediaStream::RealFrameRate,
            MediaStream::DisplayAspectRatio,
            MediaStream::SampleAspectRatio,
            MediaStream::IsCropped,
            MediaStream::PixelFormat,
            MediaStream::IsAvc,
            MediaStream::NalLengthSize,
            MediaStream::CanvasWidth,
            MediaStream::CanvasHeight,
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(MediaStream::Table)
                        .drop_column(column)
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}

/// The table of covers, data tracks and attached files.
async fn create_side_streams(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(
            Table::create()
                .table(PartSideStream::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(PartSideStream::Id)
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(
                    ColumnDef::new(PartSideStream::MediaPartId)
                        .big_integer()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(PartSideStream::Position)
                        .integer()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(PartSideStream::Kind)
                        .string_len(10)
                        .not_null(),
                )
                .col(ColumnDef::new(PartSideStream::Codec).string())
                .col(ColumnDef::new(PartSideStream::CodecTag).string())
                .col(ColumnDef::new(PartSideStream::Profile).string())
                .col(ColumnDef::new(PartSideStream::Language).string())
                .col(ColumnDef::new(PartSideStream::TimeBase).string())
                .col(ColumnDef::new(PartSideStream::MimeType).string())
                .col(ColumnDef::new(PartSideStream::Filename).text())
                .col(ColumnDef::new(PartSideStream::Width).integer())
                .col(ColumnDef::new(PartSideStream::Height).integer())
                .col(ColumnDef::new(PartSideStream::PixelFormat).string())
                .col(ColumnDef::new(PartSideStream::BitDepth).integer())
                .col(ColumnDef::new(PartSideStream::ColorSpace).string())
                .col(ColumnDef::new(PartSideStream::Level).integer())
                .col(ColumnDef::new(PartSideStream::RealFrameRate).string())
                .col(ColumnDef::new(PartSideStream::AverageFrameRate).string())
                .col(ColumnDef::new(PartSideStream::SampleAspectRatio).string())
                .col(ColumnDef::new(PartSideStream::DisplayAspectRatio).string())
                .col(
                    ColumnDef::new(PartSideStream::IsInterlaced)
                        .boolean()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(PartSideStream::IsDefault)
                        .boolean()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(PartSideStream::IsForced)
                        .boolean()
                        .not_null(),
                )
                .foreign_key(
                    ForeignKey::create()
                        .from(PartSideStream::Table, PartSideStream::MediaPartId)
                        .to(MediaPart::Table, MediaPart::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .to_owned(),
        )
        .await?;
    manager
        .create_index(
            Index::create()
                .if_not_exists()
                .name("idx_part_side_stream_part")
                .table(PartSideStream::Table)
                .col(PartSideStream::MediaPartId)
                .col(PartSideStream::Position)
                .to_owned(),
        )
        .await
}

#[derive(DeriveIden)]
enum MediaStream {
    Table,
    DeclaredLanguage,
    HandlerName,
    TimeBase,
    CodecTag,
    DeclaredBitrate,
    TaggedBitrate,
    AverageFrameRate,
    RealFrameRate,
    DisplayAspectRatio,
    SampleAspectRatio,
    IsCropped,
    PixelFormat,
    IsAvc,
    NalLengthSize,
    CanvasWidth,
    CanvasHeight,
}

#[derive(DeriveIden)]
enum MediaPart {
    Table,
    Id,
    FormatName,
    DeclaredDurationUs,
    DeclaredBitrate,
}

#[derive(DeriveIden)]
enum PartSideStream {
    Table,
    Id,
    MediaPartId,
    Position,
    Kind,
    Codec,
    CodecTag,
    Profile,
    Language,
    TimeBase,
    MimeType,
    Filename,
    Width,
    Height,
    PixelFormat,
    BitDepth,
    ColorSpace,
    Level,
    RealFrameRate,
    AverageFrameRate,
    SampleAspectRatio,
    DisplayAspectRatio,
    IsInterlaced,
    IsDefault,
    IsForced,
}
