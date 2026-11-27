//! Storage for source records and the items they describe (`core::source_metadata`).

use std::collections::{BTreeSet, HashMap};

use klapz_core::edit::ItemField;
use klapz_core::language::{Language, LanguageTag};
use klapz_core::metadata::{ItemKind, MetadataItem, TagKind};
use klapz_core::source_metadata::{
    AppliedItem, AppliedTag, ExternalId, ImageKind, ItemImage, Linked, MatchPlan, MatchTarget,
    ProviderRating, SavedSource, SeasonTarget, ShownImage, SourceMetadataError,
    SourceMetadataRepository, SourceRecord, Target,
};
use klapz_core::{RatingKey, SectionId};
use sea_orm::sea_query::{Alias, Condition, Expr, Query};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, DbErr, EntityTrait,
    QueryFilter, QuerySelect, TransactionTrait,
};

use crate::entity::{
    image_file, item_corner_colors, item_external_id, item_field_lock, item_image, item_rating,
    item_source, item_tag, library_section, metadata_item, person, source_metadata,
    source_metadata_certification, source_metadata_external_id, source_metadata_image,
    source_metadata_translation, tag, user_favorite, user_rating, watch_state,
};
use crate::metadata_repository::{kind_name, to_domain};
use crate::write;

/// Rows per insert, under every engine's limit on bound parameters.
const INSERT_CHUNK: usize = 400;

#[derive(Debug, Clone)]
pub struct SqlSourceMetadataRepository {
    connection: DatabaseConnection,
}

impl SqlSourceMetadataRepository {
    pub fn new(connection: DatabaseConnection) -> Self {
        Self { connection }
    }
}

fn storage(error: DbErr) -> SourceMetadataError {
    SourceMetadataError::Storage(Box::new(error))
}

#[async_trait::async_trait]
impl SourceMetadataRepository for SqlSourceMetadataRepository {
    async fn match_plan(
        &self,
        section: SectionId,
        force: bool,
    ) -> Result<MatchPlan, SourceMetadataError> {
        let library = library_section::Entity::find_by_id(section.get())
            .one(&self.connection)
            .await
            .map_err(storage)?
            .ok_or(SourceMetadataError::SectionNotFound(section))?;
        let top_kind = match library.kind.as_str() {
            "movie" => ItemKind::Movie,
            "show" => ItemKind::Show,
            _ => return Err(SourceMetadataError::UnsupportedKind),
        };
        let mut top = metadata_item::Entity::find()
            .filter(metadata_item::Column::LibrarySectionId.eq(section.get()))
            .filter(metadata_item::Column::ParentId.is_null())
            .filter(metadata_item::Column::Kind.eq(kind_name(top_kind)));
        if !force {
            top = top.filter(
                metadata_item::Column::Id.not_in_subquery(
                    Query::select()
                        .column(item_source::Column::ItemId)
                        .from(item_source::Entity)
                        .to_owned(),
                ),
            );
        }
        let top = top.all(&self.connection).await.map_err(storage)?;
        let targets = self.targets(top).await?;
        Ok(MatchPlan {
            kind: top_kind,
            language: LanguageTag::parse(&library.language).unwrap_or(Language::Und.into()),
            targets,
        })
    }

    async fn save(
        &self,
        record: &SourceRecord,
        at: i64,
    ) -> Result<SavedSource, SourceMetadataError> {
        let transaction = self.connection.begin().await.map_err(storage)?;
        let id = save_row(&transaction, record, None, at)
            .await
            .map_err(storage)?;
        let mut seasons = Vec::with_capacity(record.children.len());
        for season in &record.children {
            let season_id = save_row(&transaction, season, Some(id), at)
                .await
                .map_err(storage)?;
            let mut episodes = Vec::with_capacity(season.children.len());
            for episode in &season.children {
                episodes.push(SavedSource {
                    id: save_row(&transaction, episode, Some(season_id), at)
                        .await
                        .map_err(storage)?,
                    children: Vec::new(),
                });
            }
            seasons.push(SavedSource {
                id: season_id,
                children: episodes,
            });
        }
        transaction.commit().await.map_err(storage)?;
        Ok(SavedSource {
            id,
            children: seasons,
        })
    }

    async fn imported_guid(
        &self,
        kind: ItemKind,
        ids: &[ExternalId],
    ) -> Result<Option<String>, SourceMetadataError> {
        if ids.is_empty() {
            return Ok(None);
        }
        let any_id = ids.iter().fold(Condition::any(), |condition, id| {
            condition.add(
                Condition::all()
                    .add(item_external_id::Column::Source.eq(id.source.as_str()))
                    .add(item_external_id::Column::Value.eq(id.value.as_str())),
            )
        });
        let item_ids = Query::select()
            .column(item_external_id::Column::ItemId)
            .from(item_external_id::Entity)
            .cond_where(any_id)
            .to_owned();
        let guid = metadata_item::Entity::find()
            .select_only()
            .column(metadata_item::Column::Guid)
            .filter(metadata_item::Column::Kind.eq(kind_name(kind)))
            .filter(metadata_item::Column::Guid.starts_with("plex://"))
            .filter(metadata_item::Column::Id.in_subquery(item_ids))
            .into_tuple::<String>()
            .one(&self.connection)
            .await
            .map_err(storage)?;
        Ok(guid)
    }

    async fn apply(&self, items: &[AppliedItem], at: i64) -> Result<(), SourceMetadataError> {
        let transaction = self.connection.begin().await.map_err(storage)?;
        for applied in items {
            apply_item(&transaction, applied, at)
                .await
                .map_err(storage)?;
        }
        transaction.commit().await.map_err(storage)
    }

    async fn save_images(&self, images: &[ItemImage], at: i64) -> Result<(), SourceMetadataError> {
        let transaction = self.connection.begin().await.map_err(storage)?;
        for image in images {
            save_image(&transaction, image, at).await.map_err(storage)?;
        }
        transaction.commit().await.map_err(storage)
    }

    async fn item_target(
        &self,
        item: RatingKey,
    ) -> Result<Option<(LanguageTag, MatchTarget)>, SourceMetadataError> {
        let Some(row) = metadata_item::Entity::find_by_id(item.get())
            .one(&self.connection)
            .await
            .map_err(storage)?
            .filter(|row| row.parent_id.is_none() && matches!(row.kind.as_str(), "movie" | "show"))
        else {
            return Ok(None);
        };
        let library = library_section::Entity::find_by_id(row.library_section_id)
            .one(&self.connection)
            .await
            .map_err(storage)?
            .ok_or(SourceMetadataError::SectionNotFound(SectionId::new(
                row.library_section_id,
            )))?;
        let target = self.targets(vec![row]).await?.into_iter().next();
        Ok(target.map(|target| {
            (
                LanguageTag::parse(&library.language).unwrap_or(Language::Und.into()),
                target,
            )
        }))
    }

    async fn unmatch(&self, item: RatingKey) -> Result<(), SourceMetadataError> {
        let transaction = self.connection.begin().await.map_err(storage)?;
        let seasons: Vec<i64> = metadata_item::Entity::find()
            .select_only()
            .column(metadata_item::Column::Id)
            .filter(metadata_item::Column::ParentId.eq(item.get()))
            .into_tuple()
            .all(&transaction)
            .await
            .map_err(storage)?;
        let episodes: Vec<i64> = if seasons.is_empty() {
            Vec::new()
        } else {
            metadata_item::Entity::find()
                .select_only()
                .column(metadata_item::Column::Id)
                .filter(metadata_item::Column::ParentId.is_in(seasons.iter().copied()))
                .into_tuple()
                .all(&transaction)
                .await
                .map_err(storage)?
        };
        let all: Vec<i64> = std::iter::once(item.get())
            .chain(seasons)
            .chain(episodes)
            .collect();
        for id in all {
            unmatch_one(&transaction, id).await.map_err(storage)?;
        }
        transaction.commit().await.map_err(storage)
    }

    async fn images_of(&self, items: &[RatingKey]) -> Result<Vec<ShownImage>, SourceMetadataError> {
        let mut shown = Vec::new();
        for chunk in items.chunks(INSERT_CHUNK) {
            let rows = item_image::Entity::find()
                .filter(item_image::Column::ItemId.is_in(chunk.iter().map(|item| item.get())))
                .find_also_related(image_file::Entity)
                .all(&self.connection)
                .await
                .map_err(storage)?;
            shown.extend(rows.into_iter().filter_map(|(image, file)| {
                let file = file?;
                Some(ShownImage {
                    item: RatingKey::new(image.item_id),
                    kind: ImageKind::parse(&image.kind)?,
                    content_type: file.content_type,
                    width: u32::try_from(file.width).ok()?,
                    height: u32::try_from(file.height).ok()?,
                    blur_hash: file.blur_hash,
                    file: file.file,
                    chosen_at: image.chosen_at,
                })
            }));
        }
        Ok(shown)
    }
}

impl SqlSourceMetadataRepository {
    async fn children_of(
        &self,
        parents: impl Iterator<Item = i64>,
    ) -> Result<Vec<metadata_item::Model>, SourceMetadataError> {
        let parents: Vec<i64> = parents.collect();
        if parents.is_empty() {
            return Ok(Vec::new());
        }
        let mut found = Vec::new();
        for chunk in parents.chunks(INSERT_CHUNK) {
            found.extend(
                metadata_item::Entity::find()
                    .filter(metadata_item::Column::ParentId.is_in(chunk.iter().copied()))
                    .all(&self.connection)
                    .await
                    .map_err(storage)?,
            );
        }
        Ok(found)
    }

    /// Films or series with their seasons and episodes, what their edits locked and the record
    /// each shows.
    async fn targets(
        &self,
        top: Vec<metadata_item::Model>,
    ) -> Result<Vec<MatchTarget>, SourceMetadataError> {
        let seasons = self.children_of(top.iter().map(|row| row.id)).await?;
        let episodes = self.children_of(seasons.iter().map(|row| row.id)).await?;
        let mut linked = self.linked(top.iter().map(|row| row.id)).await?;
        let mut locks = self
            .locks(
                top.iter()
                    .chain(&seasons)
                    .chain(&episodes)
                    .map(|row| row.id),
            )
            .await?;
        let mut target = |row: metadata_item::Model| Target {
            locked: locks.remove(&row.id).unwrap_or_default(),
            item: to_domain(row),
        };

        let mut episodes_by_season: HashMap<i64, Vec<Target>> = HashMap::new();
        for row in episodes {
            if let Some(season) = row.parent_id {
                episodes_by_season
                    .entry(season)
                    .or_default()
                    .push(target(row));
            }
        }
        let mut seasons_by_show: HashMap<i64, Vec<SeasonTarget>> = HashMap::new();
        for row in seasons {
            if let Some(show) = row.parent_id {
                let episodes = episodes_by_season.remove(&row.id).unwrap_or_default();
                seasons_by_show.entry(show).or_default().push(SeasonTarget {
                    target: target(row),
                    episodes,
                });
            }
        }
        Ok(top
            .into_iter()
            .map(|row| MatchTarget {
                seasons: seasons_by_show.remove(&row.id).unwrap_or_default(),
                linked: linked.remove(&row.id),
                target: target(row),
            })
            .collect())
    }

    /// The record each of these items shows, if any.
    async fn linked(
        &self,
        items: impl Iterator<Item = i64>,
    ) -> Result<HashMap<i64, Linked>, SourceMetadataError> {
        let items: Vec<i64> = items.collect();
        let mut found = HashMap::new();
        for chunk in items.chunks(INSERT_CHUNK) {
            let links = item_source::Entity::find()
                .filter(item_source::Column::ItemId.is_in(chunk.iter().copied()))
                .all(&self.connection)
                .await
                .map_err(storage)?;
            let records: HashMap<i64, source_metadata::Model> = source_metadata::Entity::find()
                .filter(
                    source_metadata::Column::Id
                        .is_in(links.iter().map(|link| link.source_metadata_id)),
                )
                .all(&self.connection)
                .await
                .map_err(storage)?
                .into_iter()
                .map(|record| (record.id, record))
                .collect();
            for link in links {
                if let Some(record) = records.get(&link.source_metadata_id) {
                    found.insert(
                        link.item_id,
                        Linked {
                            agent: record.agent.clone(),
                            provider_id: record.provider_id.clone(),
                            score: link.score,
                        },
                    );
                }
            }
        }
        Ok(found)
    }

    async fn locks(
        &self,
        items: impl Iterator<Item = i64>,
    ) -> Result<HashMap<i64, BTreeSet<ItemField>>, SourceMetadataError> {
        let items: Vec<i64> = items.collect();
        let mut locks: HashMap<i64, BTreeSet<ItemField>> = HashMap::new();
        for chunk in items.chunks(INSERT_CHUNK) {
            let rows = item_field_lock::Entity::find()
                .filter(item_field_lock::Column::ItemId.is_in(chunk.iter().copied()))
                .all(&self.connection)
                .await
                .map_err(storage)?;
            for row in rows {
                if let Some(field) = write::parse_field_name(&row.field) {
                    locks.entry(row.item_id).or_default().insert(field);
                }
            }
        }
        Ok(locks)
    }
}

/// Writes one record over the one stored under its provider id, and returns its id. Its texts,
/// identifiers, ratings and images are replaced whole: a provider's answer is the whole truth.
async fn save_row<C: sea_orm::ConnectionTrait>(
    connection: &C,
    record: &SourceRecord,
    parent: Option<i64>,
    at: i64,
) -> Result<i64, DbErr> {
    let existing = source_metadata::Entity::find()
        .filter(source_metadata::Column::Agent.eq(record.agent.as_str()))
        .filter(source_metadata::Column::Kind.eq(kind_name(record.kind)))
        .filter(source_metadata::Column::ProviderId.eq(record.provider_id.as_str()))
        .one(connection)
        .await?;
    let mut row = source_metadata::ActiveModel {
        agent: ActiveValue::Set(record.agent.clone()),
        kind: ActiveValue::Set(kind_name(record.kind).to_owned()),
        provider_id: ActiveValue::Set(record.provider_id.clone()),
        parent_id: ActiveValue::Set(parent),
        position: ActiveValue::Set(record.position),
        original_language: ActiveValue::Set(record.original_language.map(|tag| tag.to_string())),
        original_title: ActiveValue::Set(record.original_title.clone()),
        year: ActiveValue::Set(record.year),
        released_on: ActiveValue::Set(record.released_on),
        duration_ms: ActiveValue::Set(record.duration_ms),
        studio: ActiveValue::Set(record.studio.clone()),
        rating: ActiveValue::Set(record.rating),
        refreshed_at: ActiveValue::Set(at),
        ..Default::default()
    };
    let id = match existing {
        Some(existing) => {
            row.id = ActiveValue::Unchanged(existing.id);
            row.update(connection).await?.id
        }
        None => row.insert(connection).await?.id,
    };

    source_metadata_translation::Entity::delete_many()
        .filter(source_metadata_translation::Column::SourceMetadataId.eq(id))
        .exec(connection)
        .await?;
    source_metadata_external_id::Entity::delete_many()
        .filter(source_metadata_external_id::Column::SourceMetadataId.eq(id))
        .exec(connection)
        .await?;
    source_metadata_certification::Entity::delete_many()
        .filter(source_metadata_certification::Column::SourceMetadataId.eq(id))
        .exec(connection)
        .await?;
    source_metadata_image::Entity::delete_many()
        .filter(source_metadata_image::Column::SourceMetadataId.eq(id))
        .exec(connection)
        .await?;

    let translations: Vec<_> = dedup_by_key(&record.translations, |t| t.language)
        .map(|t| source_metadata_translation::ActiveModel {
            source_metadata_id: ActiveValue::Set(id),
            language: ActiveValue::Set(t.language.to_string()),
            title: ActiveValue::Set(t.title.clone()),
            summary: ActiveValue::Set(t.summary.clone()),
            tagline: ActiveValue::Set(t.tagline.clone()),
        })
        .collect();
    insert_chunks(connection, translations).await?;

    let own = ExternalId {
        source: record.agent.clone(),
        value: record.provider_id.clone(),
    };
    let ids: BTreeSet<&ExternalId> = record.external_ids.iter().chain([&own]).collect();
    let ids: Vec<_> = ids
        .into_iter()
        .map(|external| source_metadata_external_id::ActiveModel {
            source_metadata_id: ActiveValue::Set(id),
            source: ActiveValue::Set(external.source.clone()),
            value: ActiveValue::Set(external.value.clone()),
        })
        .collect();
    insert_chunks(connection, ids).await?;

    let certifications: Vec<_> = dedup_by_key(&record.certifications, |c| c.country.as_str())
        .filter(|c| c.country.len() == 2)
        .map(|c| source_metadata_certification::ActiveModel {
            source_metadata_id: ActiveValue::Set(id),
            country: ActiveValue::Set(c.country.to_ascii_uppercase()),
            rating: ActiveValue::Set(c.rating.clone()),
        })
        .collect();
    insert_chunks(connection, certifications).await?;

    let mut seen = BTreeSet::new();
    let images: Vec<_> = record
        .images
        .iter()
        .filter(|image| seen.insert((image.kind, image.url.as_str())))
        .zip(0..)
        .map(|(image, position)| source_metadata_image::ActiveModel {
            source_metadata_id: ActiveValue::Set(id),
            kind: ActiveValue::Set(image.kind.as_str().to_owned()),
            url: ActiveValue::Set(image.url.clone()),
            language: ActiveValue::Set(image.language.map(|tag| tag.to_string())),
            width: ActiveValue::Set(image.width),
            height: ActiveValue::Set(image.height),
            vote_average: ActiveValue::Set(image.vote_average),
            vote_count: ActiveValue::Set(image.vote_count),
            position: ActiveValue::Set(position),
        })
        .collect();
    insert_chunks(connection, images).await?;
    Ok(id)
}

/// The first of each key, in order: a provider repeats a language or a country now and then,
/// which the primary keys would refuse.
fn dedup_by_key<'a, T, K: Ord + 'a>(
    items: &'a [T],
    key: impl Fn(&'a T) -> K + 'a,
) -> impl Iterator<Item = &'a T> + 'a {
    let mut seen = BTreeSet::new();
    items.iter().filter(move |item| seen.insert(key(item)))
}

async fn insert_chunks<C, A>(connection: &C, rows: Vec<A>) -> Result<(), DbErr>
where
    C: sea_orm::ConnectionTrait,
    A: ActiveModelTrait + Send,
{
    for chunk in rows.chunks(INSERT_CHUNK) {
        <A::Entity as EntityTrait>::insert_many(chunk.to_vec())
            .exec(connection)
            .await?;
    }
    Ok(())
}

async fn apply_item<C: sea_orm::ConnectionTrait>(
    connection: &C,
    applied: &AppliedItem,
    at: i64,
) -> Result<(), DbErr> {
    let item: &MetadataItem = &applied.item;
    let id = item.id.get();
    write::update_item(connection, item, &applied.locked).await?;
    metadata_item::Entity::update_many()
        .col_expr(metadata_item::Column::Guid, Expr::value(item.guid.as_str()))
        .col_expr(metadata_item::Column::RefreshedAt, Expr::value(at))
        .col_expr(metadata_item::Column::UpdatedAt, Expr::value(at))
        .filter(metadata_item::Column::Id.eq(id))
        .exec(connection)
        .await?;

    item_source::Entity::delete_by_id(id)
        .exec(connection)
        .await?;
    item_source::ActiveModel {
        item_id: ActiveValue::Set(id),
        source_metadata_id: ActiveValue::Set(applied.source_id),
        score: ActiveValue::Set(applied.score),
        matched_at: ActiveValue::Set(at),
    }
    .insert(connection)
    .await?;

    item_external_id::Entity::delete_many()
        .filter(item_external_id::Column::ItemId.eq(id))
        .exec(connection)
        .await?;
    // Providers repeat an identifier now and then, which the primary key refuses.
    let mut seen = BTreeSet::new();
    let ids: Vec<_> = applied
        .external_ids
        .iter()
        .filter(|external| seen.insert(*external))
        // The ones Plex publishes as `<Guid>`; a season's own id is not among them.
        .filter(|external| matches!(external.source.as_str(), "imdb" | "tmdb" | "tvdb"))
        .filter(|_| item.kind != ItemKind::Season)
        .zip(0..)
        .map(|(external, position)| item_external_id::ActiveModel {
            item_id: ActiveValue::Set(id),
            source: ActiveValue::Set(external.source.clone()),
            value: ActiveValue::Set(external.value.clone()),
            position: ActiveValue::Set(position),
        })
        .collect();
    insert_chunks(connection, ids).await?;

    for (kind, tags) in &applied.tags {
        replace_tags(connection, id, *kind, tags).await?;
    }
    if let Some(ratings) = &applied.ratings {
        replace_ratings(connection, id, ratings).await?;
    }

    if applied.previous_guid != item.guid {
        move_viewer_state(connection, &applied.previous_guid, &item.guid).await?;
    }
    Ok(())
}

/// One item back to what a scan gave it, its texts aside.
async fn unmatch_one<C: sea_orm::ConnectionTrait>(connection: &C, id: i64) -> Result<(), DbErr> {
    let Some(row) = metadata_item::Entity::find_by_id(id)
        .one(connection)
        .await?
    else {
        return Ok(());
    };
    item_source::Entity::delete_by_id(id)
        .exec(connection)
        .await?;
    item_image::Entity::delete_many()
        .filter(item_image::Column::ItemId.eq(id))
        .exec(connection)
        .await?;
    item_external_id::Entity::delete_many()
        .filter(item_external_id::Column::ItemId.eq(id))
        .exec(connection)
        .await?;
    let local = format!("local://{id}");
    if row.guid != local {
        metadata_item::Entity::update_many()
            .col_expr(metadata_item::Column::Guid, Expr::value(local.as_str()))
            .filter(metadata_item::Column::Id.eq(id))
            .exec(connection)
            .await?;
        move_viewer_state(connection, &row.guid, &local).await?;
    }
    Ok(())
}

/// Records a stored image and makes it the item's of its kind. The item's backdrop colors are
/// its background's, else its poster's, as the reference draws them from the art and falls
/// back on the poster.
async fn save_image<C: sea_orm::ConnectionTrait>(
    connection: &C,
    image: &ItemImage,
    at: i64,
) -> Result<(), DbErr> {
    let stored = &image.stored;
    let known = image_file::Entity::find_by_id(stored.file.as_str())
        .one(connection)
        .await?
        .is_some();
    if !known {
        image_file::ActiveModel {
            file: ActiveValue::Set(stored.file.clone()),
            source_url: ActiveValue::Set(image.url.clone()),
            content_type: ActiveValue::Set(stored.content_type.clone()),
            width: ActiveValue::Set(i32::try_from(stored.width).unwrap_or(i32::MAX)),
            height: ActiveValue::Set(i32::try_from(stored.height).unwrap_or(i32::MAX)),
            size_bytes: ActiveValue::Set(i64::try_from(stored.size_bytes).unwrap_or(i64::MAX)),
            blur_hash: ActiveValue::Set(stored.blur_hash.clone()),
            stored_at: ActiveValue::Set(at),
        }
        .insert(connection)
        .await?;
    }
    let item = image.item.get();
    let kind = image.kind.as_str().to_owned();
    let current = item_image::Entity::find_by_id((item, kind.clone()))
        .one(connection)
        .await?;
    // The same file again keeps its time, and so the address clients cached it under.
    if current
        .as_ref()
        .is_none_or(|current| current.file != stored.file)
    {
        item_image::Entity::delete_by_id((item, kind.clone()))
            .exec(connection)
            .await?;
        item_image::ActiveModel {
            item_id: ActiveValue::Set(item),
            kind: ActiveValue::Set(kind),
            file: ActiveValue::Set(stored.file.clone()),
            chosen_at: ActiveValue::Set(at),
        }
        .insert(connection)
        .await?;
    }

    let draws_colors = match image.kind {
        ImageKind::Background => true,
        ImageKind::Poster => {
            item_image::Entity::find_by_id((item, ImageKind::Background.as_str().to_owned()))
                .one(connection)
                .await?
                .is_none()
        }
        ImageKind::ClearLogo | ImageKind::Banner | ImageKind::Thumb => false,
    };
    if let (true, Some(colors)) = (draws_colors, stored.corner_colors) {
        let [top_left, top_right, bottom_right, bottom_left] =
            colors.map(|color| i32::try_from(color).unwrap_or_default());
        item_corner_colors::Entity::delete_by_id(item)
            .exec(connection)
            .await?;
        item_corner_colors::ActiveModel {
            item_id: ActiveValue::Set(item),
            top_left: ActiveValue::Set(top_left),
            top_right: ActiveValue::Set(top_right),
            bottom_right: ActiveValue::Set(bottom_right),
            bottom_left: ActiveValue::Set(bottom_left),
        }
        .insert(connection)
        .await?;
    }
    Ok(())
}

/// Replaces an item's tags of one kind. A genre or a country is the tag of that name; a credit
/// is its person's, found by provider key, so that namesakes stay apart and a credit the
/// importer made for the same person is not duplicated.
async fn replace_tags<C: sea_orm::ConnectionTrait>(
    connection: &C,
    item: i64,
    kind: TagKind,
    tags: &[AppliedTag],
) -> Result<(), DbErr> {
    let kind_name = tag::kind_name(kind);
    item_tag::Entity::delete_many()
        .filter(item_tag::Column::ItemId.eq(item))
        .filter(
            item_tag::Column::TagId.in_subquery(
                Query::select()
                    .column(tag::Column::Id)
                    .from(tag::Entity)
                    .and_where(tag::Column::Kind.eq(kind_name))
                    .to_owned(),
            ),
        )
        .exec(connection)
        .await?;
    let mut rows = Vec::with_capacity(tags.len());
    for (applied, position) in tags.iter().zip(0..) {
        let tag_id = match &applied.person {
            None => plain_tag(connection, kind_name, &applied.name).await?,
            Some((key, photo)) => {
                let person = person_id(connection, &applied.name, key, photo.as_deref()).await?;
                credit_tag(connection, kind_name, &applied.name, person).await?
            }
        };
        rows.push(item_tag::ActiveModel {
            item_id: ActiveValue::Set(item),
            tag_id: ActiveValue::Set(tag_id),
            position: ActiveValue::Set(position),
            role: ActiveValue::Set(applied.role.clone()),
        });
    }
    insert_chunks(connection, rows).await
}

async fn plain_tag<C: sea_orm::ConnectionTrait>(
    connection: &C,
    kind: &str,
    name: &str,
) -> Result<i64, DbErr> {
    let found = tag::Entity::find()
        .filter(tag::Column::Kind.eq(kind))
        .filter(tag::Column::Name.eq(name))
        .filter(tag::Column::PersonId.is_null())
        .one(connection)
        .await?;
    match found {
        Some(found) => Ok(found.id),
        None => new_tag(connection, kind, name, None).await,
    }
}

async fn credit_tag<C: sea_orm::ConnectionTrait>(
    connection: &C,
    kind: &str,
    name: &str,
    person: i64,
) -> Result<i64, DbErr> {
    let found = tag::Entity::find()
        .filter(tag::Column::Kind.eq(kind))
        .filter(tag::Column::PersonId.eq(person))
        .one(connection)
        .await?;
    match found {
        Some(found) => Ok(found.id),
        None => new_tag(connection, kind, name, Some(person)).await,
    }
}

/// A new tag numbered past every existing one: imported tags keep Plex's numbers, which
/// clients hold in filters (`genre=38`).
async fn new_tag<C: sea_orm::ConnectionTrait>(
    connection: &C,
    kind: &str,
    name: &str,
    person: Option<i64>,
) -> Result<i64, DbErr> {
    let id = next_id::<_, tag::Entity>(connection, tag::Column::Id).await?;
    write::insert_tags(
        connection,
        vec![tag::ActiveModel {
            id: ActiveValue::Set(id),
            kind: ActiveValue::Set(kind.to_owned()),
            name: ActiveValue::Set(name.to_owned()),
            person_id: ActiveValue::Set(person),
        }],
    )
    .await?;
    Ok(id)
}

/// The person a provider knows by `key`, created on first credit. A portrait the provider
/// offers fills one the person lacks; one already there is kept.
async fn person_id<C: sea_orm::ConnectionTrait>(
    connection: &C,
    name: &str,
    key: &str,
    photo: Option<&str>,
) -> Result<i64, DbErr> {
    let found = person::Entity::find()
        .filter(person::Column::ProviderKey.eq(key))
        .one(connection)
        .await?;
    if let Some(found) = found {
        if found.thumb_url.is_none() && photo.is_some() {
            person::Entity::update_many()
                .col_expr(person::Column::ThumbUrl, Expr::value(photo))
                .filter(person::Column::Id.eq(found.id))
                .exec(connection)
                .await?;
        }
        return Ok(found.id);
    }
    let id = next_id::<_, person::Entity>(connection, person::Column::Id).await?;
    person::ActiveModel {
        id: ActiveValue::Set(id),
        name: ActiveValue::Set(name.to_owned()),
        provider_key: ActiveValue::Set(Some(key.to_owned())),
        thumb_url: ActiveValue::Set(photo.map(str::to_owned)),
    }
    .insert(connection)
    .await?;
    Ok(id)
}

async fn next_id<C, E>(connection: &C, column: E::Column) -> Result<i64, DbErr>
where
    C: sea_orm::ConnectionTrait,
    E: EntityTrait,
{
    let highest: Option<Option<i64>> = E::find()
        .select_only()
        .column_as(Expr::col(column).max(), "highest")
        .into_tuple()
        .one(connection)
        .await?;
    Ok(highest.flatten().unwrap_or(0) + 1)
}

async fn replace_ratings<C: sea_orm::ConnectionTrait>(
    connection: &C,
    item: i64,
    ratings: &[ProviderRating],
) -> Result<(), DbErr> {
    item_rating::Entity::delete_many()
        .filter(item_rating::Column::ItemId.eq(item))
        .exec(connection)
        .await?;
    let mut seen = BTreeSet::new();
    let rows: Vec<_> = ratings
        .iter()
        .filter(|rating| seen.insert((rating.source.as_str(), item_rating::kind_name(rating.kind))))
        .zip(0..)
        .map(|(rating, position)| item_rating::ActiveModel {
            item_id: ActiveValue::Set(item),
            source: ActiveValue::Set(rating.source.clone()),
            kind: ActiveValue::Set(item_rating::kind_name(rating.kind).to_owned()),
            value: ActiveValue::Set(rating.value),
            image: ActiveValue::Set(rating.image.clone()),
            position: ActiveValue::Set(position),
        })
        .collect();
    insert_chunks(connection, rows).await
}

/// Hands the viewers' state of a work from the guid an item left to the one it took, unless
/// another item still carries the old one: that copy keeps it. A viewer who already has state
/// under the new guid keeps that.
async fn move_viewer_state<C: sea_orm::ConnectionTrait>(
    connection: &C,
    from: &str,
    to: &str,
) -> Result<(), DbErr> {
    let still_carried = metadata_item::Entity::find()
        .filter(metadata_item::Column::Guid.eq(from))
        .one(connection)
        .await?
        .is_some();
    if still_carried {
        return Ok(());
    }
    move_rows(
        connection,
        watch_state::Entity,
        watch_state::Column::Viewer,
        watch_state::Column::Guid,
        from,
        to,
    )
    .await?;
    move_rows(
        connection,
        user_rating::Entity,
        user_rating::Column::Viewer,
        user_rating::Column::Guid,
        from,
        to,
    )
    .await?;
    move_rows(
        connection,
        user_favorite::Entity,
        user_favorite::Column::Viewer,
        user_favorite::Column::Guid,
        from,
        to,
    )
    .await
}

async fn move_rows<C, E>(
    connection: &C,
    entity: E,
    viewer: E::Column,
    guid: E::Column,
    from: &str,
    to: &str,
) -> Result<(), DbErr>
where
    C: sea_orm::ConnectionTrait,
    E: EntityTrait,
{
    let holders = Query::select()
        .column(viewer)
        .from(Alias::new(entity.table_name()))
        .and_where(guid.eq(to))
        .to_owned();
    E::update_many()
        .col_expr(guid, Expr::value(to))
        .filter(guid.eq(from))
        .filter(viewer.not_in_subquery(holders))
        .exec(connection)
        .await?;
    E::delete_many()
        .filter(guid.eq(from))
        .exec(connection)
        .await?;
    Ok(())
}
