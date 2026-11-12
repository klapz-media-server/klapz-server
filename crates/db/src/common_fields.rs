//! What a set of items has in common, aggregated by the engine.
//!
//! One aggregate over the items for the scalars — per field, how many items hold a value,
//! how many distinct values there are, and the smallest — then one grouped query each for
//! tags, collections and locks, kept when every item carries them. Nothing is read item by
//! item, because the set can be a whole library.
//!
//! What every item carries, one item carries, so the grouped queries count only the first
//! item's tags, collections and locks: over a library's 79,518 episodes, a few dozen rows
//! to count rather than every credit of every episode.

use open_plex_core::common::{order_shared_tags, CommonFields, Shared, SharedTag};
use open_plex_core::edit::ItemField;
use open_plex_core::{CollectionId, RatingKey};
use sea_orm::sea_query::{Alias, Expr, Func, Query, SelectStatement};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, DbErr, EntityTrait, FromQueryResult,
    JoinType, QueryFilter, QueryOrder, QueryResult, QuerySelect, RelationTrait, TryGetable,
};

use crate::entity::{collection, collection_entry, item_field_lock, item_tag, metadata_item, tag};
use crate::metadata_repository::{with_tag_columns, TagRow};
use crate::write;

/// The scalar fields, by the alias their aggregates take and where they are read: the item,
/// its parent, or its grandparent.
const FIELDS: &[(&str, Generation, metadata_item::Column)] = &[
    ("id", Generation::Item, metadata_item::Column::Id),
    ("parent", Generation::Item, metadata_item::Column::ParentId),
    (
        "parent_title",
        Generation::Parent,
        metadata_item::Column::Title,
    ),
    (
        "grandparent",
        Generation::Parent,
        metadata_item::Column::ParentId,
    ),
    (
        "grandparent_title",
        Generation::Grandparent,
        metadata_item::Column::Title,
    ),
    ("title", Generation::Item, metadata_item::Column::Title),
    (
        "title_sort",
        Generation::Item,
        metadata_item::Column::TitleSort,
    ),
    (
        "original_title",
        Generation::Item,
        metadata_item::Column::OriginalTitle,
    ),
    ("tagline", Generation::Item, metadata_item::Column::Tagline),
    ("summary", Generation::Item, metadata_item::Column::Summary),
    ("studio", Generation::Item, metadata_item::Column::Studio),
    (
        "content_rating",
        Generation::Item,
        metadata_item::Column::ContentRating,
    ),
    ("year", Generation::Item, metadata_item::Column::Year),
    (
        "released_on",
        Generation::Item,
        metadata_item::Column::ReleasedOn,
    ),
    ("rating", Generation::Item, metadata_item::Column::Rating),
    (
        "position",
        Generation::Item,
        metadata_item::Column::Position,
    ),
];

#[derive(Debug, Clone, Copy)]
enum Generation {
    Item,
    Parent,
    Grandparent,
}

impl Generation {
    fn alias(self) -> Alias {
        Alias::new(match self {
            Self::Item => "item",
            Self::Parent => "parent",
            Self::Grandparent => "grandparent",
        })
    }
}

/// `items` is a subquery returning the ids of the set.
pub(crate) async fn common_fields(
    connection: &DatabaseConnection,
    items: SelectStatement,
) -> Result<CommonFields, DbErr> {
    let Some(row) = connection
        .query_one(
            connection
                .get_database_backend()
                .build(&aggregate(items.clone())),
        )
        .await?
    else {
        return Ok(CommonFields::empty());
    };
    let count: i64 = row.try_get("", "items")?;
    let Some(first) = row.try_get::<Option<i64>>("", "id_value")? else {
        return Ok(CommonFields::empty());
    };
    let set = Set { count, first };
    let read_key = |name: &str| -> Result<Shared<RatingKey>, DbErr> {
        Ok(match shared::<i64>(&row, name, count)? {
            Shared::Mixed => Shared::Mixed,
            Shared::Same(key) => Shared::Same(key.map(RatingKey::new)),
        })
    };

    Ok(CommonFields {
        items: u64::try_from(count).unwrap_or(0),
        id: read_key("id")?,
        parent: read_key("parent")?,
        parent_title: shared(&row, "parent_title", count)?,
        grandparent: read_key("grandparent")?,
        grandparent_title: shared(&row, "grandparent_title", count)?,
        title: shared(&row, "title", count)?,
        title_sort: shared(&row, "title_sort", count)?,
        original_title: shared(&row, "original_title", count)?,
        tagline: shared(&row, "tagline", count)?,
        summary: shared(&row, "summary", count)?,
        studio: shared(&row, "studio", count)?,
        content_rating: shared(&row, "content_rating", count)?,
        year: shared(&row, "year", count)?,
        released_on: shared(&row, "released_on", count)?,
        rating: shared(&row, "rating", count)?,
        position: shared(&row, "position", count)?,
        tags: shared_tags(connection, &set, items.clone()).await?,
        collections: shared_collections(connection, &set, items.clone()).await?,
        locked: shared_locks(connection, &set, items).await?,
    })
}

/// How many items a set holds, and the one with the smallest id.
#[derive(Debug, Clone, Copy)]
struct Set {
    count: i64,
    first: i64,
}

/// One row: the set's size, and three aggregates per field.
fn aggregate(items: SelectStatement) -> SelectStatement {
    let (item, parent, grandparent) = (
        Generation::Item.alias(),
        Generation::Parent.alias(),
        Generation::Grandparent.alias(),
    );
    let mut statement = Query::select();
    statement
        .from_as(metadata_item::Entity, item.clone())
        .join_as(
            JoinType::LeftJoin,
            metadata_item::Entity,
            parent.clone(),
            Expr::col((parent.clone(), metadata_item::Column::Id))
                .equals((item.clone(), metadata_item::Column::ParentId)),
        )
        .join_as(
            JoinType::LeftJoin,
            metadata_item::Entity,
            grandparent.clone(),
            Expr::col((grandparent, metadata_item::Column::Id))
                .equals((parent, metadata_item::Column::ParentId)),
        )
        .and_where(Expr::col((item.clone(), metadata_item::Column::Id)).in_subquery(items))
        .expr_as(
            Func::count(Expr::col((item, metadata_item::Column::Id))),
            Alias::new("items"),
        );
    for (name, generation, column) in FIELDS {
        let value = || Expr::col((generation.alias(), *column));
        statement
            .expr_as(Func::count(value()), Alias::new(format!("{name}_present")))
            .expr_as(
                Func::count_distinct(value()),
                Alias::new(format!("{name}_distinct")),
            )
            .expr_as(Func::min(value()), Alias::new(format!("{name}_value")));
    }
    statement
}

/// A field is mixed when two values differ, or when some items hold one and others none.
fn shared<T: TryGetable>(row: &QueryResult, name: &str, items: i64) -> Result<Shared<T>, DbErr> {
    let present: i64 = row.try_get("", &format!("{name}_present"))?;
    let distinct: i64 = row.try_get("", &format!("{name}_distinct"))?;
    if distinct > 1 || (present > 0 && present < items) {
        return Ok(Shared::Mixed);
    }
    Ok(Shared::Same(row.try_get("", &format!("{name}_value"))?))
}

#[derive(Debug, FromQueryResult)]
struct SharedTagRow {
    position: i32,
    role: Option<String>,
    #[sea_orm(nested)]
    tag: TagRow,
}

async fn shared_tags(
    connection: &DatabaseConnection,
    set: &Set,
    items: SelectStatement,
) -> Result<Vec<SharedTag>, DbErr> {
    let single = set.count == 1;
    let candidates = Query::select()
        .column(item_tag::Column::TagId)
        .from(item_tag::Entity)
        .and_where(Expr::col(item_tag::Column::ItemId).eq(set.first))
        .to_owned();
    let rows = with_tag_columns(item_tag::Entity::find().select_only())
        .expr_as(
            Func::min(Expr::col((item_tag::Entity, item_tag::Column::Position))),
            "position",
        )
        .expr_as(
            Func::min(Expr::col((item_tag::Entity, item_tag::Column::Role))),
            "role",
        )
        .join(JoinType::InnerJoin, item_tag::Relation::Tag.def())
        .join(JoinType::LeftJoin, tag::Relation::Person.def())
        .filter(item_tag::Column::ItemId.in_subquery(items))
        .filter(item_tag::Column::TagId.in_subquery(candidates))
        .group_by(tag::Column::Id)
        .group_by(tag::Column::Kind)
        .group_by(tag::Column::Name)
        .group_by(tag::Column::PersonId)
        .group_by(crate::entity::person::Column::Name)
        .group_by(crate::entity::person::Column::ProviderKey)
        .group_by(crate::entity::person::Column::ThumbUrl)
        .having(
            Expr::expr(Func::count_distinct(Expr::col((
                item_tag::Entity,
                item_tag::Column::ItemId,
            ))))
            .eq(set.count),
        )
        .into_model::<SharedTagRow>()
        .all(connection)
        .await?;
    let mut tags: Vec<SharedTag> = rows
        .into_iter()
        .filter_map(|row| {
            Some(SharedTag {
                tag: row.tag.into_tag()?,
                role: row.role.filter(|_| single),
                position: single.then_some(row.position),
            })
        })
        .collect();
    order_shared_tags(&mut tags);
    Ok(tags)
}

async fn shared_collections(
    connection: &DatabaseConnection,
    set: &Set,
    items: SelectStatement,
) -> Result<Vec<CollectionId>, DbErr> {
    let candidates = Query::select()
        .column(collection_entry::Column::CollectionId)
        .from(collection_entry::Entity)
        .and_where(Expr::col(collection_entry::Column::ItemId).eq(set.first))
        .to_owned();
    let ids: Vec<i64> = collection_entry::Entity::find()
        .select_only()
        .column(collection_entry::Column::CollectionId)
        .join(
            JoinType::InnerJoin,
            collection_entry::Relation::Collection.def(),
        )
        .filter(collection_entry::Column::ItemId.in_subquery(items))
        .filter(collection_entry::Column::CollectionId.in_subquery(candidates))
        .group_by(collection_entry::Column::CollectionId)
        .group_by(collection::Column::TagNumber)
        .having(
            Expr::expr(Func::count_distinct(Expr::col((
                collection_entry::Entity,
                collection_entry::Column::ItemId,
            ))))
            .eq(set.count),
        )
        .order_by_asc(collection::Column::TagNumber)
        .into_tuple()
        .all(connection)
        .await?;
    Ok(ids.into_iter().map(CollectionId::new).collect())
}

async fn shared_locks(
    connection: &DatabaseConnection,
    set: &Set,
    items: SelectStatement,
) -> Result<Vec<ItemField>, DbErr> {
    let candidates = Query::select()
        .column(item_field_lock::Column::Field)
        .from(item_field_lock::Entity)
        .and_where(Expr::col(item_field_lock::Column::ItemId).eq(set.first))
        .to_owned();
    let names: Vec<String> = item_field_lock::Entity::find()
        .select_only()
        .column(item_field_lock::Column::Field)
        .filter(item_field_lock::Column::ItemId.in_subquery(items))
        .filter(item_field_lock::Column::Field.in_subquery(candidates))
        .group_by(item_field_lock::Column::Field)
        .having(Expr::expr(Func::count(Expr::col(item_field_lock::Column::ItemId))).eq(set.count))
        .into_tuple()
        .all(connection)
        .await?;
    Ok(ItemField::ALL
        .into_iter()
        .filter(|field| names.iter().any(|name| *name == write::field_name(*field)))
        .collect())
}
