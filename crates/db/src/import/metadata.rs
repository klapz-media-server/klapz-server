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
//! Tags, people, external identifiers and ratings come next, see [`super::tags`]; then
//! markers, see [`super::markers`]. Collections come next, since their members are items;
//! see [`super::collections`]. Extras come last, see [`super::extras`].

use std::collections::HashSet;

use klapz_core::chapter::ChapterSource;
use klapz_core::edit::{CalendarDate, ItemField};
use klapz_core::metadata::{self, ItemKind};
use sea_orm::{ActiveValue, ConnectionTrait, DatabaseConnection, EntityTrait, Statement};

use super::analysis::Analysis;
use super::{
    chapters, collections, extras, markers, source_error, tags, ImportError, PlexRow, SOURCE_PAGE,
};
use crate::entity::{
    item_corner_colors, media, media_part, media_stream, metadata_item, stream_dolby_vision,
    stream_loudness,
};
use crate::language::canonical;
use crate::write;

/// How many rows to write per statement.
///
/// Chosen for the narrowest backend: `SQLite` binds one parameter per column per row, and
/// `metadata_item` has 23 columns, so 500 rows is 11,500 parameters — comfortably inside
/// every engine's limit.
const WRITE_CHUNK: usize = 500;

/// [`WRITE_CHUNK`] for tracks, whose 47 columns would make 500 rows 23,500 parameters:
/// inside `SQLite`'s 32,766, but not comfortably.
const STREAM_WRITE_CHUNK: usize = 250;

/// Plex's `metadata_type` values we import, in the order their foreign keys require.
const KINDS: [(i32, &str); 4] = [(1, "movie"), (2, "show"), (3, "season"), (4, "episode")];

#[derive(Debug, Default, Clone, Copy)]
pub struct MetadataSummary {
    pub items: usize,
    /// Rows written to the title word index, which the search filter reads.
    pub title_words: usize,
    /// Fields locked against a metadata refresh, one per item and field.
    pub locked_fields: usize,
    /// Items with the four colors of their backdrop.
    pub corner_colors: usize,
    pub media: usize,
    pub files: usize,
    pub streams: usize,
    pub tags: tags::TagSummary,
    pub markers: markers::MarkerSummary,
    pub chapters: chapters::ChapterSummary,
    pub collections: collections::CollectionSummary,
    pub extras: extras::ExtraSummary,
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
        let imported = import_items(
            destination,
            source,
            source_path,
            metadata_type,
            kind,
            &mut item_ids,
        )
        .await?;
        summary.items += imported.written;
        summary.orphans += imported.orphans;
        summary.title_words += imported.words;
        summary.locked_fields += imported.locks;
        summary.corner_colors += imported.corner_colors;
        tracing::info!(
            kind,
            written = imported.written,
            orphans = imported.orphans,
            words = imported.words,
            locks = imported.locks,
            corner_colors = imported.corner_colors,
            "imported items"
        );
    }

    let media_ids = import_media(destination, source, source_path, &item_ids).await?;
    summary.media = media_ids.len();
    tracing::info!(count = summary.media, "imported media");

    let part_ids = import_parts(destination, source, source_path, &media_ids).await?;
    summary.files = part_ids.len();
    tracing::info!(count = summary.files, "imported files");

    summary.streams = import_streams(destination, source, source_path, &part_ids).await?;
    tracing::info!(count = summary.streams, "imported streams");

    summary.tags = tags::import(destination, source, source_path, &item_ids).await?;
    tracing::info!(
        people = summary.tags.people,
        tags = summary.tags.tags,
        item_tags = summary.tags.item_tags,
        external_ids = summary.tags.external_ids,
        ratings = summary.tags.ratings,
        "imported tags"
    );

    summary.markers = markers::import(destination, source, source_path, &item_ids).await?;
    tracing::info!(
        markers = summary.markers.markers,
        skipped = summary.markers.skipped,
        "imported markers"
    );
    summary.chapters =
        chapters::import(destination, source, source_path, &item_ids, &media_ids).await?;
    tracing::info!(
        chapters = summary.chapters.chapters,
        skipped = summary.chapters.skipped,
        "imported chapters"
    );
    summary.collections = collections::import(destination, source, source_path, &item_ids).await?;
    tracing::info!(
        collections = summary.collections.collections,
        entries = summary.collections.entries,
        smart_skipped = summary.collections.smart_skipped,
        orphan_memberships = summary.collections.orphan_memberships,
        "imported collections"
    );
    summary.extras = extras::import(destination, source, source_path, &item_ids).await?;
    tracing::info!(
        extras = summary.extras.extras,
        versions = summary.extras.versions,
        tracks = summary.extras.tracks,
        skipped = summary.extras.skipped,
        "imported extras"
    );

    Ok(summary)
}

/// What one kind's import wrote.
struct ImportedItems {
    written: usize,
    orphans: usize,
    /// Rows the title index gained.
    words: usize,
    locks: usize,
    corner_colors: usize,
}

/// Imports one kind of item, with its searchable words, its locks and its backdrop colors.
async fn import_items(
    destination: &DatabaseConnection,
    source: &DatabaseConnection,
    source_path: &str,
    metadata_type: i32,
    kind: &str,
    item_ids: &mut HashSet<i64>,
) -> Result<ImportedItems, ImportError> {
    let needs_parent = metadata_type == 3 || metadata_type == 4;
    let mut offset = 0_u64;
    let mut imported = ImportedItems {
        written: 0,
        orphans: 0,
        words: 0,
        locks: 0,
        corner_colors: 0,
    };

    loop {
        let rows = source
            .query_all(Statement::from_string(
                sea_orm::DbBackend::Sqlite,
                format!(
                    "SELECT id, library_section_id, parent_id, guid, title, title_sort, \
                     original_title, summary, tagline, studio, content_rating, \
                     content_rating_age, \"index\", year, originally_available_at, duration, \
                     rating, audience_rating, added_at, updated_at, refreshed_at, user_fields, extra_data \
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
        let mut locks = Vec::new();
        let mut colors = Vec::new();
        for row in &rows {
            let id: i64 = row.plex_get("id")?;
            let parent_id: Option<i64> = row.plex_opt("parent_id");

            // A season or episode whose parent was not imported cannot satisfy the foreign
            // key — whether the parent is corrupt in the source or was itself skipped.
            if needs_parent && !parent_id.is_some_and(|parent| item_ids.contains(&parent)) {
                tracing::warn!(id, kind, ?parent_id, "skipping item with an absent parent");
                imported.orphans += 1;
                continue;
            }

            // Empty means absent, and storing it as such avoids the empty-string sentinel
            // we reject in the source schema.
            let title = row.plex_text("title");
            let title_sort = row.plex_text("title_sort");
            let original_title = row.plex_text("original_title");
            let sort_key = metadata::sort_key(title_sort.as_deref(), title.as_deref());
            if let Some(user_fields) = row.plex_text("user_fields") {
                locks.extend(
                    locked_fields(&user_fields, metadata_type)
                        .into_iter()
                        .map(|field| (id, field)),
                );
            }
            let extra = Analysis::parse(row.plex_text("extra_data").as_deref());
            colors.extend(corner_colors(&extra).map(|corners| corner_colors_row(id, corners)));

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
                // The source writes -1 for 39 episodes whose year it does not know, and the
                // reference publishes no year for them.
                year: ActiveValue::Set(row.plex_opt::<i32>("year").filter(|year| *year > 0)),
                // A date stored as a time in the source: kept as its day's midnight, UTC.
                released_on: ActiveValue::Set(
                    row.plex_opt::<i64>("originally_available_at")
                        .and_then(CalendarDate::containing)
                        .map(CalendarDate::midnight),
                ),
                duration_ms: ActiveValue::Set(row.plex_opt("duration")),
                rating: ActiveValue::Set(row.plex_opt("rating")),
                audience_rating: ActiveValue::Set(row.plex_opt("audience_rating")),
                added_at: ActiveValue::Set(row.plex_opt("added_at").unwrap_or(0)),
                updated_at: ActiveValue::Set(row.plex_opt("updated_at").unwrap_or(0)),
                refreshed_at: ActiveValue::Set(row.plex_opt("refreshed_at")),
                chapter_source: ActiveValue::Set(chapter_source(&extra)),
            });
            item_ids.insert(id);
        }

        imported.written += batch.len();
        // The one write path, which derives the title-word index from the models themselves.
        imported.words += write::insert_items(destination, batch).await?;
        imported.locks += locks.len();
        write::insert_locks(destination, locks).await?;
        imported.corner_colors += colors.len();
        for chunk in colors.chunks(WRITE_CHUNK) {
            item_corner_colors::Entity::insert_many(chunk.to_vec())
                .exec(destination)
                .await?;
        }
    }

    Ok(imported)
}

/// Where an item's chapters came from, when the source names an origin we know.
fn chapter_source(extra: &Analysis) -> Option<String> {
    extra
        .text("ma:chapterSource")
        .filter(|source| ChapterSource::parse(source).is_some())
}

fn corner_colors_row(
    item: i64,
    [top_left, top_right, bottom_right, bottom_left]: [i32; 4],
) -> item_corner_colors::ActiveModel {
    item_corner_colors::ActiveModel {
        item_id: ActiveValue::Set(item),
        top_left: ActiveValue::Set(top_left),
        top_right: ActiveValue::Set(top_right),
        bottom_right: ActiveValue::Set(bottom_right),
        bottom_left: ActiveValue::Set(bottom_left),
    }
}

/// The backdrop colors the reference's `UltraBlurProcessor` packs into `extra_data`, top left
/// first and clockwise, as `0xRRGGBB`: all four, or none when one is missing or is not six
/// hex digits.
fn corner_colors(extra: &Analysis) -> Option<[i32; 4]> {
    let mut corners = [0; 4];
    for (corner, key) in corners.iter_mut().zip([
        "pv:ultraBlurTopLeft",
        "pv:ultraBlurTopRight",
        "pv:ultraBlurBottomRight",
        "pv:ultraBlurBottomLeft",
    ]) {
        let hex = extra.text(key)?;
        if hex.len() != 6 || !hex.bytes().all(|digit| digit.is_ascii_hexdigit()) {
            return None;
        }
        *corner = i32::from_str_radix(&hex, 16).ok()?;
    }
    Some(corners)
}

/// The fields an item's `user_fields` locks, `lockedFields=1|2|9` among its `&`-separated
/// pairs, that items of its kind hold.
///
/// Numbers we have no field for are dropped: Plex numbers fields we do not model, such as
/// its banner or theme music.
fn locked_fields(user_fields: &str, metadata_type: i32) -> Vec<ItemField> {
    let kind = match metadata_type {
        1 => ItemKind::Movie,
        2 => ItemKind::Show,
        3 => ItemKind::Season,
        _ => ItemKind::Episode,
    };
    let mut fields: Vec<ItemField> = user_fields
        .split('&')
        .filter_map(|pair| pair.strip_prefix("lockedFields="))
        .flat_map(|numbers| numbers.split('|'))
        .filter_map(|number| plex_field(number.parse().ok()?))
        .filter(|field| field.applies_to(kind))
        .collect();
    fields.sort();
    fields.dedup();
    fields
}

/// Plex's number for a lockable field, as `PUT /library/sections/{id}/all` stores them: each
/// locked alone on a test film of the reference, and read back from its database.
const fn plex_field(number: u32) -> Option<ItemField> {
    Some(match number {
        1 => ItemField::Title,
        2 => ItemField::TitleSort,
        3 => ItemField::OriginalTitle,
        4 => ItemField::Studio,
        5 => ItemField::Rating,
        6 => ItemField::Tagline,
        7 => ItemField::Summary,
        8 => ItemField::ContentRating,
        9 => ItemField::Poster,
        10 => ItemField::Background,
        13 => ItemField::ReleasedOn,
        14 => ItemField::Year,
        15 => ItemField::Genres,
        16 => ItemField::Collections,
        17 => ItemField::Directors,
        18 => ItemField::Writers,
        19 => ItemField::Cast,
        23 => ItemField::Producers,
        24 => ItemField::Countries,
        30 => ItemField::AudienceRating,
        _ => return None,
    })
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
                     updated_at, hash, open_subtitle_hash, extra_data FROM media_parts ORDER BY id \
                     LIMIT {SOURCE_PAGE} OFFSET {offset}"
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
            let analysis = Analysis::parse(row.plex_text("extra_data").as_deref());
            batch.push(media_part::ActiveModel {
                id: ActiveValue::Set(id),
                media_id: ActiveValue::Set(media_id),
                position: ActiveValue::Set(row.plex_opt("index").unwrap_or(0)),
                path: ActiveValue::Set(path),
                size_bytes: ActiveValue::Set(row.plex_opt("size")),
                duration_ms: ActiveValue::Set(row.plex_opt("duration")),
                created_at: ActiveValue::Set(row.plex_opt("created_at").unwrap_or(0)),
                updated_at: ActiveValue::Set(row.plex_opt("updated_at").unwrap_or(0)),
                has_thumbnail: ActiveValue::Set(analysis.flag("ma:hasThumbnail")),
                has_64bit_offsets: ActiveValue::Set(analysis.tristate("ma:has64bitOffsets")),
                optimized_for_streaming: ActiveValue::Set(
                    analysis.tristate("ma:optimizedForStreaming"),
                ),
                packet_length: ActiveValue::Set(analysis.int("ma:packetLength")),
                has_chapter_text_stream: ActiveValue::Set(analysis.flag("ma:hasChapterTextStream")),
                content_hash: ActiveValue::Set(row.plex_text("hash")),
                open_subtitles_hash: ActiveValue::Set(row.plex_text("open_subtitle_hash")),
                // What ffprobe states as printed, which the source's analysis does not keep.
                ..Default::default()
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
                     channels, bitrate, \"default\", forced, url, extra_data \
                     FROM media_streams ORDER BY id LIMIT {SOURCE_PAGE} OFFSET {offset}"
                ),
            ))
            .await
            .map_err(|error| source_error(source_path, error))?;

        if rows.is_empty() {
            break;
        }
        offset += rows.len() as u64;

        let mut batch = Vec::with_capacity(rows.len());
        let mut dolby_vision = Vec::new();
        let mut loudness = Vec::new();
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
            let id: i64 = row.plex_get("id")?;
            let analysis = Analysis::parse(row.plex_text("extra_data").as_deref());
            dolby_vision.extend(dolby_vision_of(id, &analysis));
            loudness.extend(loudness_of(id, &analysis));
            batch.push(stream_row(id, part_id, kind, row, &analysis));
        }

        for chunk in batch.chunks(STREAM_WRITE_CHUNK) {
            media_stream::Entity::insert_many(chunk.to_vec())
                .exec(destination)
                .await?;
        }
        for chunk in dolby_vision.chunks(WRITE_CHUNK) {
            stream_dolby_vision::Entity::insert_many(chunk.to_vec())
                .exec(destination)
                .await?;
        }
        for chunk in loudness.chunks(WRITE_CHUNK) {
            stream_loudness::Entity::insert_many(chunk.to_vec())
                .exec(destination)
                .await?;
        }
        written += batch.len();
    }

    Ok(written)
}

/// One track, from its source row and the analysis packed into it.
fn stream_row(
    id: i64,
    part: i64,
    kind: &str,
    row: &sea_orm::QueryResult,
    analysis: &Analysis,
) -> media_stream::ActiveModel {
    media_stream::ActiveModel {
        id: ActiveValue::Set(id),
        media_part_id: ActiveValue::Set(part),
        kind: ActiveValue::Set(kind.to_owned()),
        position: ActiveValue::Set(row.plex_opt("index").unwrap_or(0)),
        codec: ActiveValue::Set(row.plex_text("codec")),
        language: ActiveValue::Set(canonical(row.plex_text("language").as_deref())),
        channels: ActiveValue::Set(row.plex_opt("channels")),
        bitrate: ActiveValue::Set(row.plex_opt("bitrate")),
        is_default: ActiveValue::Set(row.plex_flag("default")),
        is_forced: ActiveValue::Set(row.plex_flag("forced")),
        // The source writes an empty string for an embedded track.
        external_path: ActiveValue::Set(row.plex_text("url")),
        title: ActiveValue::Set(analysis.text_as_written("ma:title")),
        stream_identifier: ActiveValue::Set(analysis.int("ma:streamIdentifier")),
        profile: ActiveValue::Set(analysis.text("ma:profile")),
        bit_depth: ActiveValue::Set(analysis.int("ma:bitDepth")),
        width: ActiveValue::Set(analysis.int("ma:width")),
        height: ActiveValue::Set(analysis.int("ma:height")),
        coded_width: ActiveValue::Set(analysis.int("ma:codedWidth")),
        coded_height: ActiveValue::Set(analysis.int("ma:codedHeight")),
        frame_rate: ActiveValue::Set(analysis.float("ma:frameRate")),
        level: ActiveValue::Set(analysis.int("ma:level")),
        ref_frames: ActiveValue::Set(analysis.int("ma:refFrames")),
        scan_type: ActiveValue::Set(analysis.text("ma:scanType")),
        chroma_subsampling: ActiveValue::Set(analysis.text("ma:chromaSubsampling")),
        chroma_location: ActiveValue::Set(analysis.text("ma:chromaLocation")),
        color_range: ActiveValue::Set(analysis.text("ma:colorRange")),
        color_space: ActiveValue::Set(analysis.text("ma:colorSpace")),
        color_primaries: ActiveValue::Set(analysis.text("ma:colorPrimaries")),
        color_transfer: ActiveValue::Set(analysis.text("ma:colorTrc")),
        has_scaling_matrix: ActiveValue::Set(analysis.tristate("ma:hasScalingMatrix")),
        is_anamorphic: ActiveValue::Set(analysis.tristate("ma:anamorphic")),
        pixel_aspect_ratio: ActiveValue::Set(analysis.text("ma:pixelAspectRatio")),
        fourcc: ActiveValue::Set(analysis.text("ma:codecID")),
        has_closed_captions: ActiveValue::Set(analysis.flag("ma:closedCaptions")),
        has_separate_fields: ActiveValue::Set(analysis.flag("ma:separateFields")),
        has_hdr10_plus: ActiveValue::Set(analysis.flag("ma:HDR10PlusPresent")),
        sampling_rate: ActiveValue::Set(analysis.int("ma:samplingRate")),
        channel_layout: ActiveValue::Set(analysis.text("ma:audioChannelLayout")),
        subtitle_format: ActiveValue::Set(analysis.text("ma:format")),
        is_embedded_in_video: ActiveValue::Set(analysis.flag("ma:embeddedInVideo")),
        is_original: ActiveValue::Set(analysis.flag("ma:original")),
        is_dub: ActiveValue::Set(analysis.flag("ma:dub")),
        is_commentary: ActiveValue::Set(analysis.flag("ma:comment")),
        is_hearing_impaired: ActiveValue::Set(analysis.flag("ma:hearingImpaired")),
        is_visual_impaired: ActiveValue::Set(analysis.flag("ma:visualImpaired")),
        has_header_compression: ActiveValue::Set(analysis.flag("ma:headerCompression")),
        required_bandwidths: ActiveValue::Set(analysis.text("ma:requiredBandwidths")),
        // What ffprobe states as printed, which the source's analysis does not keep.
        ..Default::default()
    }
}

/// A track's Dolby Vision record, when the analysis found one.
///
/// Keyed on `DOVIPresent`, which the source writes on every Dolby Vision track; the profile
/// and level are part of the configuration record, so a track without them is skipped.
fn dolby_vision_of(stream: i64, analysis: &Analysis) -> Option<stream_dolby_vision::ActiveModel> {
    if !analysis.flag("ma:DOVIPresent") {
        return None;
    }
    Some(stream_dolby_vision::ActiveModel {
        stream_id: ActiveValue::Set(stream),
        version: ActiveValue::Set(analysis.text("ma:DOVIVersion")?),
        profile: ActiveValue::Set(analysis.int("ma:DOVIProfile")?),
        level: ActiveValue::Set(analysis.int("ma:DOVILevel")?),
        base_layer_compatibility: ActiveValue::Set(analysis.int("ma:DOVIBLCompatID")),
        has_base_layer: ActiveValue::Set(analysis.flag("ma:DOVIBLPresent")),
        has_enhancement_layer: ActiveValue::Set(analysis.flag("ma:DOVIELPresent")),
        has_rpu: ActiveValue::Set(analysis.flag("ma:DOVIRPUPresent")),
    })
}

/// A track's loudness measurement, when every one of its values is there.
fn loudness_of(stream: i64, analysis: &Analysis) -> Option<stream_loudness::ActiveModel> {
    Some(stream_loudness::ActiveModel {
        stream_id: ActiveValue::Set(stream),
        analysis_version: ActiveValue::Set(analysis.text("ln:loudnessAnalysisVersion")?),
        integrated: ActiveValue::Set(analysis.float("ln:loudness")?),
        loudness_range: ActiveValue::Set(analysis.float("ln:lra")?),
        true_peak: ActiveValue::Set(analysis.float("ln:peak")?),
        threshold: ActiveValue::Set(analysis.float("ln:threshold")?),
        gain_offset: ActiveValue::Set(analysis.float("ln:gainOffset")?),
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_four_backdrop_colors_or_none() {
        let packed = |extra: &str| corner_colors(&Analysis::parse(Some(extra)));
        // Item 21812 of the reference, the hex case aside.
        assert_eq!(
            packed(
                r#"{"pv:ultraBlurBottomLeft":"8f461d","pv:ultraBlurBottomRight":"58421c","pv:ultraBlurChangestamp":"189731385","pv:ultraBlurTopLeft":"1a2c53","pv:ultraBlurTopRight":"81502B"}"#
            ),
            Some([0x001a_2c53, 0x0081_502b, 0x0058_421c, 0x008f_461d])
        );
        assert_eq!(
            packed(
                r#"{"pv:ultraBlurTopLeft":"1a2c53","pv:ultraBlurTopRight":"81502b","pv:ultraBlurBottomRight":"58421c"}"#
            ),
            None
        );
        assert_eq!(
            packed(
                r#"{"pv:ultraBlurBottomLeft":"+8f461","pv:ultraBlurBottomRight":"58421c","pv:ultraBlurTopLeft":"1a2c53","pv:ultraBlurTopRight":"81502b"}"#
            ),
            None
        );
        assert_eq!(packed("{}"), None);
    }

    #[test]
    fn reads_the_locks_plex_packs_into_user_fields() {
        // Item 429849 of the reference, published with Field elements for title, titleSort,
        // originalTitle, studio, thumb, originallyAvailableAt, year and writer.
        assert_eq!(
            locked_fields("lockedFields=1|2|3|4|9|13|14|18", 1),
            vec![
                ItemField::Title,
                ItemField::TitleSort,
                ItemField::OriginalTitle,
                ItemField::Studio,
                ItemField::Poster,
                ItemField::ReleasedOn,
                ItemField::Year,
                ItemField::Writers,
            ]
        );
        assert_eq!(locked_fields("lockedFields=", 1), Vec::new());
        // Item 550296: a clear logo and a square art among them, which we do not model.
        assert_eq!(
            locked_fields("lockedFields=9|10|36|39", 1),
            vec![ItemField::Poster, ItemField::Background]
        );
    }

    #[test]
    fn keeps_only_the_locks_a_kind_holds() {
        assert_eq!(
            locked_fields("lockedFields=1|4|15", 4),
            vec![ItemField::Title]
        );
    }
}
