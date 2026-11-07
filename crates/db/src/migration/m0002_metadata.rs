use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::{
    big_integer, big_integer_null, boolean, double_null, integer, integer_null, string,
    string_null, text_null,
};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // One function per table: a single `up` covering four tables and six indexes is past
        // the point where a reader can hold it in their head.
        create_metadata_item(manager).await?;
        create_media(manager).await?;
        create_media_part(manager).await?;
        create_media_stream(manager).await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(MediaStream::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(MediaPart::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Media::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(MetadataItem::Table).to_owned())
            .await
    }
}

async fn create_metadata_item(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(
            Table::create()
                .table(MetadataItem::Table)
                .if_not_exists()
                // Ids come from the source library and are part of the API surface, so
                // they are preserved rather than generated.
                .col(big_integer(MetadataItem::Id).primary_key())
                .col(big_integer(MetadataItem::LibrarySectionId))
                // Null for a movie or a show; the season for an episode.
                .col(big_integer_null(MetadataItem::ParentId))
                .col(string(MetadataItem::Kind))
                .col(string(MetadataItem::Guid))
                .col(string_null(MetadataItem::Title))
                // The raw sort title, which the API exposes as nullable.
                .col(string_null(MetadataItem::TitleSort))
                // What the listing orders by and what search matches against: the sort
                // title lowercased, materialized because neither `COALESCE` nor `LOWER` is
                // indexable portably. One column serves both so a single index can, which
                // measurement forced: filtering on a folded column while ordering by a raw
                // one made SQLite scan 10,773 rows instead of seeking.
                .col(string(MetadataItem::SortKey))
                .col(string_null(MetadataItem::OriginalTitle))
                .col(text_null(MetadataItem::Summary))
                .col(string_null(MetadataItem::Tagline))
                .col(string_null(MetadataItem::Studio))
                .col(string_null(MetadataItem::ContentRating))
                .col(integer_null(MetadataItem::ContentRatingAge))
                // Episode or season number within its parent.
                .col(integer_null(MetadataItem::Position))
                .col(integer_null(MetadataItem::Year))
                .col(string_null(MetadataItem::ReleasedOn))
                .col(big_integer_null(MetadataItem::DurationMs))
                .col(double_null(MetadataItem::Rating))
                .col(double_null(MetadataItem::AudienceRating))
                .col(big_integer(MetadataItem::AddedAt))
                .col(big_integer(MetadataItem::UpdatedAt))
                .foreign_key(
                    ForeignKey::create()
                        .name("fk_item_section")
                        .from(MetadataItem::Table, MetadataItem::LibrarySectionId)
                        .to(LibrarySection::Table, LibrarySection::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .foreign_key(
                    ForeignKey::create()
                        .name("fk_item_parent")
                        .from(MetadataItem::Table, MetadataItem::ParentId)
                        .to(MetadataItem::Table, MetadataItem::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .to_owned(),
        )
        .await?;

    // The listing endpoint: one library, one kind, ordered by sort title, paginated.
    // The trailing id is the cursor's tiebreaker, so the whole window is one index scan.
    manager
        .create_index(
            Index::create()
                .if_not_exists()
                .name("idx_item_listing")
                .table(MetadataItem::Table)
                .col(MetadataItem::LibrarySectionId)
                .col(MetadataItem::Kind)
                .col(MetadataItem::SortKey)
                .col(MetadataItem::Id)
                .to_owned(),
        )
        .await?;

    // Children of a show or a season, in order.
    manager
        .create_index(
            Index::create()
                .if_not_exists()
                .name("idx_item_parent")
                .table(MetadataItem::Table)
                .col(MetadataItem::ParentId)
                .col(MetadataItem::Position)
                .to_owned(),
        )
        .await?;

    // Watch state keys on the guid, and it is how the same work is found across
    // libraries — 78% of movies here have a twin.
    manager
        .create_index(
            Index::create()
                .if_not_exists()
                .name("idx_item_guid")
                .table(MetadataItem::Table)
                .col(MetadataItem::Guid)
                .to_owned(),
        )
        .await
}

async fn create_media(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(
            Table::create()
                .table(Media::Table)
                .if_not_exists()
                .col(big_integer(Media::Id).primary_key())
                .col(big_integer(Media::MetadataItemId))
                .col(integer_null(Media::Width))
                .col(integer_null(Media::Height))
                .col(double_null(Media::AspectRatio))
                .col(big_integer_null(Media::DurationMs))
                .col(integer_null(Media::Bitrate))
                .col(string_null(Media::Container))
                .col(string_null(Media::VideoCodec))
                .col(string_null(Media::AudioCodec))
                .col(integer_null(Media::AudioChannels))
                // The measured rate, e.g. 23.976. The Plex surface turns it into a
                // label like "24p"; that mapping is a wire concern, not a stored one.
                .col(double_null(Media::FrameRate))
                .col(boolean(Media::Interlaced))
                .col(boolean(Media::OptimizedForStreaming))
                .col(big_integer(Media::CreatedAt))
                .col(big_integer(Media::UpdatedAt))
                .foreign_key(
                    ForeignKey::create()
                        .name("fk_media_item")
                        .from(Media::Table, Media::MetadataItemId)
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
                .name("idx_media_item")
                .table(Media::Table)
                .col(Media::MetadataItemId)
                .to_owned(),
        )
        .await
}

async fn create_media_part(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(
            Table::create()
                .table(MediaPart::Table)
                .if_not_exists()
                .col(big_integer(MediaPart::Id).primary_key())
                .col(big_integer(MediaPart::MediaId))
                // Disc order within a stacked release; 0 for the common single file.
                .col(integer(MediaPart::Position))
                // The one column in the schema that becomes a filesystem access, and
                // therefore the one the path-traversal rules apply to.
                .col(string(MediaPart::Path))
                .col(big_integer_null(MediaPart::SizeBytes))
                .col(big_integer_null(MediaPart::DurationMs))
                .col(big_integer(MediaPart::CreatedAt))
                .col(big_integer(MediaPart::UpdatedAt))
                .foreign_key(
                    ForeignKey::create()
                        .name("fk_part_media")
                        .from(MediaPart::Table, MediaPart::MediaId)
                        .to(Media::Table, Media::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .to_owned(),
        )
        .await?;

    manager
        .create_index(
            Index::create()
                .if_not_exists()
                .name("idx_part_media")
                .table(MediaPart::Table)
                .col(MediaPart::MediaId)
                .col(MediaPart::Position)
                .to_owned(),
        )
        .await
}

async fn create_media_stream(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(
            Table::create()
                .table(MediaStream::Table)
                .if_not_exists()
                .col(big_integer(MediaStream::Id).primary_key())
                .col(big_integer(MediaStream::MediaPartId))
                .col(string(MediaStream::Kind))
                // The index the transcoder addresses the track by, so it keeps its
                // ffmpeg meaning.
                .col(integer(MediaStream::Position))
                .col(string_null(MediaStream::Codec))
                .col(string_null(MediaStream::Language))
                .col(integer_null(MediaStream::Channels))
                .col(integer_null(MediaStream::Bitrate))
                .col(boolean(MediaStream::IsDefault))
                .col(boolean(MediaStream::IsForced))
                // Set for a sidecar subtitle file, null when the track is embedded.
                .col(string_null(MediaStream::ExternalPath))
                .foreign_key(
                    ForeignKey::create()
                        .name("fk_stream_part")
                        .from(MediaStream::Table, MediaStream::MediaPartId)
                        .to(MediaPart::Table, MediaPart::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .to_owned(),
        )
        .await?;

    // Nearly a million rows, always read per file and per kind.
    manager
        .create_index(
            Index::create()
                .if_not_exists()
                .name("idx_stream_part")
                .table(MediaStream::Table)
                .col(MediaStream::MediaPartId)
                .col(MediaStream::Kind)
                .col(MediaStream::Position)
                .to_owned(),
        )
        .await
}

/// Declared again here rather than imported from the first migration: a migration describes
/// the schema at its own point in time, and reaching into another one couples the two.
#[derive(DeriveIden)]
enum LibrarySection {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum MetadataItem {
    Table,
    Id,
    LibrarySectionId,
    ParentId,
    Kind,
    Guid,
    Title,
    TitleSort,
    SortKey,
    OriginalTitle,
    Summary,
    Tagline,
    Studio,
    ContentRating,
    ContentRatingAge,
    Position,
    Year,
    ReleasedOn,
    DurationMs,
    Rating,
    AudienceRating,
    AddedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum Media {
    Table,
    Id,
    MetadataItemId,
    Width,
    Height,
    AspectRatio,
    DurationMs,
    Bitrate,
    Container,
    VideoCodec,
    AudioCodec,
    AudioChannels,
    FrameRate,
    Interlaced,
    OptimizedForStreaming,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum MediaPart {
    Table,
    Id,
    MediaId,
    Position,
    Path,
    SizeBytes,
    DurationMs,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum MediaStream {
    Table,
    Id,
    MediaPartId,
    Kind,
    Position,
    Codec,
    Language,
    Channels,
    Bitrate,
    IsDefault,
    IsForced,
    ExternalPath,
}
