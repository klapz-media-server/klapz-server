//! Importing collections from a Plex database.
//!
//! Plex keeps a collection twice over: as an item of type 18 in its library, holding the
//! title, summary and settings, and as a tag (`tag_type` 2) its members carry, numbered by
//! the item's `index`. One tag serves every library that has a collection of that name, so a
//! membership is a tagging of an item whose library holds a collection under that number.
//! We keep the collection, and its members as entries of it.
//!
//! Its settings are packed into `extra_data` (`pr:collectionMode`, `pr:collectionSort`) and
//! its locks into `user_fields`, as an item's are; both are unpacked into columns here.
//!
//! Smart collections are not imported: the reference's four all list `episode.duplicate`, a
//! filter the official API cannot express, and a rule that cannot be listed would be a
//! collection that pretends to be empty.

use std::collections::{HashMap, HashSet};

use klapz_core::collection::{Collection, CollectionField, CollectionMode, CollectionOrder};
use klapz_core::metadata::ItemKind;
use klapz_core::playlist::POSITION_GAP;
use klapz_core::{CollectionId, RatingKey, SectionId};
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement, TransactionTrait};

use super::{source_error, ImportError, PlexRow, SOURCE_PAGE};
use crate::collection_repository::{insert_placed, to_row};
use crate::write;

#[derive(Debug, Default, Clone, Copy)]
pub struct CollectionSummary {
    pub collections: usize,
    pub entries: usize,
    /// Smart collections, left out; see the module documentation.
    pub smart_skipped: usize,
    /// Memberships of a tag no collection of the item's library carries.
    pub orphan_memberships: usize,
}

/// Copies every collection of the imported libraries, and its members among the imported
/// items.
///
/// # Errors
///
/// Fails if the source cannot be read or a write fails.
pub async fn import<S: std::hash::BuildHasher>(
    destination: &DatabaseConnection,
    source: &DatabaseConnection,
    source_path: &str,
    item_ids: &HashSet<i64, S>,
) -> Result<CollectionSummary, ImportError> {
    let mut summary = CollectionSummary::default();
    let artwork = artwork(source, source_path).await?;
    let rows = source
        .query_all(Statement::from_string(
            sea_orm::DbBackend::Sqlite,
            "SELECT c.id, c.library_section_id, c.guid, c.title, c.title_sort, c.summary, \
             c.content_rating, c.\"index\", c.rating_count, c.extra_data, c.user_fields, \
             c.added_at, c.updated_at, s.section_type \
             FROM metadata_items c JOIN library_sections s ON s.id = c.library_section_id \
             WHERE c.metadata_type = 18 ORDER BY c.id"
                .to_owned(),
        ))
        .await
        .map_err(|error| source_error(source_path, error))?;

    let mut by_tag: HashMap<(i64, i64), CollectionId> = HashMap::new();
    let transaction = destination.begin().await?;
    for row in &rows {
        let extra: serde_json::Value = row
            .plex_text("extra_data")
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        let setting = |key: &str| extra.get(key).and_then(serde_json::Value::as_str);
        if setting("at:smart") == Some("1") {
            summary.smart_skipped += 1;
            continue;
        }
        let Some(title) = row.plex_text("title") else {
            continue;
        };
        let section: i64 = row.plex_get("library_section_id")?;
        let tag_number: i64 = row.plex_get("index")?;
        let (thumb_url, art_url) = artwork.get(&tag_number).cloned().unwrap_or_default();
        let collection = Collection {
            id: CollectionId::new(row.plex_get("id")?),
            section: SectionId::new(section),
            kind: kind_of(
                row.plex_opt("rating_count").unwrap_or(0),
                row.plex_opt("section_type").unwrap_or(1),
            ),
            guid: row.plex_get("guid")?,
            tag_number,
            title,
            title_sort: row.plex_text("title_sort"),
            summary: row.plex_text("summary").unwrap_or_default(),
            content_rating: row.plex_text("content_rating"),
            rule: None,
            source_uri: None,
            mode: mode_of(setting("pr:collectionMode")),
            order: order_of(setting("pr:collectionSort")),
            locked: row
                .plex_text("user_fields")
                .map(|fields| locked_fields(&fields))
                .unwrap_or_default(),
            thumb_url,
            art_url,
            added_at: row.plex_opt("added_at").unwrap_or(0),
            updated_at: row.plex_opt("updated_at").unwrap_or(0),
        };
        by_tag.insert((section, tag_number), collection.id);
        write::save_collection(&transaction, to_row(&collection)).await?;
        summary.collections += 1;
    }

    let members = memberships(source, source_path, item_ids, &by_tag, &mut summary).await?;
    // Each collection's custom order is its taggings' own: by `index`, then by id.
    let mut ranked: HashMap<CollectionId, Vec<(Rank, RatingKey)>> = HashMap::new();
    for (collection, item, rank) in &members {
        ranked.entry(*collection).or_default().push((*rank, *item));
    }
    let mut positions: HashMap<(CollectionId, RatingKey), i64> = HashMap::new();
    for (collection, mut items) in ranked {
        items.sort();
        for ((_, item), rank) in items.into_iter().zip(1_i64..) {
            positions.insert((collection, item), rank * POSITION_GAP);
        }
    }
    // Inserted in tagging order, across collections, so that an item lists its collections
    // in the order it joined them, as the reference does.
    let placed: Vec<(i64, RatingKey, i64)> = members
        .iter()
        .map(|(collection, item, _)| {
            let position = positions.get(&(*collection, *item)).copied();
            (collection.get(), *item, position.unwrap_or_default())
        })
        .collect();
    insert_placed(&transaction, &placed).await?;
    summary.entries = placed.len();
    transaction.commit().await?;
    Ok(summary)
}

/// A tagging's place in its collection's custom order: its `index`, then its id.
type Rank = (i64, i64);

/// The members of the imported collections, in tagging order, each with its rank.
async fn memberships<S: std::hash::BuildHasher>(
    source: &DatabaseConnection,
    source_path: &str,
    item_ids: &HashSet<i64, S>,
    by_tag: &HashMap<(i64, i64), CollectionId>,
    summary: &mut CollectionSummary,
) -> Result<Vec<(CollectionId, RatingKey, Rank)>, ImportError> {
    let mut members = Vec::new();
    let mut seen: HashSet<(CollectionId, i64)> = HashSet::new();
    let mut last_id = i64::MIN;
    loop {
        let rows = source
            .query_all(Statement::from_string(
                sea_orm::DbBackend::Sqlite,
                format!(
                    "SELECT tg.id, tg.metadata_item_id, tg.tag_id, tg.\"index\", \
                     i.library_section_id \
                     FROM taggings tg JOIN tags t ON t.id = tg.tag_id AND t.tag_type = 2 \
                     JOIN metadata_items i ON i.id = tg.metadata_item_id \
                     WHERE tg.id > {last_id} ORDER BY tg.id LIMIT {SOURCE_PAGE}"
                ),
            ))
            .await
            .map_err(|error| source_error(source_path, error))?;
        let Some(last) = rows.last() else {
            break;
        };
        last_id = last.plex_get("id")?;
        for row in &rows {
            let item: i64 = row.plex_get("metadata_item_id")?;
            if !item_ids.contains(&item) {
                continue;
            }
            let section: i64 = row.plex_get("library_section_id")?;
            let tag: i64 = row.plex_get("tag_id")?;
            let Some(collection) = by_tag.get(&(section, tag)).copied() else {
                summary.orphan_memberships += 1;
                continue;
            };
            // The source tags three items twice with the same collection.
            if !seen.insert((collection, item)) {
                continue;
            }
            let rank = (row.plex_opt("index").unwrap_or(0), row.plex_get("id")?);
            members.push((collection, RatingKey::new(item), rank));
        }
    }
    Ok(members)
}

/// The provider's poster and background of each collection tag, by tag number: what a
/// search result and a filter choice show. Only addresses on the web; a local path names
/// nothing a client could fetch.
async fn artwork(
    source: &DatabaseConnection,
    source_path: &str,
) -> Result<HashMap<i64, (Option<String>, Option<String>)>, ImportError> {
    let rows = source
        .query_all(Statement::from_string(
            sea_orm::DbBackend::Sqlite,
            "SELECT id, user_thumb_url, user_art_url FROM tags WHERE tag_type = 2".to_owned(),
        ))
        .await
        .map_err(|error| source_error(source_path, error))?;
    let web = |text: Option<String>| text.filter(|url| url.starts_with("https://"));
    rows.iter()
        .map(|row| {
            Ok((
                row.plex_get("id")?,
                (
                    web(row.plex_text("user_thumb_url")),
                    web(row.plex_text("user_art_url")),
                ),
            ))
        })
        .collect()
}

/// What a collection holds. The reference stores it in `rating_count`, as the `type` number
/// it was created with, plus 100 for a smart one; a value that names no kind falls back to
/// the library's top level.
fn kind_of(rating_count: i32, section_type: i32) -> ItemKind {
    match rating_count % 100 {
        1 => ItemKind::Movie,
        2 => ItemKind::Show,
        3 => ItemKind::Season,
        4 => ItemKind::Episode,
        _ if section_type == 2 => ItemKind::Show,
        _ => ItemKind::Movie,
    }
}

/// `pr:collectionMode`: -1 the library's setting, 0 hidden, 1 instead of its items, 2 with
/// them.
fn mode_of(stored: Option<&str>) -> CollectionMode {
    match stored {
        Some("0") => CollectionMode::Hidden,
        Some("1") => CollectionMode::InsteadOfItems,
        Some("2") => CollectionMode::WithItems,
        _ => CollectionMode::LibraryDefault,
    }
}

/// `pr:collectionSort`: 0 release date, 1 title, 2 custom.
fn order_of(stored: Option<&str>) -> CollectionOrder {
    match stored {
        Some("1") => CollectionOrder::Title,
        Some("2") => CollectionOrder::Custom,
        _ => CollectionOrder::ReleaseDate,
    }
}

/// The fields a collection's `user_fields` locks, by the numbers an item's use.
fn locked_fields(user_fields: &str) -> Vec<CollectionField> {
    let mut fields: Vec<CollectionField> = user_fields
        .split('&')
        .filter_map(|pair| pair.strip_prefix("lockedFields="))
        .flat_map(|numbers| numbers.split('|'))
        .filter_map(|number| match number.trim() {
            "1" => Some(CollectionField::Title),
            "2" => Some(CollectionField::TitleSort),
            "7" => Some(CollectionField::Summary),
            "8" => Some(CollectionField::ContentRating),
            "9" => Some(CollectionField::Poster),
            "10" => Some(CollectionField::Background),
            _ => None,
        })
        .collect();
    fields.sort();
    fields.dedup();
    fields
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_kind_from_the_stored_type_number() {
        assert_eq!(kind_of(1, 1), ItemKind::Movie);
        assert_eq!(kind_of(2, 2), ItemKind::Show);
        assert_eq!(kind_of(104, 2), ItemKind::Episode);
        assert_eq!(kind_of(0, 2), ItemKind::Show);
        assert_eq!(kind_of(99, 1), ItemKind::Movie);
    }

    #[test]
    fn unpacks_settings_and_locks() {
        assert_eq!(mode_of(Some("-1")), CollectionMode::LibraryDefault);
        assert_eq!(mode_of(Some("1")), CollectionMode::InsteadOfItems);
        assert_eq!(mode_of(None), CollectionMode::LibraryDefault);
        assert_eq!(order_of(Some("2")), CollectionOrder::Custom);
        assert_eq!(order_of(Some("7")), CollectionOrder::ReleaseDate);
        assert_eq!(
            locked_fields("lockedFields=1|2|9|16"),
            vec![
                CollectionField::Title,
                CollectionField::TitleSort,
                CollectionField::Poster
            ]
        );
    }
}
