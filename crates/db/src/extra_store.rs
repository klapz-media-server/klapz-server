//! Extras in storage: the `extra`, `extra_media` and `extra_track` tables, read back as an
//! extra and as the versions, files and tracks the items' media are read as.
//!
//! Its own module because the repository and the importer both need the kind's encoding, and
//! because the repository's media lookups fall back to it for what the items' tables lack.

use klapz_core::extra::{Extra, ExtraKind};
use klapz_core::language::LanguageTag;
use klapz_core::metadata::{
    AudioAnalysis, FileAnalysis, FileLayout, StatedFile, StatedTrack, StreamKind, TrackTraits,
    VideoAnalysis,
};
use klapz_core::{
    ExtraId, MediaFile, MediaId, MediaStream, MediaVersion, PartId, RatingKey, StreamId,
};
use sea_orm::sea_query::NullOrdering;
use sea_orm::{ColumnTrait, ConnectionTrait, DbErr, EntityTrait, Order, QueryFilter, QueryOrder};

use crate::entity::{extra, extra_media, extra_track};

/// How a kind is stored.
pub(crate) const fn kind_name(kind: ExtraKind) -> &'static str {
    match kind {
        ExtraKind::Trailer => "trailer",
        ExtraKind::DeletedScene => "deleted_scene",
        ExtraKind::Interview => "interview",
        ExtraKind::MusicVideo => "music_video",
        ExtraKind::BehindTheScenes => "behind_the_scenes",
        ExtraKind::Scene => "scene",
        ExtraKind::LiveMusicVideo => "live_music_video",
        ExtraKind::LyricMusicVideo => "lyric_music_video",
        ExtraKind::Concert => "concert",
        ExtraKind::Featurette => "featurette",
        ExtraKind::Short => "short",
        ExtraKind::Other => "other",
    }
}

fn parse_kind(name: &str) -> Option<ExtraKind> {
    ExtraKind::ALL
        .into_iter()
        .find(|kind| kind_name(*kind) == name)
}

fn to_extra(row: extra::Model) -> Option<Extra> {
    let Some(kind) = parse_kind(&row.kind) else {
        tracing::warn!(
            kind = row.kind,
            id = row.id,
            "unknown extra kind in storage"
        );
        return None;
    };
    Some(Extra {
        id: ExtraId::new(row.id),
        item: RatingKey::new(row.item_id),
        kind,
        position: row.position,
        guid: row.guid,
        title: row.title,
        title_sort: row.title_sort,
        summary: row.summary,
        year: row.year,
        released_on: row.released_on,
        duration_ms: row.duration_ms,
        added_at: row.added_at,
        updated_at: row.updated_at,
    })
}

pub(crate) async fn extras<C: ConnectionTrait>(
    connection: &C,
    items: &[RatingKey],
) -> Result<Vec<Extra>, DbErr> {
    if items.is_empty() {
        return Ok(Vec::new());
    }
    let rows = extra::Entity::find()
        .filter(extra::Column::ItemId.is_in(items.iter().map(|item| item.get())))
        .order_by_asc(extra::Column::ItemId)
        // Spelled out: engines disagree on where an ascending order puts nulls.
        .order_by_with_nulls(extra::Column::Position, Order::Asc, NullOrdering::First)
        .order_by_asc(extra::Column::Id)
        .all(connection)
        .await?;
    Ok(rows.into_iter().filter_map(to_extra).collect())
}

pub(crate) async fn extra<C: ConnectionTrait>(
    connection: &C,
    id: ExtraId,
) -> Result<Option<Extra>, DbErr> {
    Ok(extra::Entity::find_by_id(id.get())
        .one(connection)
        .await?
        .and_then(to_extra))
}

/// A version names the item its extra comes with, which is whose readers may see it.
fn to_version(row: &extra_media::Model, item: RatingKey) -> MediaVersion {
    MediaVersion {
        id: MediaId::new(row.id),
        item_id: item,
        width: row.width,
        height: row.height,
        aspect_ratio: row.aspect_ratio,
        duration_ms: row.duration_ms,
        bitrate: row.bitrate,
        container: row.container.clone(),
        video_codec: row.video_codec.clone(),
        audio_codec: row.audio_codec.clone(),
        audio_channels: row.audio_channels,
        frame_rate: None,
        interlaced: false,
        optimized_for_streaming: row.optimized_for_streaming,
    }
}

fn to_file(row: extra_media::Model) -> MediaFile {
    MediaFile {
        id: PartId::new(row.part_id),
        version_id: MediaId::new(row.id),
        position: 0,
        path: row.path.unwrap_or_default(),
        size_bytes: row.size_bytes,
        duration_ms: row.duration_ms,
        layout: FileLayout {
            optimized_for_streaming: row.file_optimized_for_streaming,
            ..FileLayout::default()
        },
        stream_url: row.stream_url,
        content_hash: None,
        open_subtitles_hash: None,
        stated: StatedFile::default(),
        analysis: FileAnalysis::Complete,
    }
}

fn to_track(row: extra_track::Model, file: PartId) -> MediaStream {
    let kind = crate::metadata_repository::parse_stream_kind(&row.kind);
    MediaStream {
        id: StreamId::new(row.id),
        file_id: file,
        kind,
        position: row.position,
        codec: row.codec,
        language: row.language.as_deref().and_then(LanguageTag::parse),
        channels: row.channels,
        bitrate: row.bitrate,
        is_default: row.is_default,
        is_forced: false,
        external_path: None,
        title: None,
        stream_identifier: None,
        profile: None,
        bit_depth: None,
        traits: TrackTraits::default(),
        video: (kind == StreamKind::Video).then(|| {
            Box::new(VideoAnalysis {
                width: row.width,
                height: row.height,
                ..VideoAnalysis::default()
            })
        }),
        audio: (kind == StreamKind::Audio).then(AudioAnalysis::default),
        subtitle: None,
        required_bandwidths: Vec::new(),
        stated: StatedTrack::default(),
    }
}

/// The items the versions' extras come with, by extra.
async fn items_of<C: ConnectionTrait>(
    connection: &C,
    rows: &[extra_media::Model],
) -> Result<std::collections::HashMap<i64, RatingKey>, DbErr> {
    let ids: Vec<i64> = rows.iter().map(|row| row.extra_id).collect();
    Ok(extra::Entity::find()
        .filter(extra::Column::Id.is_in(ids))
        .all(connection)
        .await?
        .into_iter()
        .map(|row| (row.id, RatingKey::new(row.item_id)))
        .collect())
}

async fn with_items<C: ConnectionTrait>(
    connection: &C,
    rows: Vec<extra_media::Model>,
) -> Result<Vec<(ExtraId, MediaVersion)>, DbErr> {
    let items = items_of(connection, &rows).await?;
    Ok(rows
        .iter()
        .filter_map(|row| {
            let item = items.get(&row.extra_id)?;
            Some((ExtraId::new(row.extra_id), to_version(row, *item)))
        })
        .collect())
}

pub(crate) async fn versions<C: ConnectionTrait>(
    connection: &C,
    extras: &[ExtraId],
) -> Result<Vec<(ExtraId, MediaVersion)>, DbErr> {
    if extras.is_empty() {
        return Ok(Vec::new());
    }
    let rows = extra_media::Entity::find()
        .filter(extra_media::Column::ExtraId.is_in(extras.iter().map(|extra| extra.get())))
        .order_by_asc(extra_media::Column::ExtraId)
        .order_by_asc(extra_media::Column::Id)
        .all(connection)
        .await?;
    with_items(connection, rows).await
}

pub(crate) async fn version<C: ConnectionTrait>(
    connection: &C,
    id: MediaId,
) -> Result<Option<MediaVersion>, DbErr> {
    let Some(row) = extra_media::Entity::find_by_id(id.get())
        .one(connection)
        .await?
    else {
        return Ok(None);
    };
    Ok(with_items(connection, vec![row])
        .await?
        .into_iter()
        .next()
        .map(|(_, version)| version))
}

pub(crate) async fn files<C: ConnectionTrait>(
    connection: &C,
    versions: &[MediaId],
) -> Result<Vec<MediaFile>, DbErr> {
    if versions.is_empty() {
        return Ok(Vec::new());
    }
    Ok(extra_media::Entity::find()
        .filter(extra_media::Column::Id.is_in(versions.iter().map(|version| version.get())))
        .order_by_asc(extra_media::Column::Id)
        .all(connection)
        .await?
        .into_iter()
        .map(to_file)
        .collect())
}

pub(crate) async fn file<C: ConnectionTrait>(
    connection: &C,
    id: PartId,
) -> Result<Option<MediaFile>, DbErr> {
    Ok(extra_media::Entity::find()
        .filter(extra_media::Column::PartId.eq(id.get()))
        .one(connection)
        .await?
        .map(to_file))
}

pub(crate) async fn streams<C: ConnectionTrait>(
    connection: &C,
    files: &[PartId],
) -> Result<Vec<MediaStream>, DbErr> {
    if files.is_empty() {
        return Ok(Vec::new());
    }
    let parts: std::collections::HashMap<i64, PartId> = extra_media::Entity::find()
        .filter(extra_media::Column::PartId.is_in(files.iter().map(|file| file.get())))
        .all(connection)
        .await?
        .into_iter()
        .map(|row| (row.id, PartId::new(row.part_id)))
        .collect();
    if parts.is_empty() {
        return Ok(Vec::new());
    }
    let rows = extra_track::Entity::find()
        .filter(extra_track::Column::ExtraMediaId.is_in(parts.keys().copied()))
        .order_by_asc(extra_track::Column::ExtraMediaId)
        .order_by_asc(extra_track::Column::Position)
        .all(connection)
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let file = *parts.get(&row.extra_media_id)?;
            Some(to_track(row, file))
        })
        .collect())
}

pub(crate) async fn stream<C: ConnectionTrait>(
    connection: &C,
    id: StreamId,
) -> Result<Option<MediaStream>, DbErr> {
    let Some(row) = extra_track::Entity::find_by_id(id.get())
        .one(connection)
        .await?
    else {
        return Ok(None);
    };
    let Some(media) = extra_media::Entity::find_by_id(row.extra_media_id)
        .one(connection)
        .await?
    else {
        return Ok(None);
    };
    Ok(Some(to_track(row, PartId::new(media.part_id))))
}
