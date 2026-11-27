//! What a library scan writes: new items with their files, files that changed, files that are
//! gone, and what the analysis reads from each file afterwards.
//!
//! Ids are not drawn from the engine: imported rows keep Plex's, so a new row takes the next
//! number past the highest of its numbering, which items share with collections and extras,
//! and versions, files and tracks with an extra's.
//!
//! Removal leans on the foreign keys the migrations declare `ON DELETE CASCADE` (an item's
//! indexes, tags, locks, history and extras; a file's tracks, track choices and tasks in the
//! analysis queue; a track's offsets and analysis), and deletes by hand what refers to an item
//! without one: markers and the entries of collections, playlists and play queues. Watch state
//! and ratings are keyed by `guid` and kept, as they are for every copy of a work.

mod analysis;

use std::collections::HashMap;
use std::sync::Arc;

use klapz_core::chapter::ChapterSource;
use klapz_core::ids::{MediaId, PartId, RatingKey, SectionId};
use klapz_core::metadata;
use klapz_core::scan::{
    AnalysisProgress, ContentHash, KnownPart, MediaTask, NewItem, NewMedia, NewPart, NewSidecar,
    QueueFilter, QueuedFile, ScanError, ScanRepository, ShowKey, Stored, TaskSwitch,
};
use klapz_scanner::report::{Analysis, DeepAnalysis, TrackKind};
use sea_orm::sea_query::{Expr, OnConflict};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, Condition, ConnectionTrait, DatabaseConnection,
    DbErr, EntityTrait, IntoActiveModel, JoinType, PaginatorTrait, QueryFilter, QueryOrder,
    QuerySelect, RelationTrait, TransactionTrait,
};

use crate::entity::media_task::{HEADERS, PACKETS, PREVIEW_THUMBNAILS, SUBTITLES};
use crate::entity::{
    collection, collection_entry, extra, extra_media, extra_track, item_chapter, item_marker,
    library_section, media, media_part, media_stream, media_task, metadata_item, part_side_stream,
    play_queue_entry, playlist_entry, stream_dolby_vision, track_choice,
};
use crate::{chapter_store, write};
use analysis::MediaSummary;

/// `SQLite` refuses a statement with too many bound values.
const BATCH: usize = 200;

#[derive(Debug, Clone)]
pub struct SqlScanRepository {
    connection: DatabaseConnection,
    /// A scan and the analysis write at the same time, and each transaction numbers its rows
    /// past the highest stored (`Ids::next`): two at once would take the same numbers, and
    /// `SQLite` refuses a transaction whose snapshot another write overtook. Theirs go one at
    /// a time, shared by every clone.
    writes: Arc<tokio::sync::Mutex<()>>,
}

impl SqlScanRepository {
    #[must_use]
    pub fn new(connection: DatabaseConnection) -> Self {
        Self {
            connection,
            writes: Arc::default(),
        }
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
        let _writing = self.writes.lock().await;
        let transaction = self.connection.begin().await.map_err(storage)?;
        library_section::Entity::find_by_id(section.get())
            .one(&transaction)
            .await
            .map_err(storage)?
            .ok_or(ScanError::SectionNotFound(section))?;
        let mut ids = Ids::next(&transaction).await.map_err(storage)?;

        let (row, versions) = match item {
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

        let item_id = insert_item(&transaction, row).await.map_err(storage)?;
        let mut parts = Vec::new();
        for version in &versions {
            parts.extend(
                add_version(&transaction, &mut ids, item_id, version)
                    .await
                    .map_err(storage)?,
            );
        }
        let queued: Vec<(i64, i64)> = parts
            .into_iter()
            .map(|part| (part, section.get()))
            .collect();
        queue(&transaction, &queued, now).await.map_err(storage)?;
        transaction.commit().await.map_err(storage)?;
        Ok(RatingKey::new(item_id))
    }

    async fn update_part(&self, part: PartId, file: NewPart, now: i64) -> Result<(), ScanError> {
        let _writing = self.writes.lock().await;
        let transaction = self.connection.begin().await.map_err(storage)?;
        let stored = media_part::Entity::find_by_id(part.get())
            .one(&transaction)
            .await
            .map_err(storage)?
            .ok_or_else(|| storage(DbErr::RecordNotFound(format!("media part {}", part.get()))))?;
        let item = media::Entity::find_by_id(stored.media_id)
            .one(&transaction)
            .await
            .map_err(storage)?
            .ok_or_else(|| storage(DbErr::RecordNotFound(format!("media {}", stored.media_id))))?
            .metadata_item_id;
        let section: i64 = metadata_item::Entity::find_by_id(item)
            .select_only()
            .column(metadata_item::Column::LibrarySectionId)
            .into_tuple()
            .one(&transaction)
            .await
            .map_err(storage)?
            .ok_or_else(|| storage(DbErr::RecordNotFound(format!("item {item}"))))?;

        media_part::Entity::update_many()
            .col_expr(
                media_part::Column::SizeBytes,
                Expr::value(i64::try_from(file.size).ok()),
            )
            .col_expr(media_part::Column::UpdatedAt, Expr::value(file.modified_at))
            .filter(media_part::Column::Id.eq(part.get()))
            .exec(&transaction)
            .await
            .map_err(storage)?;
        // The reference dates a version by its file.
        media::Entity::update_many()
            .col_expr(media::Column::UpdatedAt, Expr::value(file.modified_at))
            .filter(media::Column::Id.eq(stored.media_id))
            .exec(&transaction)
            .await
            .map_err(storage)?;
        let mut ids = Ids::next(&transaction).await.map_err(storage)?;
        replace_sidecars(&transaction, &mut ids, part.get(), &file.sidecars)
            .await
            .map_err(storage)?;
        metadata_item::Entity::update_many()
            .col_expr(metadata_item::Column::UpdatedAt, Expr::value(now))
            .filter(metadata_item::Column::Id.eq(item))
            .exec(&transaction)
            .await
            .map_err(storage)?;
        queue(&transaction, &[(part.get(), section)], now)
            .await
            .map_err(storage)?;
        transaction.commit().await.map_err(storage)
    }

    async fn queue_again(&self, parts: &[PartId], now: i64) -> Result<(), ScanError> {
        let _writing = self.writes.lock().await;
        let transaction = self.connection.begin().await.map_err(storage)?;
        for chunk in parts.chunks(BATCH) {
            let queued: Vec<(i64, i64)> = media_part::Entity::find()
                .select_only()
                .column(media_part::Column::Id)
                .column(metadata_item::Column::LibrarySectionId)
                .join(JoinType::InnerJoin, media_part::Relation::Media.def())
                .join(JoinType::InnerJoin, media::Relation::MetadataItem.def())
                .filter(media_part::Column::Id.is_in(chunk.iter().map(|part| part.get())))
                .into_tuple()
                .all(&transaction)
                .await
                .map_err(storage)?;
            queue(&transaction, &queued, now).await.map_err(storage)?;
        }
        transaction.commit().await.map_err(storage)
    }

    async fn queued_files(
        &self,
        limit: usize,
        filter: &QueueFilter,
    ) -> Result<Vec<QueuedFile>, ScanError> {
        // A file holds a task of each kind at most: this many rows hold `limit` files, when
        // the queue does.
        let rows_held = limit.saturating_mul(MediaTask::ALL.len());
        let mut query = queued_query()
            .order_by_asc(media_task::Column::Task)
            .order_by_asc(media_task::Column::QueuedAt)
            .order_by_asc(media_task::Column::MediaPartId)
            .limit(u64::try_from(rows_held).unwrap_or(u64::MAX));
        if let Some(section) = filter.section {
            query = query.filter(media_task::Column::LibrarySectionId.eq(section.get()));
        }
        if let Some(task) = filter.task {
            query = query.filter(media_task::Column::Task.eq(task_number(task)));
        }
        if !filter.busy.is_empty() {
            query = query.filter(
                media_task::Column::MediaPartId
                    .is_not_in(filter.busy.iter().map(|part| part.get())),
            );
        }
        if !filter.held.is_empty() {
            query = query.filter(
                media_task::Column::LibrarySectionId
                    .is_not_in(filter.held.iter().map(|section| section.get())),
            );
        }
        for switch in &filter.switches {
            query = query.filter(taken_where_on(switch));
        }
        let rows: Vec<QueuedRow> = query
            .into_tuple()
            .all(&self.connection)
            .await
            .map_err(storage)?;
        // One task per file, its first: two at once would read the same file twice.
        let mut files: Vec<QueuedFile> = Vec::with_capacity(limit);
        for file in rows.into_iter().map(queued_file) {
            if files.len() == limit {
                break;
            }
            if !files.iter().any(|taken| taken.part == file.part) {
                files.push(file);
            }
        }
        Ok(files)
    }

    async fn queued_file(&self, part: PartId) -> Result<Option<QueuedFile>, ScanError> {
        let row: Option<QueuedRow> = queued_query()
            .filter(media_task::Column::MediaPartId.eq(part.get()))
            .order_by_asc(media_task::Column::Task)
            .into_tuple()
            .one(&self.connection)
            .await
            .map_err(storage)?;
        Ok(row.map(queued_file))
    }

    async fn store_headers(
        &self,
        file: &QueuedFile,
        analysis: Option<&Analysis>,
        now: i64,
    ) -> Result<Stored, ScanError> {
        let _writing = self.writes.lock().await;
        let transaction = self.connection.begin().await.map_err(storage)?;
        let Some(mut stored) = still_queued(&transaction, file).await.map_err(storage)? else {
            return Ok(Stored::Stale);
        };

        // Counted before the old tracks go, so that no new track takes the number of one a
        // client may still hold.
        let mut ids = Ids::next(&transaction).await.map_err(storage)?;
        // A file read again replaces what it said before; one that no longer reads as media
        // keeps its rows and loses its description.
        remove_embedded_tracks(&transaction, stored.id)
            .await
            .map_err(storage)?;
        if let Some(analysis) = analysis {
            add_tracks(&transaction, &mut ids, stored.id, &stored.path, analysis)
                .await
                .map_err(storage)?;
        }
        apply_headers(&mut stored, analysis);
        media_part::Entity::update(stored.clone().into_active_model().reset_all())
            .exec(&transaction)
            .await
            .map_err(storage)?;
        let summary = analysis
            .map(|analysis| analysis::media_summary(analysis, &stored.path))
            .unwrap_or_default();
        let described = describe(&transaction, &stored, now, |version| {
            apply_summary(version, summary);
        })
        .await
        .map_err(storage)?;
        if let Some(item) = described {
            let chapters = analysis
                .map(|analysis| chapter_rows(item, analysis))
                .unwrap_or_default();
            chapter_store::replace(&transaction, item, ChapterSource::Media, chapters)
                .await
                .map_err(storage)?;
        }
        dequeue(&transaction, file.part, HEADERS)
            .await
            .map_err(storage)?;
        if let Some(analysis) = analysis {
            let follow = following_tasks(analysis);
            let rows = follow.iter().map(|task| media_task::ActiveModel {
                media_part_id: ActiveValue::Set(file.part.get()),
                task: ActiveValue::Set(*task),
                library_section_id: ActiveValue::Set(file.section.get()),
                queued_at: ActiveValue::Set(now),
            });
            media_task::Entity::insert_many(rows)
                .on_conflict(
                    OnConflict::columns([
                        media_task::Column::MediaPartId,
                        media_task::Column::Task,
                    ])
                    .do_nothing()
                    .to_owned(),
                )
                .do_nothing()
                .exec_without_returning(&transaction)
                .await
                .map_err(storage)?;
        }
        transaction.commit().await.map_err(storage)?;
        Ok(Stored::Written)
    }

    async fn store_measurement(
        &self,
        file: &QueuedFile,
        measured: Option<&DeepAnalysis>,
        now: i64,
    ) -> Result<Stored, ScanError> {
        let _writing = self.writes.lock().await;
        let transaction = self.connection.begin().await.map_err(storage)?;
        let Some(mut stored) = still_queued(&transaction, file).await.map_err(storage)? else {
            return Ok(Stored::Stale);
        };
        if let Some(measured) = measured {
            apply_measurement(&transaction, &mut stored, measured)
                .await
                .map_err(storage)?;
            describe(&transaction, &stored, now, |version| {
                version.bitrate = analysis::to_i32(measured.bitrate);
            })
            .await
            .map_err(storage)?;
        }
        dequeue(&transaction, file.part, PACKETS)
            .await
            .map_err(storage)?;
        transaction.commit().await.map_err(storage)?;
        Ok(Stored::Written)
    }

    async fn finish_task(&self, file: &QueuedFile) -> Result<Stored, ScanError> {
        let _writing = self.writes.lock().await;
        let transaction = self.connection.begin().await.map_err(storage)?;
        if still_queued(&transaction, file)
            .await
            .map_err(storage)?
            .is_none()
        {
            return Ok(Stored::Stale);
        }
        dequeue(&transaction, file.part, task_number(file.task))
            .await
            .map_err(storage)?;
        transaction.commit().await.map_err(storage)?;
        Ok(Stored::Written)
    }

    async fn analysis_progress(
        &self,
        section: SectionId,
        counted: &[MediaTask],
    ) -> Result<AnalysisProgress, ScanError> {
        let files = media_part::Entity::find()
            .join(JoinType::InnerJoin, media_part::Relation::Media.def())
            .join(JoinType::InnerJoin, media::Relation::MetadataItem.def())
            .filter(metadata_item::Column::LibrarySectionId.eq(section.get()))
            .count(&self.connection)
            .await
            .map_err(storage)?;
        let left = |tasks: Vec<i32>| {
            media_task::Entity::find()
                .select_only()
                .column(media_task::Column::MediaPartId)
                .distinct()
                .filter(media_task::Column::LibrarySectionId.eq(section.get()))
                .filter(media_task::Column::Task.is_in(tasks))
                .count(&self.connection)
        };
        let unread = left(vec![HEADERS]).await.map_err(storage)?;
        let incomplete = left(
            std::iter::once(HEADERS)
                .chain(counted.iter().map(|task| task_number(*task)))
                .collect(),
        )
        .await
        .map_err(storage)?;
        Ok(AnalysisProgress {
            files,
            read: files.saturating_sub(unread),
            complete: files.saturating_sub(incomplete),
        })
    }

    async fn contents_changed(&self, section: SectionId) -> Result<(), ScanError> {
        let _writing = self.writes.lock().await;
        library_section::Entity::update_many()
            .col_expr(
                library_section::Column::ContentChangedAt,
                Expr::col(library_section::Column::ContentChangedAt).add(1),
            )
            .filter(library_section::Column::Id.eq(section.get()))
            .exec(&self.connection)
            .await
            .map_err(storage)?;
        Ok(())
    }

    async fn remove_parts(&self, parts: &[PartId]) -> Result<(), ScanError> {
        let _writing = self.writes.lock().await;
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
        let _writing = self.writes.lock().await;
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
        chapter_source: None,
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

/// Writes a version as its files were found, nothing of their media known yet, and returns
/// its files' ids.
async fn add_version<C: ConnectionTrait>(
    connection: &C,
    ids: &mut Ids,
    item: i64,
    version: &NewMedia,
) -> Result<Vec<i64>, DbErr> {
    let id = take(&mut ids.media);
    // The reference dates a version by its file, not by the scan.
    let modified_at = version.parts.first().map_or(0, |part| part.modified_at);
    let row = media::Model {
        id,
        metadata_item_id: item,
        width: None,
        height: None,
        aspect_ratio: None,
        duration_ms: None,
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
    media::Entity::insert(row.into_active_model().reset_all())
        .exec_without_returning(connection)
        .await?;

    let mut parts = Vec::with_capacity(version.parts.len());
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
        add_sidecars(connection, ids, part_id, &part.sidecars).await?;
        parts.push(part_id);
    }
    Ok(parts)
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

/// A file as found: what its headers say comes with [`apply_headers`].
fn part_row(file: &NewPart, media: i64, id: i64, position: i32) -> media_part::Model {
    media_part::Model {
        id,
        media_id: media,
        position,
        path: file.path.clone(),
        size_bytes: i64::try_from(file.size).ok(),
        duration_ms: None,
        // The reference keeps the file's time in both.
        created_at: file.modified_at,
        updated_at: file.modified_at,
        has_thumbnail: false,
        has_64bit_offsets: None,
        optimized_for_streaming: None,
        packet_length: None,
        has_chapter_text_stream: false,
        content_hash: None,
        open_subtitles_hash: None,
        format_name: None,
        declared_duration_us: None,
        declared_bitrate: None,
    }
}

/// What a file's headers say of it, or nothing for a file that is not media.
fn apply_headers(row: &mut media_part::Model, analysis: Option<&Analysis>) {
    row.duration_ms = analysis
        .and_then(Analysis::measured_duration_ms)
        .and_then(|duration| i64::try_from(duration).ok());
    row.has_thumbnail = analysis.is_some_and(analysis::has_thumbnail);
    row.has_64bit_offsets = analysis.and_then(|analysis| analysis.has_64bit_offsets);
    row.optimized_for_streaming = analysis.and_then(|analysis| analysis.optimized_for_streaming);
    row.packet_length = analysis
        .and_then(|analysis| analysis.packet_length)
        .and_then(|length| i32::try_from(length).ok());
    row.has_chapter_text_stream = analysis.is_some_and(analysis::has_chapter_text_stream);
    row.content_hash = analysis.and_then(|analysis| analysis.file.content_hash.clone());
    row.open_subtitles_hash =
        analysis.and_then(|analysis| analysis.file.open_subtitles_hash.clone());
    row.format_name = analysis.map(|analysis| analysis.container.clone());
    row.declared_duration_us = analysis
        .and_then(|analysis| analysis.duration_us)
        .and_then(|duration| i64::try_from(duration).ok());
    row.declared_bitrate = analysis
        .and_then(|analysis| analysis.bitrate)
        .and_then(|rate| i64::try_from(rate).ok());
}

/// The tracks inside a file, and what it carries besides them.
async fn add_tracks<C: ConnectionTrait>(
    connection: &C,
    ids: &mut Ids,
    part: i64,
    path: &str,
    analysis: &Analysis,
) -> Result<(), DbErr> {
    let embedded = analysis::track_rows(analysis, path, part, ids.stream);
    ids.stream += i64::try_from(embedded.len()).unwrap_or(i64::MAX);
    let mut streams = Vec::with_capacity(embedded.len());
    let mut dolby_vision = Vec::new();
    for rows in embedded {
        streams.push(rows.stream.into_active_model().reset_all());
        dolby_vision.extend(
            rows.dolby_vision
                .map(|record| record.into_active_model().reset_all()),
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
    let side_streams = analysis::side_stream_rows(analysis, part);
    for chunk in side_streams.chunks(BATCH / 4) {
        part_side_stream::Entity::insert_many(chunk.to_vec())
            .exec_without_returning(connection)
            .await?;
    }
    Ok(())
}

async fn add_sidecars<C: ConnectionTrait>(
    connection: &C,
    ids: &mut Ids,
    part: i64,
    sidecars: &[NewSidecar],
) -> Result<(), DbErr> {
    let rows: Vec<media_stream::ActiveModel> = sidecars
        .iter()
        .map(|sidecar| {
            analysis::sidecar_row(sidecar, part, take(&mut ids.stream))
                .into_active_model()
                .reset_all()
        })
        .collect();
    for chunk in rows.chunks(BATCH / 4) {
        media_stream::Entity::insert_many(chunk.to_vec())
            .exec_without_returning(connection)
            .await?;
    }
    Ok(())
}

/// Removes the tracks inside a file, about to be read again, and the viewers' choices that
/// name one: its sidecars stay, with their ids, the choices and offsets that name them.
async fn remove_embedded_tracks<C: ConnectionTrait>(
    connection: &C,
    part: i64,
) -> Result<(), DbErr> {
    let embedded: Vec<i64> = media_stream::Entity::find()
        .select_only()
        .column(media_stream::Column::Id)
        .filter(media_stream::Column::MediaPartId.eq(part))
        .filter(media_stream::Column::ExternalPath.is_null())
        .into_tuple()
        .all(connection)
        .await?;
    forget_tracks(connection, part, &embedded).await?;
    part_side_stream::Entity::delete_many()
        .filter(part_side_stream::Column::MediaPartId.eq(part))
        .exec(connection)
        .await?;
    Ok(())
}

/// Gives a changed file the sidecars found beside it now: one still there keeps its id, and
/// what viewers chose of it.
async fn replace_sidecars<C: ConnectionTrait>(
    connection: &C,
    ids: &mut Ids,
    part: i64,
    sidecars: &[NewSidecar],
) -> Result<(), DbErr> {
    let stored: Vec<(i64, Option<String>)> = media_stream::Entity::find()
        .select_only()
        .column(media_stream::Column::Id)
        .column(media_stream::Column::ExternalPath)
        .filter(media_stream::Column::MediaPartId.eq(part))
        .filter(media_stream::Column::ExternalPath.is_not_null())
        .into_tuple()
        .all(connection)
        .await?;
    let found: HashMap<String, &NewSidecar> = sidecars
        .iter()
        .map(|sidecar| (analysis::file_url(&sidecar.path), sidecar))
        .collect();
    let gone: Vec<i64> = stored
        .iter()
        .filter(|(_, url)| url.as_ref().is_none_or(|url| !found.contains_key(url)))
        .map(|(id, _)| *id)
        .collect();
    forget_tracks(connection, part, &gone).await?;
    let new: Vec<NewSidecar> = sidecars
        .iter()
        .filter(|sidecar| {
            let url = analysis::file_url(&sidecar.path);
            !stored.iter().any(|(_, kept)| kept.as_ref() == Some(&url))
        })
        .cloned()
        .collect();
    add_sidecars(connection, ids, part, &new).await
}

/// Deletes these tracks of a file, and every viewer's choice naming one of them: a choice
/// whose track went is no choice. Their offsets go with them.
async fn forget_tracks<C: ConnectionTrait>(
    connection: &C,
    part: i64,
    tracks: &[i64],
) -> Result<(), DbErr> {
    for chunk in tracks.chunks(BATCH) {
        track_choice::Entity::delete_many()
            .filter(track_choice::Column::MediaPartId.eq(part))
            .filter(
                Condition::any()
                    .add(track_choice::Column::AudioStreamId.is_in(chunk.iter().copied()))
                    .add(track_choice::Column::SubtitleStreamId.is_in(chunk.iter().copied())),
            )
            .exec(connection)
            .await?;
        media_stream::Entity::delete_many()
            .filter(media_stream::Column::Id.is_in(chunk.iter().copied()))
            .exec(connection)
            .await?;
    }
    Ok(())
}

/// Writes what the packets measured onto the tracks the headers gave, found by their index in
/// the file: measured or nothing, as the deep analysis has it (`analysis::track_bitrate`).
async fn apply_measurement<C: ConnectionTrait>(
    connection: &C,
    part: &mut media_part::Model,
    measured: &DeepAnalysis,
) -> Result<(), DbErr> {
    let embedded: Vec<(i64, i32)> = media_stream::Entity::find()
        .select_only()
        .column(media_stream::Column::Id)
        .column(media_stream::Column::Position)
        .filter(media_stream::Column::MediaPartId.eq(part.id))
        .filter(media_stream::Column::ExternalPath.is_null())
        .into_tuple()
        .all(connection)
        .await?;
    for (id, position) in embedded {
        let rate = measured
            .tracks
            .iter()
            .find(|track| i64::from(track.index) == i64::from(position))
            .map(|track| track.bitrate)
            .filter(|rate| *rate > 0)
            .and_then(analysis::to_i32);
        media_stream::Entity::update_many()
            .col_expr(media_stream::Column::Bitrate, Expr::value(rate))
            .filter(media_stream::Column::Id.eq(id))
            .exec(connection)
            .await?;
    }
    if let Some(duration) = measured
        .duration_ms
        .and_then(|duration| i64::try_from(duration).ok())
    {
        part.duration_ms = Some(duration);
        media_part::Entity::update_many()
            .col_expr(media_part::Column::DurationMs, Expr::value(duration))
            .filter(media_part::Column::Id.eq(part.id))
            .exec(connection)
            .await?;
    }
    Ok(())
}

/// Carries a file's change up to its version and item: `first` changes the version when the
/// file is its first, which describes it; the version's duration sums its files', and the
/// item's is its first version's. Returns the item when the file is the one that describes it,
/// whose chapters are then the file's.
async fn describe<C: ConnectionTrait>(
    connection: &C,
    part: &media_part::Model,
    now: i64,
    first: impl FnOnce(&mut media::Model),
) -> Result<Option<i64>, DbErr> {
    let mut version = media::Entity::find_by_id(part.media_id)
        .one(connection)
        .await?
        .ok_or_else(|| DbErr::RecordNotFound(format!("media {}", part.media_id)))?;
    let parts = media_part::Entity::find()
        .filter(media_part::Column::MediaId.eq(version.id))
        .order_by_asc(media_part::Column::Position)
        .order_by_asc(media_part::Column::Id)
        .all(connection)
        .await?;
    let first_file = parts.first().map(|first| first.id) == Some(part.id);
    if first_file {
        first(&mut version);
    }
    version.duration_ms = summed(parts.iter().map(|part| part.duration_ms));
    let item = version.metadata_item_id;
    let version_id = version.id;
    media::Entity::update(version.into_active_model().reset_all())
        .exec(connection)
        .await?;

    let first_version = media::Entity::find()
        .filter(media::Column::MetadataItemId.eq(item))
        .order_by_asc(media::Column::Id)
        .one(connection)
        .await?;
    let describes_item =
        first_file && first_version.as_ref().map(|first| first.id) == Some(version_id);
    metadata_item::Entity::update_many()
        .col_expr(metadata_item::Column::UpdatedAt, Expr::value(now))
        .col_expr(
            metadata_item::Column::DurationMs,
            Expr::value(first_version.and_then(|version| version.duration_ms)),
        )
        .filter(metadata_item::Column::Id.eq(item))
        .exec(connection)
        .await?;
    Ok(describes_item.then_some(item))
}

/// The chapters of the file that describes an item: its first version's first, as for the
/// item's duration.
fn chapter_rows(item: i64, analysis: &Analysis) -> Vec<item_chapter::ActiveModel> {
    analysis
        .chapters
        .iter()
        .map(|chapter| {
            let mut row = chapter_store::row(
                item,
                i32::try_from(chapter.index).unwrap_or(i32::MAX),
                i64::try_from(chapter.start_ms).unwrap_or(i64::MAX),
                i64::try_from(chapter.end_ms).unwrap_or(i64::MAX),
                chapter.title.clone(),
                None,
            );
            row.declared_title = sea_orm::ActiveValue::Set(chapter.declared_title.clone());
            row.declared_start_us =
                sea_orm::ActiveValue::Set(chapter.start_us.and_then(|us| i64::try_from(us).ok()));
            row
        })
        .collect()
}

/// The total of the durations known, none when none is.
fn summed(durations: impl Iterator<Item = Option<i64>>) -> Option<i64> {
    durations
        .flatten()
        .fold(None, |total: Option<i64>, duration| {
            Some(total.unwrap_or(0).saturating_add(duration))
        })
}

const fn task_number(task: MediaTask) -> i32 {
    match task {
        MediaTask::Headers => HEADERS,
        MediaTask::Packets => PACKETS,
        MediaTask::Subtitles => SUBTITLES,
        MediaTask::PreviewThumbnails => PREVIEW_THUMBNAILS,
    }
}

fn task_of(number: i32) -> Option<MediaTask> {
    MediaTask::ALL
        .into_iter()
        .find(|task| task_number(*task) == number)
}

/// What a file's headers call for once read: its packets measured, its subtitle tracks
/// extracted when it has any, its thumbnails made when it has a video track that is not a
/// cover.
fn following_tasks(analysis: &Analysis) -> Vec<i32> {
    let has = |kind: TrackKind| {
        analysis
            .tracks
            .iter()
            .any(|track| track.kind == kind && !track.disposition.attached_picture)
    };
    let mut tasks = vec![PACKETS];
    if has(TrackKind::Subtitle) {
        tasks.push(SUBTITLES);
    }
    if has(TrackKind::Video) {
        tasks.push(PREVIEW_THUMBNAILS);
    }
    tasks
}

/// The rows of a task the queue may take: those of libraries where it is on.
fn taken_where_on(switch: &TaskSwitch) -> Condition {
    let other_task = media_task::Column::Task.ne(task_number(switch.task));
    let except = switch.switch.except.iter().map(|section| section.get());
    let section = media_task::Column::LibrarySectionId;
    match (switch.switch.server, switch.switch.except.is_empty()) {
        (true, true) => Condition::all(),
        (true, false) => Condition::any()
            .add(other_task)
            .add(section.is_not_in(except)),
        (false, true) => Condition::all().add(other_task),
        (false, false) => Condition::any().add(other_task).add(section.is_in(except)),
    }
}

/// Queues files to be read from their headers, in place of whatever was left on them.
async fn queue<C: ConnectionTrait>(
    connection: &C,
    files: &[(i64, i64)],
    now: i64,
) -> Result<(), DbErr> {
    for chunk in files.chunks(BATCH / 4) {
        media_task::Entity::delete_many()
            .filter(media_task::Column::MediaPartId.is_in(chunk.iter().map(|(part, _)| *part)))
            .exec(connection)
            .await?;
        let rows = chunk.iter().map(|(part, section)| media_task::ActiveModel {
            media_part_id: ActiveValue::Set(*part),
            task: ActiveValue::Set(HEADERS),
            library_section_id: ActiveValue::Set(*section),
            queued_at: ActiveValue::Set(now),
        });
        media_task::Entity::insert_many(rows)
            .exec_without_returning(connection)
            .await?;
    }
    Ok(())
}

async fn dequeue<C: ConnectionTrait>(connection: &C, part: PartId, task: i32) -> Result<(), DbErr> {
    media_task::Entity::delete_by_id((part.get(), task))
        .exec(connection)
        .await?;
    Ok(())
}

/// The file, when its task is still queued and the file is as it was when the task was taken:
/// a scan that found it changed has queued it again, and a run that finished first took the
/// task off.
async fn still_queued<C: ConnectionTrait>(
    connection: &C,
    file: &QueuedFile,
) -> Result<Option<media_part::Model>, DbErr> {
    let queued = media_task::Entity::find_by_id((file.part.get(), task_number(file.task)))
        .one(connection)
        .await?
        .is_some();
    if !queued {
        return Ok(None);
    }
    Ok(media_part::Entity::find_by_id(file.part.get())
        .one(connection)
        .await?
        .filter(|part| {
            part.size_bytes == i64::try_from(file.size).ok() && part.updated_at == file.modified_at
        }))
}

/// Part, library, task, path, size, time, duration and content hash of a queued task.
type QueuedRow = (
    i64,
    i64,
    i32,
    String,
    Option<i64>,
    i64,
    Option<i64>,
    Option<String>,
);

fn queued_query() -> sea_orm::Select<media_task::Entity> {
    media_task::Entity::find()
        .select_only()
        .column(media_task::Column::MediaPartId)
        .column(media_task::Column::LibrarySectionId)
        .column(media_task::Column::Task)
        .column(media_part::Column::Path)
        .column(media_part::Column::SizeBytes)
        .column(media_part::Column::UpdatedAt)
        .column(media_part::Column::DurationMs)
        .column(media_part::Column::ContentHash)
        .join(JoinType::InnerJoin, media_task::Relation::MediaPart.def())
        // A task a later version no longer knows is not taken.
        .filter(media_task::Column::Task.is_in(MediaTask::ALL.map(task_number)))
}

fn queued_file(
    (part, section, task, path, size, modified_at, duration, content_hash): QueuedRow,
) -> QueuedFile {
    QueuedFile {
        part: PartId::new(part),
        section: SectionId::new(section),
        path,
        size: size.and_then(|size| u64::try_from(size).ok()).unwrap_or(0),
        modified_at,
        task: task_of(task).unwrap_or(MediaTask::Headers),
        duration_ms: duration.and_then(|duration| u64::try_from(duration).ok()),
        content_hash: content_hash.as_deref().and_then(ContentHash::parse),
    }
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
