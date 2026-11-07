//! Importing library contents from a Plex database.
//!
//! Four passes in dependency order, because the foreign keys are real: items (parents before
//! children), then media, files and tracks. Each pass reads the source in pages and writes in
//! chunks, so memory does not scale with the library — 204,388 items and 947,346 tracks in the
//! reference database.
//!
//! Rows whose parent does not exist are skipped and counted rather than allowed to fail the
//! foreign key. An import that dies on row 138,000 of 138,981 is worse than one that reports
//! what it could not place.
//!
//! The skip cascades, and the reference library shows why that matters: one season has a
//! dangling parent, and skipping it orphans its 32 episodes too — 34 rows reported, from two
//! corrupt references.
//!
//! Tags come last, as two more passes: the tag values, then their attachment to items.

use std::collections::HashSet;

use open_plex_core::{Tag, TagId, TagKind};
use sea_orm::{ActiveValue, ConnectionTrait, DatabaseConnection, EntityTrait, Statement};

use super::{source_error, ImportError, PlexRow, SOURCE_PAGE};
use crate::entity::{item_tag, media, media_part, media_stream, metadata_item, tag};
use crate::write;

/// How many rows to write per statement.
///
/// Chosen for the narrowest backend: `SQLite` binds one parameter per column per row, and
/// `metadata_item` has 23 columns, so 500 rows is 11,500 parameters — comfortably inside
/// every engine's limit.
const WRITE_CHUNK: usize = 500;

/// Plex's `metadata_type` values we import, in the order their foreign keys require.
const KINDS: [(i32, &str); 4] = [(1, "movie"), (2, "show"), (3, "season"), (4, "episode")];

#[derive(Debug, Default, Clone, Copy)]
pub struct MetadataSummary {
    pub items: usize,
    /// Rows written to the title word index, which the search filter reads.
    pub title_words: usize,
    pub media: usize,
    pub files: usize,
    pub streams: usize,
    pub tags: usize,
    /// Tags attached to items: one row per genre, country or credit.
    pub item_tags: usize,
    /// Rows skipped because the parent they reference does not exist.
    pub orphans: usize,
}

/// Copies items and their media chain.
///
/// # Errors
///
/// Fails if the source cannot be read or a write fails.
pub async fn import(
    destination: &DatabaseConnection,
    source: &DatabaseConnection,
    source_path: &str,
) -> Result<MetadataSummary, ImportError> {
    let mut summary = MetadataSummary::default();
    let mut item_ids: HashSet<i64> = HashSet::new();

    for (metadata_type, kind) in KINDS {
        let (written, orphans, words) = import_items(
            destination,
            source,
            source_path,
            metadata_type,
            kind,
            &mut item_ids,
        )
        .await?;
        summary.items += written;
        summary.orphans += orphans;
        summary.title_words += words;
        tracing::info!(kind, written, orphans, words, "imported items");
    }

    let media_ids = import_media(destination, source, source_path, &item_ids).await?;
    summary.media = media_ids.len();
    tracing::info!(count = summary.media, "imported media");

    let part_ids = import_parts(destination, source, source_path, &media_ids).await?;
    summary.files = part_ids.len();
    tracing::info!(count = summary.files, "imported files");

    summary.streams = import_streams(destination, source, source_path, &part_ids).await?;
    tracing::info!(count = summary.streams, "imported streams");

    let tag_ids = import_tags(destination, source, source_path).await?;
    summary.tags = tag_ids.len();
    tracing::info!(count = summary.tags, "imported tags");

    summary.item_tags =
        import_item_tags(destination, source, source_path, &item_ids, &tag_ids).await?;
    tracing::info!(count = summary.item_tags, "imported item tags");

    Ok(summary)
}

/// Imports one kind of item, with its searchable words.
///
/// Returns how many items were written, how many were orphaned, and how many word rows the
/// title index gained.
async fn import_items(
    destination: &DatabaseConnection,
    source: &DatabaseConnection,
    source_path: &str,
    metadata_type: i32,
    kind: &str,
    item_ids: &mut HashSet<i64>,
) -> Result<(usize, usize, usize), ImportError> {
    let needs_parent = metadata_type == 3 || metadata_type == 4;
    let mut offset = 0_u64;
    let (mut written, mut orphans) = (0_usize, 0_usize);
    let mut indexed_words = 0_usize;

    loop {
        let rows = source
            .query_all(Statement::from_string(
                sea_orm::DbBackend::Sqlite,
                format!(
                    "SELECT id, library_section_id, parent_id, guid, title, title_sort, \
                     original_title, summary, tagline, studio, content_rating, \
                     content_rating_age, \"index\", year, originally_available_at, duration, \
                     rating, audience_rating, added_at, updated_at \
                     FROM metadata_items WHERE metadata_type = {metadata_type} \
                     AND library_section_id IS NOT NULL \
                     ORDER BY id LIMIT {SOURCE_PAGE} OFFSET {offset}"
                ),
            ))
            .await
            .map_err(|error| source_error(source_path, error))?;

        if rows.is_empty() {
            break;
        }
        offset += rows.len() as u64;

        let mut batch = Vec::with_capacity(rows.len());
        for row in &rows {
            let id: i64 = row.plex_get("id")?;
            let parent_id: Option<i64> = row.plex_opt("parent_id");

            // A season or episode whose parent was not imported cannot satisfy the foreign
            // key — whether the parent is corrupt in the source or was itself skipped.
            if needs_parent && !parent_id.is_some_and(|parent| item_ids.contains(&parent)) {
                tracing::warn!(id, kind, ?parent_id, "skipping item with an absent parent");
                orphans += 1;
                continue;
            }

            // Empty means absent, and storing it as such avoids the empty-string sentinel
            // we reject in the source schema.
            let title = row.plex_text("title");
            let title_sort = row.plex_text("title_sort");
            let original_title = row.plex_text("original_title");
            let sort_key = title_sort
                .clone()
                .or_else(|| title.clone())
                .unwrap_or_default()
                .to_lowercase();

            batch.push(metadata_item::ActiveModel {
                id: ActiveValue::Set(id),
                library_section_id: ActiveValue::Set(row.plex_get("library_section_id")?),
                parent_id: ActiveValue::Set(parent_id),
                kind: ActiveValue::Set(kind.to_owned()),
                guid: ActiveValue::Set(row.plex_get("guid")?),
                // Materialized so one index serves ordering and search alike.
                sort_key: ActiveValue::Set(sort_key),
                title: ActiveValue::Set(title),
                title_sort: ActiveValue::Set(title_sort),
                original_title: ActiveValue::Set(original_title),
                summary: ActiveValue::Set(row.plex_text("summary")),
                tagline: ActiveValue::Set(row.plex_text("tagline")),
                studio: ActiveValue::Set(row.plex_text("studio")),
                content_rating: ActiveValue::Set(row.plex_text("content_rating")),
                content_rating_age: ActiveValue::Set(row.plex_opt("content_rating_age")),
                position: ActiveValue::Set(row.plex_opt("index")),
                year: ActiveValue::Set(row.plex_opt("year")),
                // Stored as an epoch in the source; the API publishes a calendar date.
                released_on: ActiveValue::Set(
                    row.plex_opt::<i64>("originally_available_at")
                        .and_then(to_date),
                ),
                duration_ms: ActiveValue::Set(row.plex_opt("duration")),
                rating: ActiveValue::Set(row.plex_opt("rating")),
                audience_rating: ActiveValue::Set(row.plex_opt("audience_rating")),
                added_at: ActiveValue::Set(row.plex_opt("added_at").unwrap_or(0)),
                updated_at: ActiveValue::Set(row.plex_opt("updated_at").unwrap_or(0)),
            });
            item_ids.insert(id);
        }

        written += batch.len();
        // The one write path, which derives the title-word index from the models themselves.
        indexed_words += write::insert_items(destination, batch).await?;
    }

    Ok((written, orphans, indexed_words))
}

async fn import_media(
    destination: &DatabaseConnection,
    source: &DatabaseConnection,
    source_path: &str,
    item_ids: &HashSet<i64>,
) -> Result<HashSet<i64>, ImportError> {
    let mut written = HashSet::new();
    let mut offset = 0_u64;

    loop {
        let rows = source
            .query_all(Statement::from_string(
                sea_orm::DbBackend::Sqlite,
                format!(
                    "SELECT id, metadata_item_id, width, height, display_aspect_ratio, \
                     duration, bitrate, container, video_codec, audio_codec, audio_channels, \
                     frames_per_second, interlaced, optimized_for_streaming, created_at, \
                     updated_at FROM media_items ORDER BY id LIMIT {SOURCE_PAGE} \
                     OFFSET {offset}"
                ),
            ))
            .await
            .map_err(|error| source_error(source_path, error))?;

        if rows.is_empty() {
            break;
        }
        offset += rows.len() as u64;

        let mut batch = Vec::with_capacity(rows.len());
        for row in &rows {
            let Some(item_id) = row.plex_opt::<i64>("metadata_item_id") else {
                continue;
            };
            // Media belonging to a kind we do not import — extras, collections, tracks.
            if !item_ids.contains(&item_id) {
                continue;
            }
            let id: i64 = row.plex_get("id")?;
            batch.push(media::ActiveModel {
                id: ActiveValue::Set(id),
                metadata_item_id: ActiveValue::Set(item_id),
                width: ActiveValue::Set(row.plex_opt("width")),
                height: ActiveValue::Set(row.plex_opt("height")),
                aspect_ratio: ActiveValue::Set(row.plex_opt("display_aspect_ratio")),
                duration_ms: ActiveValue::Set(row.plex_opt("duration")),
                bitrate: ActiveValue::Set(row.plex_opt("bitrate")),
                container: ActiveValue::Set(row.plex_text("container")),
                video_codec: ActiveValue::Set(row.plex_text("video_codec")),
                audio_codec: ActiveValue::Set(row.plex_text("audio_codec")),
                audio_channels: ActiveValue::Set(row.plex_opt("audio_channels")),
                frame_rate: ActiveValue::Set(row.plex_opt("frames_per_second")),
                // Null in the source means "not interlaced" and "not prepared", rather than
                // unknown: Plex only writes these when they are true.
                interlaced: ActiveValue::Set(row.plex_flag("interlaced")),
                optimized_for_streaming: ActiveValue::Set(row.plex_flag("optimized_for_streaming")),
                created_at: ActiveValue::Set(row.plex_opt("created_at").unwrap_or(0)),
                updated_at: ActiveValue::Set(row.plex_opt("updated_at").unwrap_or(0)),
            });
            written.insert(id);
        }

        for chunk in batch.chunks(WRITE_CHUNK) {
            media::Entity::insert_many(chunk.to_vec())
                .exec(destination)
                .await?;
        }
    }

    Ok(written)
}

async fn import_parts(
    destination: &DatabaseConnection,
    source: &DatabaseConnection,
    source_path: &str,
    media_ids: &HashSet<i64>,
) -> Result<HashSet<i64>, ImportError> {
    let mut written = HashSet::new();
    let mut offset = 0_u64;

    loop {
        let rows = source
            .query_all(Statement::from_string(
                sea_orm::DbBackend::Sqlite,
                format!(
                    "SELECT id, media_item_id, \"index\", file, size, duration, created_at, \
                     updated_at FROM media_parts ORDER BY id LIMIT {SOURCE_PAGE} \
                     OFFSET {offset}"
                ),
            ))
            .await
            .map_err(|error| source_error(source_path, error))?;

        if rows.is_empty() {
            break;
        }
        offset += rows.len() as u64;

        let mut batch = Vec::with_capacity(rows.len());
        for row in &rows {
            let Some(media_id) = row.plex_opt::<i64>("media_item_id") else {
                continue;
            };
            if !media_ids.contains(&media_id) {
                continue;
            }
            // A part with no path cannot be served, so it is not worth a row.
            let Some(path) = row.plex_text("file") else {
                continue;
            };
            let id: i64 = row.plex_get("id")?;
            batch.push(media_part::ActiveModel {
                id: ActiveValue::Set(id),
                media_id: ActiveValue::Set(media_id),
                position: ActiveValue::Set(row.plex_opt("index").unwrap_or(0)),
                path: ActiveValue::Set(path),
                size_bytes: ActiveValue::Set(row.plex_opt("size")),
                duration_ms: ActiveValue::Set(row.plex_opt("duration")),
                created_at: ActiveValue::Set(row.plex_opt("created_at").unwrap_or(0)),
                updated_at: ActiveValue::Set(row.plex_opt("updated_at").unwrap_or(0)),
            });
            written.insert(id);
        }

        for chunk in batch.chunks(WRITE_CHUNK) {
            media_part::Entity::insert_many(chunk.to_vec())
                .exec(destination)
                .await?;
        }
    }

    Ok(written)
}

async fn import_streams(
    destination: &DatabaseConnection,
    source: &DatabaseConnection,
    source_path: &str,
    part_ids: &HashSet<i64>,
) -> Result<usize, ImportError> {
    let mut written = 0_usize;
    let mut offset = 0_u64;

    loop {
        let rows = source
            .query_all(Statement::from_string(
                sea_orm::DbBackend::Sqlite,
                format!(
                    "SELECT id, media_part_id, stream_type_id, \"index\", codec, language, \
                     channels, bitrate, \"default\", forced, url FROM media_streams \
                     ORDER BY id LIMIT {SOURCE_PAGE} OFFSET {offset}"
                ),
            ))
            .await
            .map_err(|error| source_error(source_path, error))?;

        if rows.is_empty() {
            break;
        }
        offset += rows.len() as u64;

        let mut batch = Vec::with_capacity(rows.len());
        for row in &rows {
            let Some(part_id) = row.plex_opt::<i64>("media_part_id") else {
                continue;
            };
            if !part_ids.contains(&part_id) {
                continue;
            }
            let Some(kind) = row.plex_opt::<i32>("stream_type_id").and_then(stream_kind) else {
                continue;
            };
            batch.push(media_stream::ActiveModel {
                id: ActiveValue::Set(row.plex_get("id")?),
                media_part_id: ActiveValue::Set(part_id),
                kind: ActiveValue::Set(kind.to_owned()),
                position: ActiveValue::Set(row.plex_opt("index").unwrap_or(0)),
                codec: ActiveValue::Set(row.plex_text("codec")),
                language: ActiveValue::Set(row.plex_text("language")),
                channels: ActiveValue::Set(row.plex_opt("channels")),
                bitrate: ActiveValue::Set(row.plex_opt("bitrate")),
                is_default: ActiveValue::Set(row.plex_flag("default")),
                is_forced: ActiveValue::Set(row.plex_flag("forced")),
                // The source writes an empty string for an embedded track.
                external_path: ActiveValue::Set(row.plex_text("url")),
            });
        }

        for chunk in batch.chunks(WRITE_CHUNK) {
            media_stream::Entity::insert_many(chunk.to_vec())
                .exec(destination)
                .await?;
        }
        written += batch.len();
    }

    Ok(written)
}

/// Replaces the stored tags with the source's.
///
/// Every tag of the six kinds, referenced or not: 292,000 rows, against the cost of first
/// collecting which ones the imported items use. Deleting first is what makes a re-import
/// idempotent, since tags outlive the items that a re-import replaces; the attachments go
/// with them by cascade.
async fn import_tags(
    destination: &DatabaseConnection,
    source: &DatabaseConnection,
    source_path: &str,
) -> Result<HashSet<i64>, ImportError> {
    tag::Entity::delete_many().exec(destination).await?;

    let mut written = HashSet::new();
    let mut last_id = 0_i64;
    loop {
        // Keyset rather than `OFFSET`: an offset re-reads every row before it, which turns
        // a pass over a large table quadratic.
        let rows = source
            .query_all(Statement::from_string(
                sea_orm::DbBackend::Sqlite,
                format!(
                    "SELECT id, tag, tag_type, key, user_thumb_url FROM tags \
                     WHERE tag_type IN (1, 4, 5, 6, 7, 8) AND id > {last_id} \
                     ORDER BY id LIMIT {SOURCE_PAGE}"
                ),
            ))
            .await
            .map_err(|error| source_error(source_path, error))?;
        let Some(last) = rows.last() else {
            break;
        };
        last_id = last.plex_get("id")?;

        let mut batch = Vec::with_capacity(rows.len());
        for row in &rows {
            let Some(kind) = row.plex_opt::<i32>("tag_type").and_then(tag_kind) else {
                continue;
            };
            let Some(name) = row.plex_text("tag") else {
                continue;
            };
            let id: i64 = row.plex_get("id")?;
            batch.push(Tag {
                id: TagId::new(id),
                kind,
                name,
                provider_key: row.plex_text("key"),
                thumb_url: row.plex_text("user_thumb_url"),
            });
            written.insert(id);
        }
        write::insert_tags(destination, &batch).await?;
    }

    Ok(written)
}

/// Attaches tags to the imported items.
///
/// `text` is kept for actors only, where it is the character played. The source fills it
/// for directors, writers and producers too, and the reference publishes it for none of
/// them, so neither do we.
async fn import_item_tags(
    destination: &DatabaseConnection,
    source: &DatabaseConnection,
    source_path: &str,
    item_ids: &HashSet<i64>,
    tag_ids: &HashSet<i64>,
) -> Result<usize, ImportError> {
    let mut written = 0_usize;
    let mut last_id = 0_i64;
    loop {
        let rows = source
            .query_all(Statement::from_string(
                sea_orm::DbBackend::Sqlite,
                format!(
                    "SELECT tg.id, tg.metadata_item_id, tg.tag_id, tg.\"index\", tg.text, \
                     t.tag_type FROM taggings tg JOIN tags t ON t.id = tg.tag_id \
                     WHERE tg.id > {last_id} ORDER BY tg.id LIMIT {SOURCE_PAGE}"
                ),
            ))
            .await
            .map_err(|error| source_error(source_path, error))?;
        let Some(last) = rows.last() else {
            break;
        };
        last_id = last.plex_get("id")?;

        let mut batch = Vec::with_capacity(rows.len());
        for row in &rows {
            let (Some(item_id), Some(tag_id)) = (
                row.plex_opt::<i64>("metadata_item_id"),
                row.plex_opt::<i64>("tag_id"),
            ) else {
                continue;
            };
            // Attachments to kinds we do not import, on either side: extras and collections
            // among items, artwork and external ids among tags.
            if !item_ids.contains(&item_id) || !tag_ids.contains(&tag_id) {
                continue;
            }
            let is_actor =
                row.plex_opt::<i32>("tag_type").and_then(tag_kind) == Some(TagKind::Actor);
            batch.push(item_tag::ActiveModel {
                item_id: ActiveValue::Set(item_id),
                tag_id: ActiveValue::Set(tag_id),
                position: ActiveValue::Set(row.plex_opt("index").unwrap_or(0)),
                role: ActiveValue::Set(if is_actor {
                    row.plex_text("text")
                } else {
                    None
                }),
            });
        }

        for chunk in batch.chunks(WRITE_CHUNK) {
            item_tag::Entity::insert_many(chunk.to_vec())
                .exec(destination)
                .await?;
        }
        written += batch.len();
    }

    Ok(written)
}

/// Plex's `tag_type` discriminants for the kinds we import, identified by matching the
/// reference server's output element by element: 1 is `<Genre>`, 6 is `<Role>`, and so on.
/// The source uses some twenty others — artwork, external ids, reviews, ratings — which are
/// not tags in our model.
const fn tag_kind(tag_type: i32) -> Option<TagKind> {
    match tag_type {
        1 => Some(TagKind::Genre),
        4 => Some(TagKind::Director),
        5 => Some(TagKind::Writer),
        6 => Some(TagKind::Actor),
        7 => Some(TagKind::Producer),
        8 => Some(TagKind::Country),
        _ => None,
    }
}

/// Plex's `stream_type_id` discriminants. Anything else is a kind we do not model.
const fn stream_kind(stream_type: i32) -> Option<&'static str> {
    match stream_type {
        1 => Some("video"),
        2 => Some("audio"),
        3 => Some("subtitle"),
        _ => None,
    }
}

/// Formats an epoch second as the calendar date the API publishes.
fn to_date(epoch: i64) -> Option<String> {
    chrono::DateTime::from_timestamp(epoch, 0).map(|moment| moment.format("%Y-%m-%d").to_string())
}
