//! The one way to write items.
//!
//! An item carries a derived index — its searchable title words — and that index is only
//! correct if *every* write maintains it. Leaving that to each caller is how a search quietly
//! stops finding things: the importer remembers, a later write path does not, and nothing
//! fails until a user cannot find a film that is plainly there. The test suite found this the
//! easy way, by inserting items directly and then matching nothing.
//!
//! So the words are derived here, from the values being written, and there is no path that
//! inserts or edits an item without them. An edit rewrites the sort key the same way. Tags
//! carry the same kind of index over their names, written the same way, and so do
//! collections over their titles.

use std::collections::BTreeSet;

use open_plex_core::edit::ItemField;
use open_plex_core::metadata::{self, MetadataItem};
use open_plex_core::search;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter,
};

use crate::entity::{
    collection, collection_word, item_field_lock, item_title_word, metadata_item, tag, tag_word,
};

/// Rows per statement.
///
/// `SQLite` caps bound parameters per statement, and an item carries twenty-odd columns; 500
/// rows stays comfortably under every engine's limit while keeping the round trips few.
const WRITE_CHUNK: usize = 500;

/// Inserts items and their title-word index.
///
/// # Errors
///
/// Fails if any insert fails. Not transactional on purpose: the importer runs this per source
/// page and reports progress, and a partial import is resumable because the whole operation is
/// idempotent on a fresh database.
pub async fn insert_items<C: ConnectionTrait>(
    connection: &C,
    items: Vec<metadata_item::ActiveModel>,
) -> Result<usize, DbErr> {
    let words: Vec<item_title_word::ActiveModel> = items.iter().flat_map(model_words).collect();

    for chunk in items.chunks(WRITE_CHUNK) {
        metadata_item::Entity::insert_many(chunk.to_vec())
            .exec(connection)
            .await?;
    }
    // After the items: the word rows carry a foreign key to them.
    for chunk in words.chunks(WRITE_CHUNK) {
        item_title_word::Entity::insert_many(chunk.to_vec())
            .exec(connection)
            .await?;
    }

    Ok(words.len())
}

/// Inserts tags and their name-word index.
///
/// # Errors
///
/// Fails if any insert fails, as [`insert_items`] does and for the same reasons.
pub async fn insert_tags<C: ConnectionTrait>(
    connection: &C,
    tags: Vec<tag::ActiveModel>,
) -> Result<(), DbErr> {
    let words: Vec<tag_word::ActiveModel> = tags
        .iter()
        .filter_map(|tag| Some((*tag.id.try_as_ref()?, tag.name.try_as_ref()?)))
        .flat_map(|(id, name)| {
            search::name_words(name)
                .into_iter()
                .map(move |word| tag_word::ActiveModel {
                    tag_id: ActiveValue::Set(id),
                    word: ActiveValue::Set(word),
                })
        })
        .collect();

    for chunk in tags.chunks(WRITE_CHUNK) {
        tag::Entity::insert_many(chunk.to_vec())
            .exec(connection)
            .await?;
    }
    for chunk in words.chunks(WRITE_CHUNK) {
        tag_word::Entity::insert_many(chunk.to_vec())
            .exec(connection)
            .await?;
    }
    Ok(())
}

/// Writes a collection, new or over the one under its id, with its sort key and the words
/// its titles are searched by. Its entries are left as they are. Run it inside a
/// transaction.
///
/// # Errors
///
/// Fails if any statement fails.
pub async fn save_collection<C: ConnectionTrait>(
    connection: &C,
    mut row: collection::Model,
) -> Result<(), DbErr> {
    row.sort_key = metadata::sort_key(row.title_sort.as_deref(), Some(&row.title));
    let id = row.id;
    let words: Vec<collection_word::ActiveModel> =
        search::title_words(Some(&row.title), row.title_sort.as_deref(), None)
            .into_iter()
            .map(|word| collection_word::ActiveModel {
                collection_id: ActiveValue::Set(id),
                word: ActiveValue::Set(word),
            })
            .collect();
    let exists = collection::Entity::find_by_id(id)
        .one(connection)
        .await?
        .is_some();
    let active = collection::ActiveModel::from(row).reset_all();
    if exists {
        collection::Entity::update(active).exec(connection).await?;
    } else {
        collection::Entity::insert(active).exec(connection).await?;
    }
    collection_word::Entity::delete_many()
        .filter(collection_word::Column::CollectionId.eq(id))
        .exec(connection)
        .await?;
    if !words.is_empty() {
        collection_word::Entity::insert_many(words)
            .exec(connection)
            .await?;
    }
    Ok(())
}

/// Writes an edited item's values, sort key, title words and locks.
///
/// Every column an edit can reach is written, whether or not it changed: the edit has already
/// been applied to `item`, and writing its whole editable state is what keeps this function
/// ignorant of which fields an edit touches. Run it inside a transaction.
///
/// # Errors
///
/// Fails if any statement fails.
pub async fn update_item<C: ConnectionTrait>(
    connection: &C,
    item: &MetadataItem,
    locked: &BTreeSet<ItemField>,
) -> Result<(), DbErr> {
    let id = item.id.get();
    metadata_item::Entity::update(metadata_item::ActiveModel {
        id: ActiveValue::Unchanged(id),
        title: ActiveValue::Set(item.title.clone()),
        title_sort: ActiveValue::Set(item.title_sort.clone()),
        sort_key: ActiveValue::Set(metadata::sort_key(
            item.title_sort.as_deref(),
            item.title.as_deref(),
        )),
        original_title: ActiveValue::Set(item.original_title.clone()),
        summary: ActiveValue::Set(item.summary.clone()),
        tagline: ActiveValue::Set(item.tagline.clone()),
        studio: ActiveValue::Set(item.studio.clone()),
        content_rating: ActiveValue::Set(item.content_rating.clone()),
        year: ActiveValue::Set(item.year),
        released_on: ActiveValue::Set(item.released_on.clone()),
        rating: ActiveValue::Set(item.rating),
        audience_rating: ActiveValue::Set(item.audience_rating),
        ..Default::default()
    })
    .exec(connection)
    .await?;

    item_title_word::Entity::delete_many()
        .filter(item_title_word::Column::ItemId.eq(id))
        .exec(connection)
        .await?;
    let words = title_words(
        id,
        item.title.as_deref(),
        item.title_sort.as_deref(),
        item.original_title.as_deref(),
    );
    if !words.is_empty() {
        item_title_word::Entity::insert_many(words)
            .exec(connection)
            .await?;
    }

    item_field_lock::Entity::delete_many()
        .filter(item_field_lock::Column::ItemId.eq(id))
        .exec(connection)
        .await?;
    insert_locks(connection, locked.iter().map(|field| (id, *field))).await
}

/// Inserts locks on items that have none of these yet.
///
/// # Errors
///
/// Fails if an insert fails, an already present lock included.
pub async fn insert_locks<C: ConnectionTrait>(
    connection: &C,
    locks: impl IntoIterator<Item = (i64, ItemField)>,
) -> Result<(), DbErr> {
    let rows: Vec<item_field_lock::ActiveModel> = locks
        .into_iter()
        .map(|(item_id, field)| item_field_lock::ActiveModel {
            item_id: ActiveValue::Set(item_id),
            field: ActiveValue::Set(field_name(field).to_owned()),
        })
        .collect();
    for chunk in rows.chunks(WRITE_CHUNK) {
        item_field_lock::Entity::insert_many(chunk.to_vec())
            .exec(connection)
            .await?;
    }
    Ok(())
}

/// How a locked field is stored.
pub const fn field_name(field: ItemField) -> &'static str {
    match field {
        ItemField::Title => "title",
        ItemField::TitleSort => "title_sort",
        ItemField::OriginalTitle => "original_title",
        ItemField::Studio => "studio",
        ItemField::Rating => "rating",
        ItemField::Tagline => "tagline",
        ItemField::Summary => "summary",
        ItemField::ContentRating => "content_rating",
        ItemField::Poster => "poster",
        ItemField::Background => "background",
        ItemField::ReleasedOn => "released_on",
        ItemField::Year => "year",
        ItemField::Genres => "genres",
        ItemField::Collections => "collections",
        ItemField::Directors => "directors",
        ItemField::Writers => "writers",
        ItemField::Cast => "cast",
        ItemField::Producers => "producers",
        ItemField::Countries => "countries",
        ItemField::AudienceRating => "audience_rating",
    }
}

/// The field a stored name stands for. `None` for a name no version of this schema wrote.
pub fn parse_field_name(stored: &str) -> Option<ItemField> {
    ItemField::ALL
        .into_iter()
        .find(|field| field_name(*field) == stored)
}

/// The word rows for one item, read back off the model being written.
///
/// Derived from the model rather than taken as a second argument, so the two cannot disagree.
fn model_words(item: &metadata_item::ActiveModel) -> Vec<item_title_word::ActiveModel> {
    let Some(&id) = item.id.try_as_ref() else {
        // An item with no id cannot be referenced; the insert itself will reject it.
        return Vec::new();
    };
    let text = |value: &ActiveValue<Option<String>>| value.try_as_ref().cloned().flatten();
    title_words(
        id,
        text(&item.title).as_deref(),
        text(&item.title_sort).as_deref(),
        text(&item.original_title).as_deref(),
    )
}

fn title_words(
    id: i64,
    title: Option<&str>,
    title_sort: Option<&str>,
    original_title: Option<&str>,
) -> Vec<item_title_word::ActiveModel> {
    search::title_words(title, title_sort, original_title)
        .into_iter()
        .map(|word| item_title_word::ActiveModel {
            item_id: ActiveValue::Set(id),
            word: ActiveValue::Set(word),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_field_reads_back_from_its_stored_name() {
        for field in ItemField::ALL {
            assert_eq!(parse_field_name(field_name(field)), Some(field));
        }
        assert_eq!(parse_field_name("thumb"), None);
    }
}
