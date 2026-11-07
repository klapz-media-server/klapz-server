//! The one way to insert items.
//!
//! An item carries a derived index — its searchable title words — and that index is only
//! correct if *every* write maintains it. Leaving that to each caller is how a search quietly
//! stops finding things: the importer remembers, a later write path does not, and nothing
//! fails until a user cannot find a film that is plainly there. The test suite found this the
//! easy way, by inserting items directly and then matching nothing.
//!
//! So the words are derived here, from the models themselves, and there is no path that
//! inserts an item without them. Tags follow the same rule for the same reason: the folded
//! name a filter matches against is derived here, never handed in.

use open_plex_core::{metadata, search, Tag};
use sea_orm::{ActiveValue, ConnectionTrait, DbErr, EntityTrait};

use crate::entity::{item_title_word, metadata_item, tag};

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
    let words: Vec<item_title_word::ActiveModel> = items.iter().flat_map(title_words).collect();

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

/// Inserts tags, with the folded name a filter by name compares against.
///
/// # Errors
///
/// Fails if any insert fails.
pub async fn insert_tags<C: ConnectionTrait>(connection: &C, tags: &[Tag]) -> Result<(), DbErr> {
    let rows: Vec<tag::ActiveModel> = tags
        .iter()
        .map(|value| tag::ActiveModel {
            id: ActiveValue::Set(value.id.get()),
            kind: ActiveValue::Set(tag::kind_name(value.kind).to_owned()),
            name: ActiveValue::Set(value.name.clone()),
            name_key: ActiveValue::Set(metadata::tag_name_key(&value.name)),
            provider_key: ActiveValue::Set(value.provider_key.clone()),
            thumb_url: ActiveValue::Set(value.thumb_url.clone()),
        })
        .collect();

    for chunk in rows.chunks(WRITE_CHUNK) {
        tag::Entity::insert_many(chunk.to_vec())
            .exec(connection)
            .await?;
    }
    Ok(())
}

/// The word rows for one item, read back off the model being written.
///
/// Derived from the model rather than taken as a second argument, so the two cannot disagree.
fn title_words(item: &metadata_item::ActiveModel) -> Vec<item_title_word::ActiveModel> {
    let Some(&id) = item.id.try_as_ref() else {
        // An item with no id cannot be referenced; the insert itself will reject it.
        return Vec::new();
    };
    let text = |value: &ActiveValue<Option<String>>| value.try_as_ref().cloned().flatten();

    search::title_words(
        text(&item.title).as_deref(),
        text(&item.title_sort).as_deref(),
        text(&item.original_title).as_deref(),
    )
    .into_iter()
    .map(|word| item_title_word::ActiveModel {
        item_id: ActiveValue::Set(id),
        word: ActiveValue::Set(word),
    })
    .collect()
}
