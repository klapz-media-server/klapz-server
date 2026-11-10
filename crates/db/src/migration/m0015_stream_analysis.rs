//! What the media analysis measured about each track and file.
//!
//! Plex keeps these in a packed key/value column (`extra_data`); here each is a column of its
//! own, typed, and Dolby Vision and loudness — present on a few thousand and a few dozen
//! tracks — get tables of their own rather than eight and six mostly-null columns.

use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::{
    big_integer, big_integer_null, boolean, boolean_null, double, double_null, integer,
    integer_null, string, string_null,
};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for mut column in stream_columns() {
            add_column(manager, MediaStream::Table, &mut column).await?;
        }
        for mut column in part_columns() {
            add_column(manager, MediaPart::Table, &mut column).await?;
        }
        create_dolby_vision(manager).await?;
        create_loudness(manager).await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(StreamLoudness::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(StreamDolbyVision::Table).to_owned())
            .await?;
        for column in part_columns() {
            drop_column(manager, MediaPart::Table, &column).await?;
        }
        for column in stream_columns() {
            drop_column(manager, MediaStream::Table, &column).await?;
        }
        Ok(())
    }
}

/// One column per statement: `SQLite` cannot add several in one `ALTER TABLE`.
async fn add_column<T: Iden + 'static>(
    manager: &SchemaManager<'_>,
    table: T,
    column: &mut ColumnDef,
) -> Result<(), DbErr> {
    manager
        .alter_table(Table::alter().table(table).add_column(column).to_owned())
        .await
}

async fn drop_column<T: Iden + 'static>(
    manager: &SchemaManager<'_>,
    table: T,
    column: &ColumnDef,
) -> Result<(), DbErr> {
    manager
        .alter_table(
            Table::alter()
                .table(table)
                .drop_column(Alias::new(column.get_column_name()))
                .to_owned(),
        )
        .await
}

fn flag(column: impl IntoIden) -> ColumnDef {
    boolean(column).default(false).to_owned()
}

fn stream_columns() -> Vec<ColumnDef> {
    vec![
        string_null(MediaStream::Title),
        big_integer_null(MediaStream::StreamIdentifier),
        string_null(MediaStream::Profile),
        integer_null(MediaStream::BitDepth),
        integer_null(MediaStream::Width),
        integer_null(MediaStream::Height),
        integer_null(MediaStream::CodedWidth),
        integer_null(MediaStream::CodedHeight),
        double_null(MediaStream::FrameRate),
        integer_null(MediaStream::Level),
        integer_null(MediaStream::RefFrames),
        string_null(MediaStream::ScanType),
        string_null(MediaStream::ChromaSubsampling),
        string_null(MediaStream::ChromaLocation),
        string_null(MediaStream::ColorRange),
        string_null(MediaStream::ColorSpace),
        string_null(MediaStream::ColorPrimaries),
        string_null(MediaStream::ColorTransfer),
        boolean_null(MediaStream::HasScalingMatrix),
        boolean_null(MediaStream::IsAnamorphic),
        string_null(MediaStream::PixelAspectRatio),
        string_null(MediaStream::Fourcc),
        flag(MediaStream::HasClosedCaptions),
        flag(MediaStream::HasSeparateFields),
        flag(MediaStream::HasHdr10Plus),
        integer_null(MediaStream::SamplingRate),
        string_null(MediaStream::ChannelLayout),
        string_null(MediaStream::SubtitleFormat),
        flag(MediaStream::IsEmbeddedInVideo),
        flag(MediaStream::IsOriginal),
        flag(MediaStream::IsDub),
        flag(MediaStream::IsCommentary),
        flag(MediaStream::IsHearingImpaired),
        flag(MediaStream::IsVisualImpaired),
        flag(MediaStream::HasHeaderCompression),
    ]
}

fn part_columns() -> Vec<ColumnDef> {
    vec![
        flag(MediaPart::HasThumbnail),
        boolean_null(MediaPart::Has64bitOffsets),
        boolean_null(MediaPart::OptimizedForStreaming),
        integer_null(MediaPart::PacketLength),
        flag(MediaPart::HasChapterTextStream),
    ]
}

async fn create_dolby_vision(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(
            Table::create()
                .table(StreamDolbyVision::Table)
                .if_not_exists()
                .col(big_integer(StreamDolbyVision::StreamId).primary_key())
                .col(string(StreamDolbyVision::Version))
                .col(integer(StreamDolbyVision::Profile))
                .col(integer(StreamDolbyVision::Level))
                .col(integer_null(StreamDolbyVision::BaseLayerCompatibility))
                .col(boolean(StreamDolbyVision::HasBaseLayer))
                .col(boolean(StreamDolbyVision::HasEnhancementLayer))
                .col(boolean(StreamDolbyVision::HasRpu))
                .foreign_key(
                    ForeignKey::create()
                        .from(StreamDolbyVision::Table, StreamDolbyVision::StreamId)
                        .to(MediaStream::Table, MediaStream::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .to_owned(),
        )
        .await
}

async fn create_loudness(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(
            Table::create()
                .table(StreamLoudness::Table)
                .if_not_exists()
                .col(big_integer(StreamLoudness::StreamId).primary_key())
                .col(string(StreamLoudness::AnalysisVersion))
                .col(double(StreamLoudness::Integrated))
                .col(double(StreamLoudness::LoudnessRange))
                .col(double(StreamLoudness::TruePeak))
                .col(double(StreamLoudness::Threshold))
                .col(double(StreamLoudness::GainOffset))
                .foreign_key(
                    ForeignKey::create()
                        .from(StreamLoudness::Table, StreamLoudness::StreamId)
                        .to(MediaStream::Table, MediaStream::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .to_owned(),
        )
        .await
}

#[derive(DeriveIden)]
enum MediaStream {
    Table,
    Id,
    Title,
    StreamIdentifier,
    Profile,
    BitDepth,
    Width,
    Height,
    CodedWidth,
    CodedHeight,
    FrameRate,
    Level,
    RefFrames,
    ScanType,
    ChromaSubsampling,
    ChromaLocation,
    ColorRange,
    ColorSpace,
    ColorPrimaries,
    ColorTransfer,
    HasScalingMatrix,
    IsAnamorphic,
    PixelAspectRatio,
    Fourcc,
    HasClosedCaptions,
    HasSeparateFields,
    HasHdr10Plus,
    SamplingRate,
    ChannelLayout,
    SubtitleFormat,
    IsEmbeddedInVideo,
    IsOriginal,
    IsDub,
    IsCommentary,
    IsHearingImpaired,
    IsVisualImpaired,
    HasHeaderCompression,
}

#[derive(DeriveIden)]
enum MediaPart {
    Table,
    HasThumbnail,
    #[sea_orm(iden = "has_64bit_offsets")]
    Has64bitOffsets,
    OptimizedForStreaming,
    PacketLength,
    HasChapterTextStream,
}

#[derive(DeriveIden)]
enum StreamDolbyVision {
    Table,
    StreamId,
    Version,
    Profile,
    Level,
    BaseLayerCompatibility,
    HasBaseLayer,
    HasEnhancementLayer,
    HasRpu,
}

#[derive(DeriveIden)]
enum StreamLoudness {
    Table,
    StreamId,
    AnalysisVersion,
    Integrated,
    LoudnessRange,
    TruePeak,
    Threshold,
    GainOffset,
}
