//! Importing tags, the people they credit, and what external providers say about an item.
//!
//! Three passes over the source. People first, because a credit points at one: Plex has no
//! person table, only one tag per person and role, so people are rebuilt by grouping those
//! tags on the provider's key. Then the tags. Then one pass over the attachments, which is
//! where Plex also keeps external identifiers (`tag_type` 314) and ratings (316); those go to
//! their own tables.
//!
//! Every pass pages by key (`id > last`) rather than by `OFFSET`: an offset re-reads every row
//! before it, and over 5.6 million attachments that turns the pass quadratic.

use std::collections::{HashMap, HashSet};

use open_plex_core::{RatingKind, TagKind};
use sea_orm::{
    ActiveValue, ConnectionTrait, DatabaseConnection, EntityTrait, QueryResult, Statement,
};

use super::{source_error, ImportError, PlexRow, SOURCE_PAGE};
use crate::entity::{item_external_id, item_rating, item_tag, person, tag};

const WRITE_CHUNK: usize = 500;

/// The source's `tag_type` values for credits, as they appear in SQL.
const CREDIT_TYPES: &str = "4, 5, 6, 7";
const EXTERNAL_ID_TYPE: i32 = 314;
const RATING_TYPE: i32 = 316;

#[derive(Debug, Default, Clone, Copy)]
pub struct TagSummary {
    pub people: usize,
    pub tags: usize,
    /// Tags attached to items: one row per genre, country or credit.
    pub item_tags: usize,
    pub external_ids: usize,
    pub ratings: usize,
}

/// Replaces the stored tags, people, identifiers and ratings with the source's.
///
/// Deleting first is what makes a re-import idempotent: tags and people outlive the items a
/// re-import replaces. Attachments, identifiers and ratings go with them by cascade, and are
/// deleted explicitly anyway so the order of the passes does not depend on it.
///
/// # Errors
///
/// Fails if the source cannot be read or a write fails.
pub(crate) async fn import(
    destination: &DatabaseConnection,
    source: &DatabaseConnection,
    source_path: &str,
    item_ids: &HashSet<i64>,
) -> Result<TagSummary, ImportError> {
    item_external_id::Entity::delete_many()
        .exec(destination)
        .await?;
    item_rating::Entity::delete_many().exec(destination).await?;
    item_tag::Entity::delete_many().exec(destination).await?;
    tag::Entity::delete_many().exec(destination).await?;
    person::Entity::delete_many().exec(destination).await?;

    let mut people = import_people(destination, source, source_path).await?;
    let tag_ids = import_tags(destination, source, source_path, &mut people).await?;
    let attachments =
        import_attachments(destination, source, source_path, item_ids, &tag_ids).await?;

    Ok(TagSummary {
        people: people.written,
        tags: tag_ids.len(),
        item_tags: attachments.item_tags,
        external_ids: attachments.external_ids,
        ratings: attachments.ratings,
    })
}

/// People as rebuilt from the source, and the ids they were given.
#[derive(Debug, Default)]
struct People {
    by_provider_key: HashMap<String, i64>,
    next_id: i64,
    written: usize,
}

impl People {
    fn allocate(&mut self) -> i64 {
        self.next_id += 1;
        self.next_id
    }
}

/// What one person looks like after reading all of their tags.
struct PersonDraft {
    id: i64,
    name: String,
    /// Every portrait the person's tags carry, with how many attachments show each.
    portraits: HashMap<String, u64>,
}

impl PersonDraft {
    /// The portrait most of the person's credits show.
    ///
    /// The reference shows each tag's own portrait, and 964 people carry two; one per person
    /// cannot match them all. Measured over the 3.45 million credits of the reference
    /// library, the most attached portrait leaves 6,717 showing another, the most recent
    /// tag's 29,094.
    fn portrait(&self) -> Option<String> {
        self.portraits
            .iter()
            .max_by(|left, right| left.1.cmp(right.1).then_with(|| right.0.cmp(left.0)))
            .map(|(url, _)| url.clone())
    }
}

/// Creates one person per provider key.
///
/// The key is what identifies a person; the name does not, since homonyms share one. A
/// person's tags do not always agree — 949 keys carry two spellings, `Andy Garcia` and
/// `Andy García`, and 964 two portraits. The name of the most recent tag wins, being the one
/// the provider last refreshed; the portrait is the one most credits show.
async fn import_people(
    destination: &DatabaseConnection,
    source: &DatabaseConnection,
    source_path: &str,
) -> Result<People, ImportError> {
    let mut people = People::default();
    let mut drafts: HashMap<String, PersonDraft> = HashMap::new();
    let mut last_id = 0_i64;
    loop {
        let rows = source
            .query_all(Statement::from_string(
                sea_orm::DbBackend::Sqlite,
                format!(
                    "SELECT id, tag, key, user_thumb_url, \
                     (SELECT COUNT(*) FROM taggings WHERE tag_id = tags.id) AS attached \
                     FROM tags WHERE tag_type IN ({CREDIT_TYPES}) AND key IS NOT NULL \
                     AND key <> '' AND id > {last_id} ORDER BY id LIMIT {SOURCE_PAGE}"
                ),
            ))
            .await
            .map_err(|error| source_error(source_path, error))?;
        let Some(last) = rows.last() else {
            break;
        };
        last_id = last.plex_get("id")?;

        for row in &rows {
            let (Some(key), Some(name)) = (row.plex_text("key"), row.plex_text("tag")) else {
                continue;
            };
            let draft = drafts.entry(key).or_insert_with(|| PersonDraft {
                id: people.allocate(),
                name: String::new(),
                portraits: HashMap::new(),
            });
            draft.name = name;
            if let Some(url) = row.plex_text("user_thumb_url") {
                let attached =
                    u64::try_from(row.plex_opt::<i64>("attached").unwrap_or(0)).unwrap_or(0);
                *draft.portraits.entry(url).or_insert(0) += attached;
            }
        }
    }

    let mut rows: Vec<person::ActiveModel> = Vec::with_capacity(drafts.len());
    for (key, draft) in drafts {
        people.by_provider_key.insert(key.clone(), draft.id);
        rows.push(person::ActiveModel {
            id: ActiveValue::Set(draft.id),
            thumb_url: ActiveValue::Set(draft.portrait()),
            name: ActiveValue::Set(draft.name),
            provider_key: ActiveValue::Set(Some(key)),
        });
    }
    // In id order, so the table reads in the order people were first met.
    rows.sort_by_key(|row| *row.id.as_ref());
    for chunk in rows.chunks(WRITE_CHUNK) {
        person::Entity::insert_many(chunk.to_vec())
            .exec(destination)
            .await?;
    }
    people.written = rows.len();
    Ok(people)
}

/// Imports every tag of the six kinds, attaching each credit to its person.
///
/// A credit with no provider key gets a person of its own rather than being matched by
/// name: without the key there is no telling a namesake from the same person, and merging
/// two people is the worse mistake.
async fn import_tags(
    destination: &DatabaseConnection,
    source: &DatabaseConnection,
    source_path: &str,
    people: &mut People,
) -> Result<HashSet<i64>, ImportError> {
    let mut written = HashSet::new();
    let mut last_id = 0_i64;
    loop {
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

        let mut keyless = Vec::new();
        let mut tags = Vec::with_capacity(rows.len());
        for row in &rows {
            let Some(kind) = row.plex_opt::<i32>("tag_type").and_then(tag_kind) else {
                continue;
            };
            let Some(name) = row.plex_text("tag") else {
                continue;
            };
            let person_id = match kind {
                TagKind::Genre | TagKind::Country => None,
                _ => Some(match row.plex_text("key") {
                    Some(key) if people.by_provider_key.contains_key(&key) => {
                        people.by_provider_key[&key]
                    }
                    _ => {
                        let id = people.allocate();
                        keyless.push(person::ActiveModel {
                            id: ActiveValue::Set(id),
                            name: ActiveValue::Set(name.clone()),
                            provider_key: ActiveValue::Set(None),
                            thumb_url: ActiveValue::Set(row.plex_text("user_thumb_url")),
                        });
                        id
                    }
                }),
            };
            let id: i64 = row.plex_get("id")?;
            tags.push(tag::ActiveModel {
                id: ActiveValue::Set(id),
                kind: ActiveValue::Set(tag::kind_name(kind).to_owned()),
                name: ActiveValue::Set(name),
                person_id: ActiveValue::Set(person_id),
            });
            written.insert(id);
        }

        // People before the tags pointing at them: the foreign key is real.
        people.written += keyless.len();
        for chunk in keyless.chunks(WRITE_CHUNK) {
            person::Entity::insert_many(chunk.to_vec())
                .exec(destination)
                .await?;
        }
        for chunk in tags.chunks(WRITE_CHUNK) {
            tag::Entity::insert_many(chunk.to_vec())
                .exec(destination)
                .await?;
        }
    }

    Ok(written)
}

#[derive(Debug, Default)]
struct Attachments {
    item_tags: usize,
    external_ids: usize,
    ratings: usize,
}

/// A rating badge: who gives the score, and whether critics or the audience.
struct Badge {
    source: String,
    kind: RatingKind,
    image: String,
}

/// One pass over the source's attachments, sorting each into tags, identifiers or ratings.
///
/// An actor's `text` is the character played and is kept; directors, writers and producers
/// carry text too, which the reference publishes for none of them, so it is dropped.
async fn import_attachments(
    destination: &DatabaseConnection,
    source: &DatabaseConnection,
    source_path: &str,
    item_ids: &HashSet<i64>,
    tag_ids: &HashSet<i64>,
) -> Result<Attachments, ImportError> {
    let badges = rating_badges(source, source_path).await?;
    // Ratings carry no order of their own in the source; this keeps the order they were
    // attached in.
    let mut rating_positions: HashMap<i64, i32> = HashMap::new();
    let mut counts = Attachments::default();
    let mut last_id = 0_i64;
    loop {
        let rows = source
            .query_all(Statement::from_string(
                sea_orm::DbBackend::Sqlite,
                format!(
                    "SELECT tg.id, tg.metadata_item_id, tg.tag_id, tg.\"index\", tg.text, \
                     t.tag_type, t.tag FROM taggings tg JOIN tags t ON t.id = tg.tag_id \
                     WHERE t.tag_type IN (1, 4, 5, 6, 7, 8, {EXTERNAL_ID_TYPE}, {RATING_TYPE}) \
                     AND tg.id > {last_id} ORDER BY tg.id LIMIT {SOURCE_PAGE}"
                ),
            ))
            .await
            .map_err(|error| source_error(source_path, error))?;
        let Some(last) = rows.last() else {
            break;
        };
        last_id = last.plex_get("id")?;

        let mut batch = AttachmentBatch::default();
        for row in &rows {
            batch.add(row, item_ids, tag_ids, &badges, &mut rating_positions);
        }
        batch.write(destination, &mut counts).await?;
    }

    Ok(counts)
}

/// One page of attachments, sorted into the three tables they go to.
#[derive(Default)]
struct AttachmentBatch {
    item_tags: Vec<item_tag::ActiveModel>,
    external_ids: Vec<item_external_id::ActiveModel>,
    ratings: Vec<item_rating::ActiveModel>,
}

impl AttachmentBatch {
    fn add(
        &mut self,
        row: &QueryResult,
        item_ids: &HashSet<i64>,
        tag_ids: &HashSet<i64>,
        badges: &HashMap<i64, Badge>,
        rating_positions: &mut HashMap<i64, i32>,
    ) {
        let (Some(item_id), Some(tag_id), Some(tag_type)) = (
            row.plex_opt::<i64>("metadata_item_id"),
            row.plex_opt::<i64>("tag_id"),
            row.plex_opt::<i32>("tag_type"),
        ) else {
            return;
        };
        // Extras, collections and tracks are not imported, and neither is what is attached
        // to them.
        if !item_ids.contains(&item_id) {
            return;
        }
        let position: i32 = row.plex_opt("index").unwrap_or(0);

        match tag_type {
            EXTERNAL_ID_TYPE => {
                if let Some((source, value)) =
                    row.plex_text("tag").and_then(|guid| split_guid(&guid))
                {
                    self.external_ids.push(item_external_id::ActiveModel {
                        item_id: ActiveValue::Set(item_id),
                        source: ActiveValue::Set(source),
                        value: ActiveValue::Set(value),
                        position: ActiveValue::Set(position),
                    });
                }
            }
            RATING_TYPE => {
                let score = row
                    .plex_text("text")
                    .and_then(|text| text.parse::<f64>().ok());
                if let (Some(badge), Some(value)) = (badges.get(&tag_id), score) {
                    let order = rating_positions.entry(item_id).or_insert(0);
                    self.ratings.push(item_rating::ActiveModel {
                        item_id: ActiveValue::Set(item_id),
                        source: ActiveValue::Set(badge.source.clone()),
                        kind: ActiveValue::Set(item_rating::kind_name(badge.kind).to_owned()),
                        value: ActiveValue::Set(value),
                        image: ActiveValue::Set(badge.image.clone()),
                        position: ActiveValue::Set(*order),
                    });
                    *order += 1;
                }
            }
            _ if tag_ids.contains(&tag_id) => {
                let is_actor = tag_kind(tag_type) == Some(TagKind::Actor);
                self.item_tags.push(item_tag::ActiveModel {
                    item_id: ActiveValue::Set(item_id),
                    tag_id: ActiveValue::Set(tag_id),
                    position: ActiveValue::Set(position),
                    role: ActiveValue::Set(if is_actor {
                        row.plex_text("text")
                    } else {
                        None
                    }),
                });
            }
            _ => {}
        }
    }

    async fn write(
        self,
        destination: &DatabaseConnection,
        counts: &mut Attachments,
    ) -> Result<(), ImportError> {
        counts.item_tags += self.item_tags.len();
        counts.external_ids += self.external_ids.len();
        counts.ratings += self.ratings.len();
        for chunk in self.item_tags.chunks(WRITE_CHUNK) {
            item_tag::Entity::insert_many(chunk.to_vec())
                .exec(destination)
                .await?;
        }
        for chunk in self.external_ids.chunks(WRITE_CHUNK) {
            item_external_id::Entity::insert_many(chunk.to_vec())
                .exec(destination)
                .await?;
        }
        for chunk in self.ratings.chunks(WRITE_CHUNK) {
            item_rating::Entity::insert_many(chunk.to_vec())
                .exec(destination)
                .await?;
        }
        Ok(())
    }
}

/// The rating badges, by tag id.
///
/// Six in the reference library. The badge names the provider; whether it scores critics or
/// the audience is packed into the tag's `extra_data`, as `"at:type":"critic"`.
async fn rating_badges(
    source: &DatabaseConnection,
    source_path: &str,
) -> Result<HashMap<i64, Badge>, ImportError> {
    let rows = source
        .query_all(Statement::from_string(
            sea_orm::DbBackend::Sqlite,
            format!("SELECT id, tag, extra_data FROM tags WHERE tag_type = {RATING_TYPE}"),
        ))
        .await
        .map_err(|error| source_error(source_path, error))?;

    let mut badges = HashMap::new();
    for row in &rows {
        let Some(image) = row.plex_text("tag") else {
            continue;
        };
        let Some((source_name, _)) = image.split_once("://") else {
            continue;
        };
        let kind = row
            .plex_text("extra_data")
            .and_then(|packed| serde_json::from_str::<serde_json::Value>(&packed).ok())
            .and_then(|extra| match extra.get("at:type")?.as_str()? {
                "critic" => Some(RatingKind::Critic),
                "audience" => Some(RatingKind::Audience),
                _ => None,
            });
        let Some(kind) = kind else {
            tracing::warn!(badge = image, "rating badge of an unknown kind, skipped");
            continue;
        };
        badges.insert(
            row.plex_get("id")?,
            Badge {
                source: source_name.to_owned(),
                kind,
                image,
            },
        );
    }
    Ok(badges)
}

/// Splits `imdb://tt0293662` into its source and its value.
fn split_guid(guid: &str) -> Option<(String, String)> {
    let (source, value) = guid.split_once("://")?;
    (!source.is_empty() && !value.is_empty()).then(|| (source.to_owned(), value.to_owned()))
}

/// Plex's `tag_type` discriminants for the kinds we import as tags, identified by matching
/// the reference server's output element by element: 1 is `<Genre>`, 6 is `<Role>`, and so
/// on. The source uses some twenty others — artwork, identifiers, reviews, ratings.
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{split_guid, PersonDraft};

    #[test]
    fn a_person_shows_the_portrait_most_of_their_credits_show() {
        let draft = PersonDraft {
            id: 1,
            name: "Andy Garcia".to_owned(),
            portraits: HashMap::from([
                ("https://metadata-static.plex.tv/old.jpg".to_owned(), 41),
                ("https://metadata-static.plex.tv/new.jpg".to_owned(), 3),
            ]),
        };
        assert_eq!(
            draft.portrait().as_deref(),
            Some("https://metadata-static.plex.tv/old.jpg")
        );
    }

    #[test]
    fn splits_a_guid_into_source_and_value() {
        assert_eq!(
            split_guid("imdb://tt0293662"),
            Some(("imdb".to_owned(), "tt0293662".to_owned()))
        );
        assert_eq!(split_guid("tvdb://"), None);
        assert_eq!(split_guid("no-scheme"), None);
    }
}
