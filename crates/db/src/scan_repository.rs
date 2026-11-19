//! What a library scan writes: new items with their files and tracks, files that changed, and
//! files that are gone.
//!
//! Ids are not drawn from the engine: imported rows keep Plex's, so a new row takes the next
//! number past the highest of its numbering, which items share with collections and extras,
//! and versions, files and tracks with an extra's.
//!
//! Removal leans on the foreign keys the migrations declare `ON DELETE CASCADE` (an item's
//! indexes, tags, locks, history and extras; a file's tracks and track choices; a track's
//! offsets and analysis), and deletes by hand what refers to an item without one: markers and
//! the entries of collections, playlists and play queues. Watch state and ratings are keyed by
//! `guid` and kept, as they are for every copy of a work.

mod analysis;
mod languages;

use open_plex_core::ids::{MediaId, PartId, RatingKey, SectionId};
use open_plex_core::metadata;
use open_plex_core::scan::{
    KnownPart, NewItem, NewMedia, NewPart, ScanError, ScanRepository, ShowKey,
};
use open_plex_scanner::report::Analysis;
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, Condition, ConnectionTrait, DatabaseConnection,
    DbErr, EntityTrait, IntoActiveModel, JoinType, QueryFilter, QueryOrder, QuerySelect,
    RelationTrait, TransactionTrait,
};

use crate::entity::{
    collection, collection_entry, extra, extra_media, extra_track, item_marker, library_section,
    media, media_part, media_stream, metadata_item, play_queue_entry, playlist_entry,
    stream_dolby_vision, track_choice,
};
use crate::write;
use analysis::MediaSummary;

/// `SQLite` refuses a statement with too many bound values.
const BATCH: usize = 200;

#[derive(Debug, Clone)]
pub struct SqlScanRepository {
    connection: DatabaseConnection,
}

impl SqlScanRepository {
    #[must_use]
    pub const fn new(connection: DatabaseConnection) -> Self {
        Self { connection }
    }
}

fn storage(error: DbErr) -> ScanError {
    ScanError::Storage(Box::new(error))
}

#[async_trait::async_trait]
impl ScanRepository for SqlScanRepository {
    async fn known_parts(
        &self,
        section: SectionId,
        path_prefix: &str,
    ) -> Result<Vec<KnownPart>, ScanError> {
        let mut query = media_part::Entity::find()
            .select_only()
            .column(media_part::Column::Id)
            .column(media_part::Column::MediaId)
            .column(media::Column::MetadataItemId)
            .column(media_part::Column::Path)
            .column(media_part::Column::SizeBytes)
            .column(media_part::Column::UpdatedAt)
            .join(JoinType::InnerJoin, media_part::Relation::Media.def())
            .join(JoinType::InnerJoin, media::Relation::MetadataItem.def())
            .filter(metadata_item::Column::LibrarySectionId.eq(section.get()))
            .order_by_asc(media_part::Column::Id);
        if !path_prefix.is_empty() {
            // A range rather than `LIKE`, whose wildcards and case folding a path must not meet.
            query = query.filter(media_part::Column::Path.gte(path_prefix));
            if let Some(upper) = successor(path_prefix) {
                query = query.filter(media_part::Column::Path.lt(upper));
            }
        }
        let rows: Vec<(i64, i64, i64, String, Option<i64>, i64)> = query
            .into_tuple()
            .all(&self.connection)
            .await
            .map_err(storage)?;
        Ok(rows
            .into_iter()
            // The range is exact under a binary collation; this keeps it exact under any.
            .filter(|(.., path, _, _)| path.starts_with(path_prefix))
            .map(|(part, media, item, path, size, modified_at)| KnownPart {
                part: PartId::new(part),
                media: MediaId::new(media),
                item: RatingKey::new(item),
                path,
                size: size.and_then(|size| u64::try_from(size).ok()).unwrap_or(0),
                modified_at,
            })
            .collect())
    }

    async fn add_item(
        &self,
        section: SectionId,
        item: NewItem,
        now: i64,
    ) -> Result<RatingKey, ScanError> {
        let transaction = self.connection.begin().await.map_err(storage)?;
        library_section::Entity::find_by_id(section.get())
            .one(&transaction)
            .await
            .map_err(storage)?
            .ok_or(ScanError::SectionNotFound(section))?;
        let mut ids = Ids::next(&transaction).await.map_err(storage)?;

        let (mut row, versions) = match item {
            NewItem::Movie {
                title,
                year,
                versions,
            } => {
                let mut row = item_row(ids.take_item(), section, "movie", now);
                row.title_sort = Some(title.clone());
                row.title = Some(title);
                row.position = Some(1);
                row.year = year;
                (row, versions)
            }
            NewItem::Episode {
                show,
                season,
                episode,
                released_on,
                versions,
            } => {
                let show_id = find_or_add_show(&transaction, &mut ids, section, &show, now)
                    .await
                    .map_err(storage)?;
                let season_id =
                    find_or_add_season(&transaction, &mut ids, section, show_id, season, now)
                        .await
                        .map_err(storage)?;
                let mut row = item_row(ids.take_item(), section, "episode", now);
                row.parent_id = Some(season_id);
                row.position = Some(episode);
                row.year = show.year;
                row.released_on = released_on;
                (row, versions)
            }
        };

        row.duration_ms = versions.first().and_then(version_duration);
        let item_id = insert_item(&transaction, row).await.map_err(storage)?;
        for version in &versions {
            add_version(&transaction, &mut ids, item_id, version)
                .await
                .map_err(storage)?;
        }
        transaction.commit().await.map_err(storage)?;
        Ok(RatingKey::new(item_id))
    }

    async fn update_part(&self, part: PartId, file: NewPart, now: i64) -> Result<(), ScanError> {
        let transaction = self.connection.begin().await.map_err(storage)?;
        let stored = media_part::Entity::find_by_id(part.get())
            .one(&transaction)
            .await
            .map_err(storage)?
            .ok_or_else(|| storage(DbErr::RecordNotFound(format!("media part {}", part.get()))))?;
        let mut version = media::Entity::find_by_id(stored.media_id)
            .one(&transaction)
            .await
            .map_err(storage)?
            .ok_or_else(|| storage(DbErr::RecordNotFound(format!("media {}", stored.media_id))))?;

        // Counted before the old tracks go, so that no new track takes the number of one a
        // client may still hold.
        let mut ids = Ids::next(&transaction).await.map_err(storage)?;
        // Choices name tracks by id, and every track is about to be replaced.
        track_choice::Entity::delete_many()
            .filter(track_choice::Column::MediaPartId.eq(part.get()))
            .exec(&transaction)
            .await
            .map_err(storage)?;
        media_stream::Entity::delete_many()
            .filter(media_stream::Column::MediaPartId.eq(part.get()))
            .exec(&transaction)
            .await
            .map_err(storage)?;

        let mut row = part_row(&file, stored.media_id, stored.id, stored.position);
        row.created_at = stored.created_at;
        media_part::Entity::update(row.into_active_model().reset_all())
            .exec(&transaction)
            .await
            .map_err(storage)?;
        add_tracks(&transaction, &mut ids, stored.id, &file)
            .await
            .map_err(storage)?;

        let parts = media_part::Entity::find()
            .filter(media_part::Column::MediaId.eq(version.id))
            .order_by_asc(media_part::Column::Position)
            .order_by_asc(media_part::Column::Id)
            .all(&transaction)
            .await
            .map_err(storage)?;
        let duration = summed(parts.iter().map(|part| part.duration_ms));
        // A version is described by its first file; another's change leaves that alone.
        if parts.first().map(|first| first.id) == Some(stored.id) {
            let summary = file
                .analysis
                .as_ref()
                .map(|analysis| analysis::media_summary(analysis, &file.path))
                .unwrap_or_default();
            apply_summary(&mut version, summary);
        }
        version.duration_ms = duration;
        version.updated_at = file.modified_at;
        let item_id = version.metadata_item_id;
        media::Entity::update(version.into_active_model().reset_all())
            .exec(&transaction)
            .await
            .map_err(storage)?;

        let first_version = media::Entity::find()
            .filter(media::Column::MetadataItemId.eq(item_id))
            .order_by_asc(media::Column::Id)
            .one(&transaction)
            .await
            .map_err(storage)?;
        metadata_item::Entity::update_many()
            .col_expr(metadata_item::Column::UpdatedAt, Expr::value(now))
            .col_expr(
                metadata_item::Column::DurationMs,
                Expr::value(first_version.and_then(|version| version.duration_ms)),
            )
            .filter(metadata_item::Column::Id.eq(item_id))
            .exec(&transaction)
            .await
            .map_err(storage)?;
        transaction.commit().await.map_err(storage)
    }

    async fn remove_parts(&self, parts: &[PartId]) -> Result<(), ScanError> {
        let transaction = self.connection.begin().await.map_err(storage)?;
        let parts: Vec<i64> = parts.iter().map(|part| part.get()).collect();
        let mut touched_media = Vec::new();
        for chunk in parts.chunks(BATCH) {
            let media_ids: Vec<i64> = media_part::Entity::find()
                .select_only()
                .column(media_part::Column::MediaId)
                .filter(media_part::Column::Id.is_in(chunk.iter().copied()))
                .into_tuple()
                .all(&transaction)
                .await
                .map_err(storage)?;
            touched_media.extend(media_ids);
            media_part::Entity::delete_many()
                .filter(media_part::Column::Id.is_in(chunk.iter().copied()))
                .exec(&transaction)
                .await
                .map_err(storage)?;
        }

        let empty_media = without_children(
            &transaction,
            touched_media,
            media_part::Entity::find()
                .select_only()
                .column(media_part::Column::MediaId),
            media_part::Column::MediaId,
        )
        .await
        .map_err(storage)?;
        let mut touched_items = Vec::new();
        for chunk in empty_media.chunks(BATCH) {
            let item_ids: Vec<i64> = media::Entity::find()
                .select_only()
                .column(media::Column::MetadataItemId)
                .filter(media::Column::Id.is_in(chunk.iter().copied()))
                .into_tuple()
                .all(&transaction)
                .await
                .map_err(storage)?;
            touched_items.extend(item_ids);
            media::Entity::delete_many()
                .filter(media::Column::Id.is_in(chunk.iter().copied()))
                .exec(&transaction)
                .await
                .map_err(storage)?;
        }

        let empty_items = without_children(
            &transaction,
            touched_items,
            media::Entity::find()
                .select_only()
                .column(media::Column::MetadataItemId),
            media::Column::MetadataItemId,
        )
        .await
        .map_err(storage)?;
        let seasons = remove_items(&transaction, &empty_items, &["movie", "episode"])
            .await
            .map_err(storage)?;
        let shows = remove_childless(&transaction, seasons, "season")
            .await
            .map_err(storage)?;
        remove_childless(&transaction, shows, "show")
            .await
            .map_err(storage)?;
        transaction.commit().await.map_err(storage)
    }

    async fn finish(&self, section: SectionId, at: i64, changed: bool) -> Result<(), ScanError> {
        let transaction = self.connection.begin().await.map_err(storage)?;
        let stored = library_section::Entity::find_by_id(section.get())
            .one(&transaction)
            .await
            .map_err(storage)?
            .ok_or(ScanError::SectionNotFound(section))?;
        let mut row = library_section::ActiveModel {
            id: ActiveValue::Unchanged(stored.id),
            scanned_at: ActiveValue::Set(Some(at)),
            ..Default::default()
        };
        if changed {
            // A change counter, not a time: clients compare it for difference only.
            row.content_changed_at = ActiveValue::Set(stored.content_changed_at.saturating_add(1));
        }
        row.update(&transaction).await.map_err(storage)?;
        transaction.commit().await.map_err(storage)
    }
}

/// The next free number of each numbering a scan writes into.
struct Ids {
    item: i64,
    media: i64,
    part: i64,
    stream: i64,
}

impl Ids {
    async fn next<C: ConnectionTrait>(connection: &C) -> Result<Self, DbErr> {
        let item = highest::<_, metadata_item::Entity>(connection, metadata_item::Column::Id)
            .await?
            .max(highest::<_, collection::Entity>(connection, collection::Column::Id).await?)
            .max(highest::<_, extra::Entity>(connection, extra::Column::Id).await?);
        let media = highest::<_, media::Entity>(connection, media::Column::Id)
            .await?
            .max(highest::<_, extra_media::Entity>(connection, extra_media::Column::Id).await?);
        let part = highest::<_, media_part::Entity>(connection, media_part::Column::Id)
            .await?
            .max(highest::<_, extra_media::Entity>(connection, extra_media::Column::PartId).await?);
        let stream = highest::<_, media_stream::Entity>(connection, media_stream::Column::Id)
            .await?
            .max(highest::<_, extra_track::Entity>(connection, extra_track::Column::Id).await?);
        Ok(Self {
            item: item + 1,
            media: media + 1,
            part: part + 1,
            stream: stream + 1,
        })
    }

    fn take_item(&mut self) -> i64 {
        take(&mut self.item)
    }
}

fn take(counter: &mut i64) -> i64 {
    let id = *counter;
    *counter += 1;
    id
}

async fn highest<C, E>(connection: &C, column: E::Column) -> Result<i64, DbErr>
where
    C: ConnectionTrait,
    E: EntityTrait,
{
    let highest: Option<i64> = E::find()
        .select_only()
        .column_as(column.max(), "highest")
        .into_tuple()
        .one(connection)
        .await?
        .flatten();
    Ok(highest.unwrap_or(0))
}

/// A new item as the reference's scanner writes one before any agent (`docs/scanner/
/// discovery.md`, §6): a `local://` guid, no title until one is given, added now.
fn item_row(id: i64, section: SectionId, kind: &str, now: i64) -> metadata_item::Model {
    metadata_item::Model {
        id,
        library_section_id: section.get(),
        parent_id: None,
        kind: kind.to_owned(),
        guid: format!("local://{id}"),
        title: None,
        title_sort: None,
        sort_key: String::new(),
        original_title: None,
        summary: None,
        tagline: None,
        studio: None,
        content_rating: None,
        content_rating_age: None,
        position: None,
        year: None,
        released_on: None,
        duration_ms: None,
        rating: None,
        audience_rating: None,
        added_at: now,
        updated_at: now,
        refreshed_at: None,
    }
}

async fn find_or_add_show<C: ConnectionTrait>(
    connection: &C,
    ids: &mut Ids,
    section: SectionId,
    show: &ShowKey,
    now: i64,
) -> Result<i64, DbErr> {
    let year = match show.year {
        Some(year) => metadata_item::Column::Year.eq(year),
        None => metadata_item::Column::Year.is_null(),
    };
    let found = metadata_item::Entity::find()
        .filter(metadata_item::Column::LibrarySectionId.eq(section.get()))
        .filter(metadata_item::Column::Kind.eq("show"))
        .filter(metadata_item::Column::Title.eq(show.title.as_str()))
        .filter(year)
        .order_by_asc(metadata_item::Column::Id)
        .one(connection)
        .await?;
    if let Some(found) = found {
        return Ok(found.id);
    }
    let mut row = item_row(ids.take_item(), section, "show", now);
    row.title = Some(show.title.clone());
    row.title_sort = Some(show.title.clone());
    row.position = Some(1);
    row.year = show.year;
    insert_item(connection, row).await
}

async fn find_or_add_season<C: ConnectionTrait>(
    connection: &C,
    ids: &mut Ids,
    section: SectionId,
    show: i64,
    number: i32,
    now: i64,
) -> Result<i64, DbErr> {
    let found = metadata_item::Entity::find()
        .filter(metadata_item::Column::ParentId.eq(show))
        .filter(metadata_item::Column::Kind.eq("season"))
        .filter(metadata_item::Column::Position.eq(number))
        .order_by_asc(metadata_item::Column::Id)
        .one(connection)
        .await?;
    if let Some(found) = found {
        return Ok(found.id);
    }
    let mut row = item_row(ids.take_item(), section, "season", now);
    row.parent_id = Some(show);
    row.position = Some(number);
    insert_item(connection, row).await
}

async fn insert_item<C: ConnectionTrait>(
    connection: &C,
    mut row: metadata_item::Model,
) -> Result<i64, DbErr> {
    row.sort_key = metadata::sort_key(row.title_sort.as_deref(), row.title.as_deref());
    let id = row.id;
    write::insert_items(connection, vec![row.into_active_model().reset_all()]).await?;
    Ok(id)
}

async fn add_version<C: ConnectionTrait>(
    connection: &C,
    ids: &mut Ids,
    item: i64,
    version: &NewMedia,
) -> Result<(), DbErr> {
    let id = take(&mut ids.media);
    let first = version.parts.first();
    let summary = first
        .and_then(|part| Some(analysis::media_summary(part.analysis.as_ref()?, &part.path)))
        .unwrap_or_default();
    // The reference dates a version by its file, not by the scan.
    let modified_at = first.map_or(0, |part| part.modified_at);
    let mut row = media::Model {
        id,
        metadata_item_id: item,
        width: None,
        height: None,
        aspect_ratio: None,
        duration_ms: version_duration(version),
        bitrate: None,
        container: None,
        video_codec: None,
        audio_codec: None,
        audio_channels: None,
        frame_rate: None,
        interlaced: false,
        optimized_for_streaming: false,
        created_at: modified_at,
        updated_at: modified_at,
    };
    apply_summary(&mut row, summary);
    media::Entity::insert(row.into_active_model().reset_all())
        .exec_without_returning(connection)
        .await?;

    for part in &version.parts {
        let part_id = take(&mut ids.part);
        // Stacked files all carry position 0, as the reference leaves their index null.
        media_part::Entity::insert(
            part_row(part, id, part_id, 0)
                .into_active_model()
                .reset_all(),
        )
        .exec_without_returning(connection)
        .await?;
        add_tracks(connection, ids, part_id, part).await?;
    }
    Ok(())
}

fn apply_summary(row: &mut media::Model, summary: MediaSummary) {
    row.width = summary.width;
    row.height = summary.height;
    row.aspect_ratio = summary.aspect_ratio;
    row.bitrate = summary.bitrate;
    row.container = summary.container;
    row.video_codec = summary.video_codec;
    row.audio_codec = summary.audio_codec;
    row.audio_channels = summary.audio_channels;
    row.frame_rate = summary.frame_rate;
    row.interlaced = summary.interlaced;
    row.optimized_for_streaming = summary.optimized_for_streaming;
}

fn part_row(file: &NewPart, media: i64, id: i64, position: i32) -> media_part::Model {
    let analysis = file.analysis.as_ref();
    media_part::Model {
        id,
        media_id: media,
        position,
        path: file.path.clone(),
        size_bytes: i64::try_from(file.size).ok(),
        duration_ms: analysis
            .and_then(Analysis::measured_duration_ms)
            .and_then(|duration| i64::try_from(duration).ok()),
        // The reference keeps the file's time in both.
        created_at: file.modified_at,
        updated_at: file.modified_at,
        has_thumbnail: analysis.is_some_and(analysis::has_thumbnail),
        has_64bit_offsets: analysis.and_then(|analysis| analysis.has_64bit_offsets),
        optimized_for_streaming: analysis.and_then(|analysis| analysis.optimized_for_streaming),
        packet_length: analysis
            .and_then(|analysis| analysis.packet_length)
            .and_then(|length| i32::try_from(length).ok()),
        has_chapter_text_stream: analysis.is_some_and(analysis::has_chapter_text_stream),
        // The reference's identity hash is not one the scanner can reproduce yet.
        content_hash: None,
        open_subtitles_hash: analysis.map(|analysis| analysis.file.open_subtitles_hash.clone()),
    }
}

async fn add_tracks<C: ConnectionTrait>(
    connection: &C,
    ids: &mut Ids,
    part: i64,
    file: &NewPart,
) -> Result<(), DbErr> {
    let embedded = file
        .analysis
        .as_ref()
        .map(|analysis| analysis::track_rows(analysis, &file.path, part, ids.stream))
        .unwrap_or_default();
    ids.stream += i64::try_from(embedded.len()).unwrap_or(i64::MAX);
    let mut streams = Vec::with_capacity(embedded.len() + file.sidecars.len());
    let mut dolby_vision = Vec::new();
    for rows in embedded {
        streams.push(rows.stream.into_active_model().reset_all());
        dolby_vision.extend(
            rows.dolby_vision
                .map(|record| record.into_active_model().reset_all()),
        );
    }
    for sidecar in &file.sidecars {
        let id = take(&mut ids.stream);
        streams.push(
            analysis::sidecar_row(sidecar, part, id)
                .into_active_model()
                .reset_all(),
        );
    }
    // Wide rows: a small chunk keeps every engine under its bound-parameter limit.
    for chunk in streams.chunks(BATCH / 4) {
        media_stream::Entity::insert_many(chunk.to_vec())
            .exec_without_returning(connection)
            .await?;
    }
    if !dolby_vision.is_empty() {
        stream_dolby_vision::Entity::insert_many(dolby_vision)
            .exec_without_returning(connection)
            .await?;
    }
    Ok(())
}

fn version_duration(version: &NewMedia) -> Option<i64> {
    summed(version.parts.iter().map(|part| {
        part.analysis
            .as_ref()
            .and_then(Analysis::measured_duration_ms)
            .and_then(|duration| i64::try_from(duration).ok())
    }))
}

/// The total of the durations known, none when none is.
fn summed(durations: impl Iterator<Item = Option<i64>>) -> Option<i64> {
    durations
        .flatten()
        .fold(None, |total: Option<i64>, duration| {
            Some(total.unwrap_or(0).saturating_add(duration))
        })
}

/// Which of `parents` no row of `children` points at any longer.
async fn without_children<C, E>(
    connection: &C,
    mut parents: Vec<i64>,
    children: sea_orm::Select<E>,
    parent_column: E::Column,
) -> Result<Vec<i64>, DbErr>
where
    C: ConnectionTrait,
    E: EntityTrait,
{
    parents.sort_unstable();
    parents.dedup();
    let mut empty = Vec::new();
    for chunk in parents.chunks(BATCH) {
        let still: Vec<i64> = children
            .clone()
            .filter(Expr::col(parent_column).is_in(chunk.iter().copied()))
            .distinct()
            .into_tuple()
            .all(connection)
            .await?;
        empty.extend(chunk.iter().filter(|id| !still.contains(id)));
    }
    Ok(empty)
}

/// Deletes the items among `ids` of these kinds, with what refers to them, and returns their
/// parents.
async fn remove_items<C: ConnectionTrait>(
    connection: &C,
    ids: &[i64],
    kinds: &[&str],
) -> Result<Vec<i64>, DbErr> {
    let mut parents = Vec::new();
    for chunk in ids.chunks(BATCH) {
        let kept = Condition::all()
            .add(metadata_item::Column::Id.is_in(chunk.iter().copied()))
            .add(metadata_item::Column::Kind.is_in(kinds.iter().copied()));
        let rows: Vec<(i64, Option<i64>)> = metadata_item::Entity::find()
            .select_only()
            .column(metadata_item::Column::Id)
            .column(metadata_item::Column::ParentId)
            .filter(kept)
            .into_tuple()
            .all(connection)
            .await?;
        let removed: Vec<i64> = rows.iter().map(|(id, _)| *id).collect();
        parents.extend(rows.iter().filter_map(|(_, parent)| *parent));
        if removed.is_empty() {
            continue;
        }
        // No foreign key reaches an item from these: a removed number could otherwise come
        // back on the next item added.
        item_marker::Entity::delete_many()
            .filter(item_marker::Column::ItemId.is_in(removed.iter().copied()))
            .exec(connection)
            .await?;
        collection_entry::Entity::delete_many()
            .filter(collection_entry::Column::ItemId.is_in(removed.iter().copied()))
            .exec(connection)
            .await?;
        playlist_entry::Entity::delete_many()
            .filter(playlist_entry::Column::ItemId.is_in(removed.iter().copied()))
            .exec(connection)
            .await?;
        play_queue_entry::Entity::delete_many()
            .filter(play_queue_entry::Column::ItemId.is_in(removed.iter().copied()))
            .exec(connection)
            .await?;
        metadata_item::Entity::delete_many()
            .filter(metadata_item::Column::Id.is_in(removed))
            .exec(connection)
            .await?;
    }
    parents.sort_unstable();
    parents.dedup();
    Ok(parents)
}

/// Deletes the items of `kind` among `ids` that no item has as its parent any longer, and
/// returns their parents.
async fn remove_childless<C: ConnectionTrait>(
    connection: &C,
    ids: Vec<i64>,
    kind: &str,
) -> Result<Vec<i64>, DbErr> {
    let empty = without_children(
        connection,
        ids,
        metadata_item::Entity::find()
            .select_only()
            .column(metadata_item::Column::ParentId),
        metadata_item::Column::ParentId,
    )
    .await?;
    remove_items(connection, &empty, &[kind]).await
}

/// The smallest string greater than every string that begins with `prefix`: its last
/// character that can be incremented, incremented, and the rest dropped.
fn successor(prefix: &str) -> Option<String> {
    let mut characters: Vec<char> = prefix.chars().collect();
    while let Some(last) = characters.pop() {
        let next = match u32::from(last) + 1 {
            0xD800 => 0xE000,
            other => other,
        };
        if let Some(next) = char::from_u32(next) {
            let mut bound: String = characters.into_iter().collect();
            bound.push(next);
            return Some(bound);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_every_path_below_a_prefix() {
        assert_eq!(successor("C:\\Films\\").as_deref(), Some("C:\\Films]"));
        assert_eq!(successor("/mnt/a").as_deref(), Some("/mnt/b"));
        assert_eq!(successor(""), None);
    }

    #[test]
    fn sums_the_durations_known() {
        assert_eq!(summed([Some(2), None, Some(3)].into_iter()), Some(5));
        assert_eq!(summed([None, None].into_iter()), None);
    }
}
