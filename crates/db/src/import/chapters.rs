//! Importing the chapters the reference read from files or got from its metadata agent.
//!
//! Plex keeps an item's chapters among its tag attachments: one `taggings` row per chapter, on a
//! tag of `tag_type` 9 whose name is the chapter's title — one tag for every chapter of that
//! title, the empty one included — `index` from 1, the span in `time_offset` and
//! `end_time_offset`, and `thumb_url` naming the version its image was drawn from. 519,141 in
//! the reference library, on 64,251 items.
//!
//! Where an item's chapters came from is read with the item, from `ma:chapterSource`.

use std::collections::HashSet;

use sea_orm::{ConnectionTrait, DatabaseConnection, EntityTrait, Statement};

use super::{source_error, ImportError, PlexRow, SOURCE_PAGE};
use crate::chapter_store;
use crate::entity::item_chapter;

const CHAPTER_TAG_TYPE: i32 = 9;

/// Rows per insert: seven columns, far inside every engine's parameter limit.
const WRITE_CHUNK: usize = 500;

#[derive(Debug, Default, Clone, Copy)]
pub struct ChapterSummary {
    pub chapters: usize,
    /// Chapters of an item that was not imported.
    pub skipped: usize,
}

/// Copies the chapters of the imported items. Their rows go with the items an import replaces.
///
/// # Errors
///
/// Fails if the source cannot be read or a write fails.
pub(crate) async fn import(
    destination: &DatabaseConnection,
    source: &DatabaseConnection,
    source_path: &str,
    item_ids: &HashSet<i64>,
    media_ids: &HashSet<i64>,
) -> Result<ChapterSummary, ImportError> {
    let mut summary = ChapterSummary::default();
    let mut last_id = 0_i64;
    loop {
        let rows = source
            .query_all(Statement::from_string(
                sea_orm::DbBackend::Sqlite,
                format!(
                    "SELECT taggings.id, taggings.metadata_item_id, taggings.\"index\", \
                     taggings.time_offset, taggings.end_time_offset, taggings.thumb_url, \
                     tags.tag \
                     FROM taggings JOIN tags ON tags.id = taggings.tag_id \
                     WHERE tags.tag_type = {CHAPTER_TAG_TYPE} AND taggings.id > {last_id} \
                     ORDER BY taggings.id LIMIT {SOURCE_PAGE}"
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
            let item: i64 = row.plex_get("metadata_item_id")?;
            if !item_ids.contains(&item) {
                summary.skipped += 1;
                continue;
            }
            let image = row
                .plex_text("thumb_url")
                .as_deref()
                .and_then(image_media)
                .filter(|media| media_ids.contains(media));
            batch.push(chapter_store::row(
                item,
                row.plex_get("index")?,
                row.plex_opt("time_offset").unwrap_or(0),
                row.plex_opt("end_time_offset").unwrap_or(0),
                row.plex_text("tag"),
                image,
            ));
        }
        summary.chapters += batch.len();
        for chunk in batch.chunks(WRITE_CHUNK) {
            item_chapter::Entity::insert_many(chunk.to_vec())
                .exec(destination)
                .await?;
        }
    }
    Ok(summary)
}

/// The version a chapter image's address names: `/library/media/{media}/chapterImages/{index}`.
fn image_media(url: &str) -> Option<i64> {
    let (media, rest) = url.strip_prefix("/library/media/")?.split_once('/')?;
    rest.starts_with("chapterImages/")
        .then(|| media.parse().ok())
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_version_a_chapter_image_is_drawn_from() {
        assert_eq!(
            image_media("/library/media/159103/chapterImages/1"),
            Some(159_103)
        );
        assert_eq!(image_media("/library/media/159103/thumb/1"), None);
        assert_eq!(
            image_media("/library/metadata/159103/chapterImages/1"),
            None
        );
        assert_eq!(image_media(""), None);
    }
}
