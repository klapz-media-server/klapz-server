//! Chapters in storage: the `item_chapter` table and an item's `chapter_source`.
//!
//! Its own module because the repository reads the table and both the importer and the scan
//! write it.

use open_plex_core::chapter::{Chapter, ChapterSource};
use open_plex_core::{ChapterId, MediaId, RatingKey};
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveValue, ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter, QueryOrder,
};

use crate::entity::{item_chapter, metadata_item};

/// Items per query, far inside every engine's parameter limit.
const ITEMS_PER_QUERY: usize = 500;

/// The chapters of these items, each item's in order.
pub async fn chapters<C: ConnectionTrait>(
    connection: &C,
    items: &[RatingKey],
) -> Result<Vec<Chapter>, DbErr> {
    let mut chapters = Vec::new();
    for chunk in items.chunks(ITEMS_PER_QUERY) {
        let rows = item_chapter::Entity::find()
            .filter(item_chapter::Column::ItemId.is_in(chunk.iter().map(|item| item.get())))
            .order_by_asc(item_chapter::Column::ItemId)
            .order_by_asc(item_chapter::Column::Position)
            .all(connection)
            .await?;
        chapters.extend(rows.into_iter().filter_map(to_domain));
    }
    Ok(chapters)
}

pub async fn chapter<C: ConnectionTrait>(
    connection: &C,
    id: ChapterId,
) -> Result<Option<Chapter>, DbErr> {
    Ok(item_chapter::Entity::find_by_id(id.get())
        .one(connection)
        .await?
        .and_then(to_domain))
}

fn to_domain(row: item_chapter::Model) -> Option<Chapter> {
    Some(Chapter {
        id: ChapterId::new(row.id),
        item: RatingKey::new(row.item_id),
        index: u32::try_from(row.position).ok()?,
        start_ms: row.start_ms,
        end_ms: row.end_ms,
        title: row.title,
        image_version: row.image_media_id.map(MediaId::new),
    })
}

/// A chapter as written, its id left to the engine.
pub fn row(
    item: i64,
    position: i32,
    start_ms: i64,
    end_ms: i64,
    title: Option<String>,
    image_media_id: Option<i64>,
) -> item_chapter::ActiveModel {
    item_chapter::ActiveModel {
        id: ActiveValue::NotSet,
        item_id: ActiveValue::Set(item),
        position: ActiveValue::Set(position),
        start_ms: ActiveValue::Set(start_ms),
        end_ms: ActiveValue::Set(end_ms),
        title: ActiveValue::Set(title),
        image_media_id: ActiveValue::Set(image_media_id),
    }
}

/// Replaces an item's chapters, and says where they came from: nowhere when there are none.
pub async fn replace<C: ConnectionTrait>(
    connection: &C,
    item: i64,
    source: ChapterSource,
    chapters: Vec<item_chapter::ActiveModel>,
) -> Result<(), DbErr> {
    item_chapter::Entity::delete_many()
        .filter(item_chapter::Column::ItemId.eq(item))
        .exec(connection)
        .await?;
    let source = (!chapters.is_empty()).then(|| source.as_str());
    metadata_item::Entity::update_many()
        .col_expr(metadata_item::Column::ChapterSource, Expr::value(source))
        .filter(metadata_item::Column::Id.eq(item))
        .exec(connection)
        .await?;
    if !chapters.is_empty() {
        item_chapter::Entity::insert_many(chapters)
            .exec(connection)
            .await?;
    }
    Ok(())
}
