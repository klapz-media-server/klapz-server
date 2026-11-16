//! Importing the extras that come with items: trailers, deleted scenes, featurettes.
//!
//! The reference keeps an extra as a metadata item of `metadata_type` 12 outside any library,
//! tied to its item by a `metadata_relations` row, its kind packed in `extra_data`
//! (`ex:extraType`) and repeated as the relation's type. 30,260 in the reference library, of
//! which 30,253 are Internet Video Archive trailers its metadata provider supplies; each has a
//! version per bitrate with one part, whose address is packed in the part's `extra_data`
//! (`at:key`), and a video and an audio track.
//!
//! The kind is the item's own, which the reference publishes, and the relation's where the
//! item says none: they disagree on 2 of 30,190. 70 extras carry `-1`, a kind no client knows,
//! and are left out with those whose item was not imported.

use std::collections::{HashMap, HashSet};

use open_plex_core::edit::CalendarDate;
use open_plex_core::extra::ExtraKind;
use sea_orm::{ActiveValue, ConnectionTrait, DatabaseConnection, EntityTrait, Statement};

use super::analysis::Analysis;
use super::{source_error, ImportError, PlexRow, SOURCE_PAGE};
use crate::entity::{extra, extra_media, extra_track};
use crate::extra_store;

/// Rows per insert: at most nineteen columns, far inside every engine's parameter limit.
const WRITE_CHUNK: usize = 500;

#[derive(Debug, Default, Clone, Copy)]
pub struct ExtraSummary {
    pub extras: usize,
    pub versions: usize,
    pub tracks: usize,
    /// Extras of an item that was not imported, or of a kind no client knows.
    pub skipped: usize,
}

/// Copies the extras of imported items, with their versions and tracks.
///
/// # Errors
///
/// Fails if the source cannot be read or a write fails.
pub(crate) async fn import(
    destination: &DatabaseConnection,
    source: &DatabaseConnection,
    source_path: &str,
    item_ids: &HashSet<i64>,
) -> Result<ExtraSummary, ImportError> {
    let mut summary = ExtraSummary::default();
    let extras = import_extras(destination, source, source_path, item_ids, &mut summary).await?;
    let parts = import_versions(destination, source, source_path, &extras, &mut summary).await?;
    summary.tracks = import_tracks(destination, source, source_path, &parts).await?;
    Ok(summary)
}

async fn import_extras(
    destination: &DatabaseConnection,
    source: &DatabaseConnection,
    source_path: &str,
    item_ids: &HashSet<i64>,
    summary: &mut ExtraSummary,
) -> Result<HashSet<i64>, ImportError> {
    let mut written = HashSet::new();
    let mut last_id = 0_i64;
    loop {
        let rows = source
            .query_all(Statement::from_string(
                sea_orm::DbBackend::Sqlite,
                format!(
                    "SELECT x.id, x.guid, x.title, x.title_sort, x.summary, x.\"index\", x.year, \
                     x.originally_available_at, x.duration, x.added_at, x.updated_at, \
                     x.extra_data, r.metadata_item_id, r.relation_type \
                     FROM metadata_relations r \
                     JOIN metadata_items x ON x.id = r.related_metadata_item_id \
                     WHERE x.metadata_type = 12 AND x.id > {last_id} \
                     ORDER BY x.id, r.id LIMIT {SOURCE_PAGE}"
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
            let id: i64 = row.plex_get("id")?;
            let item: i64 = row.plex_get("metadata_item_id")?;
            let packed = Analysis::parse(row.plex_text("extra_data").as_deref());
            let kind = packed
                .int::<i32>("ex:extraType")
                .or_else(|| row.plex_opt("relation_type"))
                .and_then(kind_of);
            // An extra tied to two items keeps the first: none is, in the reference library.
            let (Some(kind), true, false) = (kind, item_ids.contains(&item), written.contains(&id))
            else {
                summary.skipped += 1;
                continue;
            };
            batch.push(extra::ActiveModel {
                id: ActiveValue::Set(id),
                item_id: ActiveValue::Set(item),
                kind: ActiveValue::Set(extra_store::kind_name(kind).to_owned()),
                position: ActiveValue::Set(row.plex_opt("index")),
                guid: ActiveValue::Set(row.plex_get("guid")?),
                title: ActiveValue::Set(row.plex_text("title")),
                title_sort: ActiveValue::Set(row.plex_text("title_sort")),
                summary: ActiveValue::Set(row.plex_text("summary")),
                year: ActiveValue::Set(row.plex_opt::<i32>("year").filter(|year| *year > 0)),
                released_on: ActiveValue::Set(
                    row.plex_opt::<i64>("originally_available_at")
                        .and_then(CalendarDate::containing)
                        .map(CalendarDate::midnight),
                ),
                duration_ms: ActiveValue::Set(row.plex_opt("duration")),
                added_at: ActiveValue::Set(row.plex_opt("added_at")),
                updated_at: ActiveValue::Set(row.plex_opt("updated_at")),
            });
            written.insert(id);
        }
        summary.extras += batch.len();
        for chunk in batch.chunks(WRITE_CHUNK) {
            extra::Entity::insert_many(chunk.to_vec())
                .exec(destination)
                .await?;
        }
    }
    Ok(written)
}

/// A part as the version that holds it needs it.
struct SourcePart {
    id: i64,
    path: Option<String>,
    size_bytes: Option<i64>,
    stream_url: Option<String>,
    optimized_for_streaming: Option<bool>,
}

/// Writes each version with its part, and answers the parts written, by version.
async fn import_versions(
    destination: &DatabaseConnection,
    source: &DatabaseConnection,
    source_path: &str,
    extras: &HashSet<i64>,
    summary: &mut ExtraSummary,
) -> Result<HashMap<i64, i64>, ImportError> {
    let mut versions = Vec::new();
    let mut last_id = 0_i64;
    loop {
        let rows = source
            .query_all(Statement::from_string(
                sea_orm::DbBackend::Sqlite,
                format!(
                    "SELECT m.id, m.metadata_item_id, m.width, m.height, \
                     m.display_aspect_ratio, m.duration, m.bitrate, m.container, \
                     m.video_codec, m.audio_codec, m.audio_channels, m.optimized_for_streaming \
                     FROM media_items m JOIN metadata_items x ON x.id = m.metadata_item_id \
                     WHERE x.metadata_type = 12 AND m.id > {last_id} \
                     ORDER BY m.id LIMIT {SOURCE_PAGE}"
                ),
            ))
            .await
            .map_err(|error| source_error(source_path, error))?;
        let Some(last) = rows.last() else {
            break;
        };
        last_id = last.plex_get("id")?;
        for row in rows {
            let extra: i64 = row.plex_get("metadata_item_id")?;
            if extras.contains(&extra) {
                versions.push(row);
            }
        }
    }

    let media_ids: HashSet<i64> = versions
        .iter()
        .map(|row| row.plex_get("id"))
        .collect::<Result<_, _>>()?;
    let mut parts = source_parts(source, source_path, &media_ids).await?;

    let mut written = HashMap::new();
    let mut batch = Vec::with_capacity(versions.len());
    for row in &versions {
        let id: i64 = row.plex_get("id")?;
        // A version with no part has nothing to play, and is left out as the reference's
        // listings leave a media without parts.
        let Some(part) = parts.remove(&id) else {
            continue;
        };
        written.insert(id, part.id);
        batch.push(extra_media::ActiveModel {
            id: ActiveValue::Set(id),
            extra_id: ActiveValue::Set(row.plex_get("metadata_item_id")?),
            part_id: ActiveValue::Set(part.id),
            width: ActiveValue::Set(row.plex_opt("width")),
            height: ActiveValue::Set(row.plex_opt("height")),
            aspect_ratio: ActiveValue::Set(row.plex_opt("display_aspect_ratio")),
            duration_ms: ActiveValue::Set(row.plex_opt("duration")),
            bitrate: ActiveValue::Set(row.plex_opt("bitrate")),
            container: ActiveValue::Set(row.plex_text("container")),
            video_codec: ActiveValue::Set(row.plex_text("video_codec")),
            audio_codec: ActiveValue::Set(row.plex_text("audio_codec")),
            audio_channels: ActiveValue::Set(row.plex_opt("audio_channels")),
            optimized_for_streaming: ActiveValue::Set(row.plex_flag("optimized_for_streaming")),
            path: ActiveValue::Set(part.path),
            size_bytes: ActiveValue::Set(part.size_bytes),
            stream_url: ActiveValue::Set(part.stream_url),
            file_optimized_for_streaming: ActiveValue::Set(part.optimized_for_streaming),
        });
    }
    summary.versions = batch.len();
    for chunk in batch.chunks(WRITE_CHUNK) {
        extra_media::Entity::insert_many(chunk.to_vec())
            .exec(destination)
            .await?;
    }
    Ok(written)
}

/// The first part of each of these versions: an extra's version has exactly one.
async fn source_parts(
    source: &DatabaseConnection,
    source_path: &str,
    media_ids: &HashSet<i64>,
) -> Result<HashMap<i64, SourcePart>, ImportError> {
    let mut parts: HashMap<i64, SourcePart> = HashMap::new();
    let mut last_id = 0_i64;
    loop {
        let rows = source
            .query_all(Statement::from_string(
                sea_orm::DbBackend::Sqlite,
                format!(
                    "SELECT p.id, p.media_item_id, p.file, p.size, p.extra_data \
                     FROM media_parts p JOIN media_items m ON m.id = p.media_item_id \
                     JOIN metadata_items x ON x.id = m.metadata_item_id \
                     WHERE x.metadata_type = 12 AND p.id > {last_id} \
                     ORDER BY p.id LIMIT {SOURCE_PAGE}"
                ),
            ))
            .await
            .map_err(|error| source_error(source_path, error))?;
        let Some(last) = rows.last() else {
            break;
        };
        last_id = last.plex_get("id")?;
        for row in &rows {
            let media: i64 = row.plex_get("media_item_id")?;
            if !media_ids.contains(&media) || parts.contains_key(&media) {
                continue;
            }
            let packed = Analysis::parse(row.plex_text("extra_data").as_deref());
            parts.insert(
                media,
                SourcePart {
                    id: row.plex_get("id")?,
                    path: row.plex_text("file"),
                    size_bytes: row.plex_opt("size"),
                    stream_url: packed.text("at:key"),
                    optimized_for_streaming: packed.tristate("ma:optimizedForStreaming"),
                },
            );
        }
    }
    Ok(parts)
}

async fn import_tracks(
    destination: &DatabaseConnection,
    source: &DatabaseConnection,
    source_path: &str,
    parts: &HashMap<i64, i64>,
) -> Result<usize, ImportError> {
    let version_of: HashMap<i64, i64> = parts
        .iter()
        .map(|(version, part)| (*part, *version))
        .collect();
    let mut written = 0;
    let mut last_id = 0_i64;
    loop {
        let rows = source
            .query_all(Statement::from_string(
                sea_orm::DbBackend::Sqlite,
                format!(
                    "SELECT s.id, s.media_part_id, s.stream_type_id, s.\"index\", s.codec, \
                     s.language, s.channels, s.bitrate, s.\"default\", s.extra_data \
                     FROM media_streams s JOIN media_items m ON m.id = s.media_item_id \
                     JOIN metadata_items x ON x.id = m.metadata_item_id \
                     WHERE x.metadata_type = 12 AND s.id > {last_id} \
                     ORDER BY s.id LIMIT {SOURCE_PAGE}"
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
            let Some(version) = row
                .plex_opt::<i64>("media_part_id")
                .and_then(|part| version_of.get(&part))
            else {
                continue;
            };
            let kind = match row.plex_opt::<i32>("stream_type_id") {
                Some(1) => "video",
                Some(2) => "audio",
                Some(3) => "subtitle",
                _ => continue,
            };
            let packed = Analysis::parse(row.plex_text("extra_data").as_deref());
            batch.push(extra_track::ActiveModel {
                id: ActiveValue::Set(row.plex_get("id")?),
                extra_media_id: ActiveValue::Set(*version),
                kind: ActiveValue::Set(kind.to_owned()),
                position: ActiveValue::Set(row.plex_opt("index").unwrap_or(0)),
                codec: ActiveValue::Set(row.plex_text("codec")),
                language: ActiveValue::Set(row.plex_text("language")),
                channels: ActiveValue::Set(row.plex_opt("channels")),
                bitrate: ActiveValue::Set(row.plex_opt("bitrate")),
                width: ActiveValue::Set(packed.int("ma:width")),
                height: ActiveValue::Set(packed.int("ma:height")),
                is_default: ActiveValue::Set(row.plex_flag("default")),
            });
        }
        written += batch.len();
        for chunk in batch.chunks(WRITE_CHUNK) {
            extra_track::Entity::insert_many(chunk.to_vec())
                .exec(destination)
                .await?;
        }
    }
    Ok(written)
}

/// The kind an `extraType` number names. `-1` and the rest name none a client knows.
const fn kind_of(number: i32) -> Option<ExtraKind> {
    Some(match number {
        1 => ExtraKind::Trailer,
        2 => ExtraKind::DeletedScene,
        3 => ExtraKind::Interview,
        4 => ExtraKind::MusicVideo,
        5 => ExtraKind::BehindTheScenes,
        6 => ExtraKind::Scene,
        7 => ExtraKind::LiveMusicVideo,
        8 => ExtraKind::LyricMusicVideo,
        9 => ExtraKind::Concert,
        10 => ExtraKind::Featurette,
        11 => ExtraKind::Short,
        12 => ExtraKind::Other,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_twelve_numbers_clients_know_name_the_twelve_kinds_in_order() {
        assert_eq!(kind_of(1), Some(ExtraKind::Trailer));
        assert_eq!(kind_of(5), Some(ExtraKind::BehindTheScenes));
        assert_eq!(kind_of(6), Some(ExtraKind::Scene));
        assert_eq!(kind_of(12), Some(ExtraKind::Other));
        assert_eq!(kind_of(13), None);
        assert_eq!(kind_of(0), None);
        assert_eq!(kind_of(-1), None);
    }

    #[test]
    fn the_kind_and_the_address_come_out_of_the_packed_columns() {
        let clip = Analysis::parse(Some(r#"{"ex:extraType":"5","url":"ex%3AextraType=5"}"#));
        assert_eq!(
            clip.int::<i32>("ex:extraType").and_then(kind_of),
            Some(ExtraKind::BehindTheScenes)
        );
        let part = Analysis::parse(Some(
            r#"{"at:key":"/services/iva/assets/200752/video.mp4?fmt=4&bitrate=5000","ma:container":"mp4"}"#,
        ));
        assert_eq!(
            part.text("at:key").as_deref(),
            Some("/services/iva/assets/200752/video.mp4?fmt=4&bitrate=5000")
        );
    }
}
