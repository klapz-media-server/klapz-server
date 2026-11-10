//! Storage adapter for watch state.

use std::collections::HashMap;

use open_plex_core::watch::{
    Viewer, Viewing, WatchError, WatchRepository, WatchState, VIEWING_LIFETIME_SECONDS,
};
use open_plex_core::RatingKey;
use sea_orm::sea_query::{Alias, Expr, OnConflict};
use sea_orm::{
    ColumnTrait, Condition, DatabaseConnection, EntityTrait, FromQueryResult, JoinType,
    QueryFilter, QueryOrder, QuerySelect, TransactionTrait,
};

use crate::entity::{metadata_item, viewing, watch_state};

#[derive(Debug, Clone)]
pub struct SqlWatchRepository {
    connection: DatabaseConnection,
}

impl SqlWatchRepository {
    pub const fn new(connection: DatabaseConnection) -> Self {
        Self { connection }
    }
}

/// The stored form of a viewer: 0 is the operator, who has no user row.
pub(crate) const fn viewer_id(viewer: Viewer) -> i64 {
    match viewer {
        Viewer::Operator => 0,
        Viewer::User(user) => user.get(),
    }
}

#[async_trait::async_trait]
impl WatchRepository for SqlWatchRepository {
    async fn states(
        &self,
        viewer: Viewer,
        guids: &[String],
    ) -> Result<Vec<(String, WatchState)>, WatchError> {
        if guids.is_empty() {
            return Ok(Vec::new());
        }
        let rows = watch_state::Entity::find()
            .filter(watch_state::Column::Viewer.eq(viewer_id(viewer)))
            .filter(watch_state::Column::Guid.is_in(guids.iter().cloned()))
            .all(&self.connection)
            .await
            .map_err(storage)?;
        Ok(rows.into_iter().map(to_state).collect())
    }

    async fn save(
        &self,
        viewer: Viewer,
        states: &[(String, WatchState)],
    ) -> Result<(), WatchError> {
        let viewer = viewer_id(viewer);
        let transaction = self.connection.begin().await.map_err(storage)?;
        let (empty, kept): (Vec<_>, Vec<_>) =
            states.iter().partition(|(_, state)| state.is_empty());
        if !empty.is_empty() {
            watch_state::Entity::delete_many()
                .filter(watch_state::Column::Viewer.eq(viewer))
                .filter(watch_state::Column::Guid.is_in(empty.iter().map(|(guid, _)| guid.clone())))
                .exec(&transaction)
                .await
                .map_err(storage)?;
        }
        if !kept.is_empty() {
            let rows = kept.iter().map(|(guid, state)| watch_state::ActiveModel {
                viewer: sea_orm::ActiveValue::Set(viewer),
                guid: sea_orm::ActiveValue::Set(guid.clone()),
                view_count: sea_orm::ActiveValue::Set(
                    i32::try_from(state.view_count).unwrap_or(i32::MAX),
                ),
                offset_ms: sea_orm::ActiveValue::Set(state.offset_ms),
                last_viewed_at: sea_orm::ActiveValue::Set(state.last_viewed_at),
            });
            watch_state::Entity::insert_many(rows)
                .on_conflict(
                    OnConflict::columns([watch_state::Column::Viewer, watch_state::Column::Guid])
                        .update_columns([
                            watch_state::Column::ViewCount,
                            watch_state::Column::OffsetMs,
                            watch_state::Column::LastViewedAt,
                        ])
                        .to_owned(),
                )
                .exec(&transaction)
                .await
                .map_err(storage)?;
        }
        transaction.commit().await.map_err(storage)
    }

    async fn watched_leaves(
        &self,
        viewer: Viewer,
        parents: &[RatingKey],
    ) -> Result<Vec<(RatingKey, u64)>, WatchError> {
        #[derive(FromQueryResult)]
        struct Count {
            parent: i64,
            count: i64,
        }

        if parents.is_empty() {
            return Ok(Vec::new());
        }
        let keys: Vec<i64> = parents.iter().map(|id| id.get()).collect();
        let watched = || {
            Condition::all()
                .add(
                    Expr::col((watch_state::Entity, watch_state::Column::Viewer))
                        .eq(viewer_id(viewer)),
                )
                .add(Expr::col((watch_state::Entity, watch_state::Column::ViewCount)).gt(0))
        };
        let joined = |select: sea_orm::Select<metadata_item::Entity>| {
            select.join(
                JoinType::InnerJoin,
                metadata_item::Entity::belongs_to(watch_state::Entity)
                    .from(metadata_item::Column::Guid)
                    .to(watch_state::Column::Guid)
                    .into(),
            )
        };

        // A season's episodes are its children; a series' are its grandchildren.
        let seasons: Vec<Count> = joined(metadata_item::Entity::find())
            .select_only()
            .column_as(metadata_item::Column::ParentId, "parent")
            .column_as(metadata_item::Column::Id.count(), "count")
            .filter(metadata_item::Column::ParentId.is_in(keys.clone()))
            .filter(metadata_item::Column::Kind.eq("episode"))
            .filter(watched())
            .group_by(metadata_item::Column::ParentId)
            .into_model()
            .all(&self.connection)
            .await
            .map_err(storage)?;

        let season = Alias::new("season");
        let shows: Vec<Count> = joined(metadata_item::Entity::find())
            .select_only()
            .column_as(
                Expr::col((season.clone(), metadata_item::Column::ParentId)),
                "parent",
            )
            .column_as(metadata_item::Column::Id.count(), "count")
            .join_as(
                JoinType::InnerJoin,
                metadata_item::Entity::belongs_to(metadata_item::Entity)
                    .from(metadata_item::Column::ParentId)
                    .to(metadata_item::Column::Id)
                    .into(),
                season.clone(),
            )
            .filter(Expr::col((season.clone(), metadata_item::Column::ParentId)).is_in(keys))
            .filter(metadata_item::Column::Kind.eq("episode"))
            .filter(watched())
            .group_by(Expr::col((season, metadata_item::Column::ParentId)))
            .into_model()
            .all(&self.connection)
            .await
            .map_err(storage)?;

        let mut counts: HashMap<i64, u64> = HashMap::new();
        for row in seasons.into_iter().chain(shows) {
            *counts.entry(row.parent).or_default() += u64::try_from(row.count).unwrap_or(0);
        }
        Ok(counts
            .into_iter()
            .map(|(parent, count)| (RatingKey::new(parent), count))
            .collect())
    }

    async fn recent(
        &self,
        viewer: Viewer,
        since: i64,
        limit: u32,
    ) -> Result<Vec<(String, WatchState)>, WatchError> {
        let rows = watch_state::Entity::find()
            .filter(watch_state::Column::Viewer.eq(viewer_id(viewer)))
            .filter(watch_state::Column::LastViewedAt.gte(since))
            .order_by_desc(watch_state::Column::LastViewedAt)
            .limit(u64::from(limit))
            .all(&self.connection)
            .await
            .map_err(storage)?;
        Ok(rows.into_iter().map(to_state).collect())
    }

    async fn viewing(
        &self,
        viewer: Viewer,
        playback: &str,
        now: i64,
    ) -> Result<Option<Viewing>, WatchError> {
        Ok(
            viewing::Entity::find_by_id((viewer_id(viewer), playback.to_owned()))
                .filter(viewing::Column::ExpiresAt.gt(now))
                .one(&self.connection)
                .await
                .map_err(storage)?
                .map(|row| Viewing {
                    item: RatingKey::new(row.item_id),
                    position_ms: row.position_ms,
                    completed: row.completed,
                }),
        )
    }

    async fn remember(
        &self,
        viewer: Viewer,
        playback: &str,
        remembered: Viewing,
        now: i64,
    ) -> Result<(), WatchError> {
        let viewer = viewer_id(viewer);
        let transaction = self.connection.begin().await.map_err(storage)?;
        // Anything of this viewer's that has expired goes now: playbacks are few per viewer,
        // and no sweep has to know about them.
        viewing::Entity::delete_many()
            .filter(viewing::Column::Viewer.eq(viewer))
            .filter(viewing::Column::ExpiresAt.lte(now))
            .exec(&transaction)
            .await
            .map_err(storage)?;
        viewing::Entity::insert(viewing::ActiveModel {
            viewer: sea_orm::ActiveValue::Set(viewer),
            playback: sea_orm::ActiveValue::Set(playback.to_owned()),
            item_id: sea_orm::ActiveValue::Set(remembered.item.get()),
            position_ms: sea_orm::ActiveValue::Set(remembered.position_ms),
            completed: sea_orm::ActiveValue::Set(remembered.completed),
            expires_at: sea_orm::ActiveValue::Set(now + VIEWING_LIFETIME_SECONDS),
        })
        .on_conflict(
            OnConflict::columns([viewing::Column::Viewer, viewing::Column::Playback])
                .update_columns([
                    viewing::Column::ItemId,
                    viewing::Column::PositionMs,
                    viewing::Column::Completed,
                    viewing::Column::ExpiresAt,
                ])
                .to_owned(),
        )
        .exec(&transaction)
        .await
        .map_err(storage)?;
        transaction.commit().await.map_err(storage)
    }
}

fn to_state(row: watch_state::Model) -> (String, WatchState) {
    let state = WatchState {
        view_count: u32::try_from(row.view_count).unwrap_or(0),
        offset_ms: row.offset_ms,
        last_viewed_at: row.last_viewed_at,
    };
    (row.guid, state)
}

fn storage(error: sea_orm::DbErr) -> WatchError {
    WatchError::Storage(Box::new(error))
}
