//! Markers in storage: the `item_marker` table, and how a marker's kind and owner are
//! written there.
//!
//! Its own module because both the repository and the importer write the table, and they must
//! agree on the encoding.

use open_plex_core::marker::{Marker, MarkerKind, NewMarker};
use open_plex_core::watch::Viewer;
use open_plex_core::{MarkerId, RatingKey, UserId};
use sea_orm::{
    ActiveValue, ColumnTrait, Condition, ConnectionTrait, DbErr, EntityTrait, QueryFilter,
    QueryOrder, TransactionTrait,
};

use crate::entity::item_marker;
use crate::watch_repository::viewer_id;

/// How a kind is stored.
pub const fn kind_name(kind: MarkerKind) -> &'static str {
    match kind {
        MarkerKind::Intro => "intro",
        MarkerKind::Credits => "credits",
        MarkerKind::Commercial => "commercial",
        MarkerKind::Bookmark => "bookmark",
    }
}

fn parse_kind(name: &str) -> Option<MarkerKind> {
    match name {
        "intro" => Some(MarkerKind::Intro),
        "credits" => Some(MarkerKind::Credits),
        "commercial" => Some(MarkerKind::Commercial),
        "bookmark" => Some(MarkerKind::Bookmark),
        _ => None,
    }
}

fn owner_of(stored: i64) -> Viewer {
    if stored == 0 {
        Viewer::Operator
    } else {
        Viewer::User(UserId::new(stored))
    }
}

fn to_domain(row: item_marker::Model) -> Option<Marker> {
    let Some(kind) = parse_kind(&row.kind) else {
        tracing::warn!(
            kind = row.kind,
            id = row.id,
            "unknown marker kind in storage"
        );
        return None;
    };
    Some(Marker {
        id: MarkerId::new(row.id),
        item: RatingKey::new(row.item_id),
        kind,
        owner: row.owner.map(owner_of),
        start_ms: row.start_ms,
        end_ms: row.end_ms,
        is_final: row.is_final,
        version: row.version,
        title: row.title,
        color: row.color,
    })
}

/// The rows of the work's markers to insert, as the importer reads them.
pub(crate) fn work_marker(
    item: i64,
    kind: MarkerKind,
    start_ms: i64,
    end_ms: i64,
    is_final: bool,
    version: Option<i32>,
) -> item_marker::ActiveModel {
    item_marker::ActiveModel {
        id: ActiveValue::NotSet,
        item_id: ActiveValue::Set(item),
        owner: ActiveValue::Set(None),
        kind: ActiveValue::Set(kind_name(kind).to_owned()),
        start_ms: ActiveValue::Set(start_ms),
        end_ms: ActiveValue::Set(Some(end_ms)),
        is_final: ActiveValue::Set(is_final),
        version: ActiveValue::Set(version),
        title: ActiveValue::Set(None),
        color: ActiveValue::Set(None),
    }
}

/// Deletes the work's markers of every item, leaving the viewers' bookmarks.
pub(crate) async fn delete_work_markers<C: ConnectionTrait>(connection: &C) -> Result<u64, DbErr> {
    Ok(item_marker::Entity::delete_many()
        .filter(item_marker::Column::Owner.is_null())
        .exec(connection)
        .await?
        .rows_affected)
}

pub(crate) async fn markers<C: ConnectionTrait>(
    connection: &C,
    items: &[RatingKey],
    viewer: Option<Viewer>,
) -> Result<Vec<Marker>, DbErr> {
    if items.is_empty() {
        return Ok(Vec::new());
    }
    let mut owners = Condition::any().add(item_marker::Column::Owner.is_null());
    if let Some(viewer) = viewer {
        owners = owners.add(item_marker::Column::Owner.eq(viewer_id(viewer)));
    }
    let rows = item_marker::Entity::find()
        .filter(item_marker::Column::ItemId.is_in(items.iter().map(|item| item.get())))
        .filter(owners)
        .order_by_asc(item_marker::Column::Id)
        .all(connection)
        .await?;
    Ok(rows.into_iter().filter_map(to_domain).collect())
}

pub(crate) async fn marker<C: ConnectionTrait>(
    connection: &C,
    id: MarkerId,
) -> Result<Option<Marker>, DbErr> {
    Ok(item_marker::Entity::find_by_id(id.get())
        .one(connection)
        .await?
        .and_then(to_domain))
}

pub(crate) async fn add<C: ConnectionTrait + TransactionTrait>(
    connection: &C,
    marker: &NewMarker,
) -> Result<(Marker, bool), DbErr> {
    let transaction = connection.begin().await?;
    let present = markers(&transaction, &[marker.item()], marker.owner()).await?;
    if let Some(present) = present
        .into_iter()
        .find(|present| marker.duplicates(present))
    {
        transaction.commit().await?;
        return Ok((present, false));
    }
    let values = marker.values();
    let id = item_marker::Entity::insert(item_marker::ActiveModel {
        id: ActiveValue::NotSet,
        item_id: ActiveValue::Set(marker.item().get()),
        owner: ActiveValue::Set(marker.owner().map(viewer_id)),
        kind: ActiveValue::Set(kind_name(marker.kind()).to_owned()),
        start_ms: ActiveValue::Set(values.start_ms),
        end_ms: ActiveValue::Set(values.end_ms),
        is_final: ActiveValue::Set(values.is_final),
        version: ActiveValue::Set(None),
        title: ActiveValue::Set(values.title.clone()),
        color: ActiveValue::Set(values.color.clone()),
    })
    .exec(&transaction)
    .await?
    .last_insert_id;
    transaction.commit().await?;
    let added = Marker {
        id: MarkerId::new(id),
        item: marker.item(),
        kind: marker.kind(),
        owner: marker.owner(),
        start_ms: values.start_ms,
        end_ms: values.end_ms,
        is_final: values.is_final,
        version: None,
        title: values.title.clone(),
        color: values.color.clone(),
    };
    Ok((added, true))
}

pub(crate) async fn update<C: ConnectionTrait>(
    connection: &C,
    marker: &Marker,
) -> Result<Option<Marker>, DbErr> {
    let updated = item_marker::Entity::update_many()
        .filter(item_marker::Column::Id.eq(marker.id.get()))
        .set(item_marker::ActiveModel {
            start_ms: ActiveValue::Set(marker.start_ms),
            end_ms: ActiveValue::Set(marker.end_ms),
            is_final: ActiveValue::Set(marker.is_final),
            version: ActiveValue::Set(marker.version),
            title: ActiveValue::Set(marker.title.clone()),
            color: ActiveValue::Set(marker.color.clone()),
            ..Default::default()
        })
        .exec(connection)
        .await?;
    if updated.rows_affected == 0 {
        return Ok(None);
    }
    self::marker(connection, marker.id).await
}

pub(crate) async fn delete<C: ConnectionTrait>(
    connection: &C,
    id: MarkerId,
) -> Result<bool, DbErr> {
    Ok(item_marker::Entity::delete_by_id(id.get())
        .exec(connection)
        .await?
        .rows_affected
        > 0)
}
