//! Importing the intro and credits markers the reference's analysis found.
//!
//! Plex keeps them among an item's tag attachments: one `taggings` row per marker, attached to
//! the single tag of `tag_type` 12, the kind in `text`, the span in `time_offset` and
//! `end_time_offset`, and the rest packed in `extra_data` — `pv:final` on the credits that run
//! to the end, `pv:version` for the detection that found it. 217,863 in the reference library:
//! 80,228 intros and 137,635 credits, on films and episodes.
//!
//! Read in source order, which within an item is the order of `index`, so the ids given here
//! keep the order the reference lists them in. A viewer's bookmarks are not imported: the
//! reference keeps them per account, and no account is ever read.

use std::collections::HashSet;

use open_plex_core::marker::MarkerKind;
use sea_orm::{ConnectionTrait, DatabaseConnection, EntityTrait, Statement};

use super::analysis::Analysis;
use super::{source_error, ImportError, PlexRow, SOURCE_PAGE};
use crate::entity::item_marker;
use crate::marker_store;

const MARKER_TAG_TYPE: i32 = 12;

/// Rows per insert: eleven columns, far inside every engine's parameter limit.
const WRITE_CHUNK: usize = 500;

#[derive(Debug, Default, Clone, Copy)]
pub struct MarkerSummary {
    pub markers: usize,
    /// Markers of an item that was not imported, or of a kind we do not know.
    pub skipped: usize,
}

/// Replaces the work's markers with the source's, leaving the viewers' bookmarks.
///
/// # Errors
///
/// Fails if the source cannot be read or a write fails.
pub(crate) async fn import(
    destination: &DatabaseConnection,
    source: &DatabaseConnection,
    source_path: &str,
    item_ids: &HashSet<i64>,
) -> Result<MarkerSummary, ImportError> {
    marker_store::delete_work_markers(destination).await?;

    let mut summary = MarkerSummary::default();
    let mut last_id = 0_i64;
    loop {
        let rows = source
            .query_all(Statement::from_string(
                sea_orm::DbBackend::Sqlite,
                format!(
                    "SELECT id, metadata_item_id, text, time_offset, end_time_offset, extra_data \
                     FROM taggings WHERE tag_id IN \
                     (SELECT id FROM tags WHERE tag_type = {MARKER_TAG_TYPE}) \
                     AND id > {last_id} ORDER BY id LIMIT {SOURCE_PAGE}"
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
            let kind = row.plex_text("text").as_deref().and_then(kind_of);
            let (Some(kind), true) = (kind, item_ids.contains(&item)) else {
                summary.skipped += 1;
                continue;
            };
            let extra = Analysis::parse(row.plex_text("extra_data").as_deref());
            batch.push(marker_store::work_marker(
                item,
                kind,
                row.plex_get("time_offset")?,
                row.plex_get("end_time_offset")?,
                extra.flag("pv:final"),
                extra.int("pv:version"),
            ));
        }
        summary.markers += batch.len();
        for chunk in batch.chunks(WRITE_CHUNK) {
            item_marker::Entity::insert_many(chunk.to_vec())
                .exec(destination)
                .await?;
        }
    }
    Ok(summary)
}

/// The kind a marker's `text` names. `commercial` is what the reference's advertisement
/// detection writes; the reference library holds none.
fn kind_of(text: &str) -> Option<MarkerKind> {
    match text {
        "intro" => Some(MarkerKind::Intro),
        "credits" => Some(MarkerKind::Credits),
        "commercial" => Some(MarkerKind::Commercial),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_three_kinds_the_analysis_writes_and_nothing_else() {
        assert_eq!(kind_of("intro"), Some(MarkerKind::Intro));
        assert_eq!(kind_of("credits"), Some(MarkerKind::Credits));
        assert_eq!(kind_of("commercial"), Some(MarkerKind::Commercial));
        assert_eq!(kind_of("bookmark"), None);
        assert_eq!(kind_of("Intro"), None);
    }

    #[test]
    fn final_and_version_come_out_of_the_packed_column() {
        let credits = Analysis::parse(Some(
            r#"{"pv:final":"1","pv:version":"4","url":"pv%3Afinal=1&pv%3Aversion=4"}"#,
        ));
        assert!(credits.flag("pv:final"));
        assert_eq!(credits.int::<i32>("pv:version"), Some(4));

        let intro = Analysis::parse(Some(r#"{"pv:version":"5","url":"pv%3Aversion=5"}"#));
        assert!(!intro.flag("pv:final"));
        assert_eq!(intro.int::<i32>("pv:version"), Some(5));
    }
}
