//! Storage adapter for watch state.

use std::collections::HashMap;

use open_plex_core::history::{
    HistoryEntry, HistoryOrder, HistoryQuery, HistoryScope, NewViewing, ViewingKind,
};
use open_plex_core::page::{Cursor, CursorKey, Page, Window};
use open_plex_core::tracks::{SubtitleChoice, TrackChoice};
use open_plex_core::user_rating::{RatingRecord, UserRating};
use open_plex_core::watch::{
    Viewing, WatchError, WatchRepository, WatchState, VIEWING_LIFETIME_SECONDS,
};
use open_plex_core::{HistoryId, PartId, RatingKey, SectionId, StreamId, UserId};
use sea_orm::sea_query::{Alias, Expr, OnConflict, Query, SimpleExpr};
use sea_orm::{
    ActiveValue, ColumnTrait, Condition, ConnectionTrait, DatabaseConnection, EntityTrait,
    FromQueryResult, JoinType, Order, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, Select,
    TransactionTrait,
};

use crate::entity::{
    metadata_item, track_choice, user_rating, viewing, viewing_history, watch_state,
};

#[derive(Debug, Clone)]
pub struct SqlWatchRepository {
    connection: DatabaseConnection,
}

impl SqlWatchRepository {
    pub const fn new(connection: DatabaseConnection) -> Self {
        Self { connection }
    }
}

#[async_trait::async_trait]
impl WatchRepository for SqlWatchRepository {
    async fn states(
        &self,
        viewer: UserId,
        guids: &[String],
    ) -> Result<Vec<(String, WatchState)>, WatchError> {
        if guids.is_empty() {
            return Ok(Vec::new());
        }
        let rows = watch_state::Entity::find()
            .filter(watch_state::Column::Viewer.eq(viewer.get()))
            .filter(watch_state::Column::Guid.is_in(guids.iter().cloned()))
            .all(&self.connection)
            .await
            .map_err(storage)?;
        Ok(rows.into_iter().map(to_state).collect())
    }

    async fn save(
        &self,
        viewer: UserId,
        states: &[(String, WatchState)],
    ) -> Result<(), WatchError> {
        let transaction = self.connection.begin().await.map_err(storage)?;
        save_states(&transaction, viewer.get(), states).await?;
        transaction.commit().await.map_err(storage)
    }

    async fn record_viewings(
        &self,
        viewer: UserId,
        states: &[(String, WatchState)],
        viewings: &[NewViewing],
    ) -> Result<(), WatchError> {
        let viewer = viewer.get();
        let transaction = self.connection.begin().await.map_err(storage)?;
        save_states(&transaction, viewer, states).await?;
        if !viewings.is_empty() {
            let rows = viewings.iter().map(|viewing| viewing_history::ActiveModel {
                id: ActiveValue::NotSet,
                viewer: ActiveValue::Set(viewer),
                item_id: ActiveValue::Set(viewing.item.get()),
                kind: ActiveValue::Set(kind_name(viewing.kind).to_owned()),
                viewed_at: ActiveValue::Set(viewing.viewed_at),
            });
            viewing_history::Entity::insert_many(rows)
                .exec(&transaction)
                .await
                .map_err(storage)?;
        }
        transaction.commit().await.map_err(storage)
    }

    async fn history(
        &self,
        query: &HistoryQuery,
        window: Window,
    ) -> Result<Page<HistoryEntry>, WatchError> {
        let key = order_key(query.order);
        let upward = !query.direction.is_descending();
        let mut select = history_select(query);
        if let Some(after) = &window.after {
            select = select.filter(beyond(key.as_ref(), after, upward));
        }
        if let Some(before) = &window.before {
            select = select.filter(beyond(key.as_ref(), before, !upward));
        }
        // A window taken from the end is read backwards, then put back in order.
        let order = if upward == window.is_reversed() {
            Order::Desc
        } else {
            Order::Asc
        };
        if let Some(key) = key {
            select = select.order_by(key, order.clone());
        }
        let rows: Vec<HistoryRow> = select
            .order_by(
                Expr::col((viewing_history::Entity, viewing_history::Column::Id)),
                order,
            )
            .offset(window.skip)
            .limit(window.fetch_limit())
            .into_model()
            .all(&self.connection)
            .await
            .map_err(storage)?;
        let mut page = Page::from_overfetch(rows, &window).map(HistoryRow::into_entry);
        if window.is_reversed() {
            page.items.reverse();
        }
        Ok(page)
    }

    async fn history_count(&self, query: &HistoryQuery) -> Result<u64, WatchError> {
        history_select(query)
            .count(&self.connection)
            .await
            .map_err(storage)
    }

    async fn history_entry(&self, id: HistoryId) -> Result<Option<HistoryEntry>, WatchError> {
        let row: Option<HistoryRow> = joined_history()
            .filter(Expr::col((viewing_history::Entity, viewing_history::Column::Id)).eq(id.get()))
            .into_model()
            .one(&self.connection)
            .await
            .map_err(storage)?;
        Ok(row.map(HistoryRow::into_entry))
    }

    async fn delete_history_entry(&self, id: HistoryId) -> Result<bool, WatchError> {
        let deleted = viewing_history::Entity::delete_by_id(id.get())
            .exec(&self.connection)
            .await
            .map_err(storage)?;
        Ok(deleted.rows_affected > 0)
    }

    async fn watched_leaves(
        &self,
        viewer: UserId,
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
                .add(Expr::col((watch_state::Entity, watch_state::Column::Viewer)).eq(viewer.get()))
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
        viewer: UserId,
        since: i64,
        limit: u32,
    ) -> Result<Vec<(String, WatchState)>, WatchError> {
        let rows = watch_state::Entity::find()
            .filter(watch_state::Column::Viewer.eq(viewer.get()))
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
        viewer: UserId,
        playback: &str,
        now: i64,
    ) -> Result<Option<Viewing>, WatchError> {
        Ok(
            viewing::Entity::find_by_id((viewer.get(), playback.to_owned()))
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
        viewer: UserId,
        playback: &str,
        remembered: Viewing,
        now: i64,
    ) -> Result<(), WatchError> {
        let viewer = viewer.get();
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

    async fn track_choices(
        &self,
        viewer: UserId,
        files: &[PartId],
    ) -> Result<Vec<(PartId, TrackChoice)>, WatchError> {
        if files.is_empty() {
            return Ok(Vec::new());
        }
        let rows = track_choice::Entity::find()
            .filter(track_choice::Column::Viewer.eq(viewer.get()))
            .filter(track_choice::Column::MediaPartId.is_in(files.iter().map(|file| file.get())))
            .all(&self.connection)
            .await
            .map_err(storage)?;
        Ok(rows
            .into_iter()
            .map(|row| {
                let subtitle = if row.subtitle_off {
                    Some(SubtitleChoice::Off)
                } else {
                    row.subtitle_stream_id
                        .map(|id| SubtitleChoice::Track(StreamId::new(id)))
                };
                let choice = TrackChoice {
                    audio: row.audio_stream_id.map(StreamId::new),
                    subtitle,
                };
                (PartId::new(row.media_part_id), choice)
            })
            .collect())
    }

    async fn choose_tracks(
        &self,
        viewer: UserId,
        file: PartId,
        choice: TrackChoice,
    ) -> Result<(), WatchError> {
        if choice.is_empty() {
            track_choice::Entity::delete_by_id((viewer.get(), file.get()))
                .exec(&self.connection)
                .await
                .map_err(storage)?;
            return Ok(());
        }
        let (subtitle_stream_id, subtitle_off) = match choice.subtitle {
            Some(SubtitleChoice::Track(id)) => (Some(id.get()), false),
            Some(SubtitleChoice::Off) => (None, true),
            None => (None, false),
        };
        track_choice::Entity::insert(track_choice::ActiveModel {
            viewer: ActiveValue::Set(viewer.get()),
            media_part_id: ActiveValue::Set(file.get()),
            audio_stream_id: ActiveValue::Set(choice.audio.map(StreamId::get)),
            subtitle_stream_id: ActiveValue::Set(subtitle_stream_id),
            subtitle_off: ActiveValue::Set(subtitle_off),
        })
        .on_conflict(
            OnConflict::columns([
                track_choice::Column::Viewer,
                track_choice::Column::MediaPartId,
            ])
            .update_columns([
                track_choice::Column::AudioStreamId,
                track_choice::Column::SubtitleStreamId,
                track_choice::Column::SubtitleOff,
            ])
            .to_owned(),
        )
        .exec(&self.connection)
        .await
        .map_err(storage)?;
        Ok(())
    }

    async fn ratings(
        &self,
        viewer: UserId,
        guids: &[String],
    ) -> Result<Vec<(String, RatingRecord)>, WatchError> {
        if guids.is_empty() {
            return Ok(Vec::new());
        }
        let rows = user_rating::Entity::find()
            .filter(user_rating::Column::Viewer.eq(viewer.get()))
            .filter(user_rating::Column::Guid.is_in(guids.iter().cloned()))
            .all(&self.connection)
            .await
            .map_err(storage)?;
        rows.into_iter()
            .map(|row| {
                // Written through `UserRating` alone, so a value off the scale is a corrupted
                // row rather than a rating to show.
                let rating = row
                    .rating
                    .map(UserRating::new)
                    .transpose()
                    .map_err(|error| WatchError::Storage(Box::new(error)))?;
                Ok((
                    row.guid,
                    RatingRecord {
                        rating,
                        at: row.rated_at,
                    },
                ))
            })
            .collect()
    }

    async fn save_rating(
        &self,
        viewer: UserId,
        guid: &str,
        record: RatingRecord,
    ) -> Result<(), WatchError> {
        user_rating::Entity::insert(user_rating::ActiveModel {
            viewer: ActiveValue::Set(viewer.get()),
            guid: ActiveValue::Set(guid.to_owned()),
            rating: ActiveValue::Set(record.rating.map(UserRating::get)),
            rated_at: ActiveValue::Set(record.at),
        })
        .on_conflict(
            OnConflict::columns([user_rating::Column::Viewer, user_rating::Column::Guid])
                .update_columns([user_rating::Column::Rating, user_rating::Column::RatedAt])
                .to_owned(),
        )
        .exec(&self.connection)
        .await
        .map_err(storage)?;
        Ok(())
    }
}

/// Writes states within the caller's transaction, an empty one deleting the row.
async fn save_states(
    connection: &impl ConnectionTrait,
    viewer: i64,
    states: &[(String, WatchState)],
) -> Result<(), WatchError> {
    let (empty, kept): (Vec<_>, Vec<_>) = states.iter().partition(|(_, state)| state.is_empty());
    if !empty.is_empty() {
        watch_state::Entity::delete_many()
            .filter(watch_state::Column::Viewer.eq(viewer))
            .filter(watch_state::Column::Guid.is_in(empty.iter().map(|(guid, _)| guid.clone())))
            .exec(connection)
            .await
            .map_err(storage)?;
    }
    if !kept.is_empty() {
        let rows = kept.iter().map(|(guid, state)| watch_state::ActiveModel {
            viewer: ActiveValue::Set(viewer),
            guid: ActiveValue::Set(guid.clone()),
            view_count: ActiveValue::Set(i32::try_from(state.view_count).unwrap_or(i32::MAX)),
            offset_ms: ActiveValue::Set(state.offset_ms),
            last_viewed_at: ActiveValue::Set(state.last_viewed_at),
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
            .exec(connection)
            .await
            .map_err(storage)?;
    }
    Ok(())
}

const fn kind_name(kind: ViewingKind) -> &'static str {
    match kind {
        ViewingKind::Played => "played",
        ViewingKind::Marked => "marked",
    }
}

/// An entry with its item's library, which the history is filtered and ordered by.
#[derive(Debug, FromQueryResult)]
struct HistoryRow {
    id: i64,
    viewer: i64,
    item_id: i64,
    kind: String,
    viewed_at: i64,
    section_id: i64,
}

impl HistoryRow {
    fn into_entry(self) -> HistoryEntry {
        HistoryEntry {
            id: HistoryId::new(self.id),
            viewer: UserId::new(self.viewer),
            item: RatingKey::new(self.item_id),
            section: SectionId::new(self.section_id),
            kind: if self.kind == kind_name(ViewingKind::Marked) {
                ViewingKind::Marked
            } else {
                ViewingKind::Played
            },
            viewed_at: self.viewed_at,
        }
    }
}

/// Entries joined to their items, whose library they are read with.
fn joined_history() -> Select<viewing_history::Entity> {
    viewing_history::Entity::find()
        .select_only()
        .columns([
            viewing_history::Column::Id,
            viewing_history::Column::Viewer,
            viewing_history::Column::ItemId,
            viewing_history::Column::Kind,
            viewing_history::Column::ViewedAt,
        ])
        .column_as(
            Expr::col((
                metadata_item::Entity,
                metadata_item::Column::LibrarySectionId,
            )),
            "section_id",
        )
        .join(
            JoinType::InnerJoin,
            viewing_history::Entity::belongs_to(metadata_item::Entity)
                .from(viewing_history::Column::ItemId)
                .to(metadata_item::Column::Id)
                .into(),
        )
}

fn history_select(query: &HistoryQuery) -> Select<viewing_history::Entity> {
    let mut select = joined_history();
    if let HistoryScope::Viewer(viewer) = query.scope {
        select = select.filter(viewing_history::Column::Viewer.eq(viewer.get()));
    }
    if let Some(item) = query.item.map(RatingKey::get) {
        let parent = || Expr::col((metadata_item::Entity, metadata_item::Column::ParentId));
        // A season's episodes are its children, a series' its grandchildren.
        let seasons = Query::select()
            .column(metadata_item::Column::Id)
            .from(metadata_item::Entity)
            .and_where(metadata_item::Column::ParentId.eq(item))
            .to_owned();
        select = select.filter(
            Condition::any()
                .add(viewing_history::Column::ItemId.eq(item))
                .add(parent().eq(item))
                .add(parent().in_subquery(seasons)),
        );
    }
    if let Some(sections) = &query.sections {
        select = select.filter(
            Expr::col((
                metadata_item::Entity,
                metadata_item::Column::LibrarySectionId,
            ))
            .is_in(sections.iter().map(|section| section.get())),
        );
    }
    if let Some(kind) = query.kind {
        select = select.filter(viewing_history::Column::Kind.eq(kind_name(kind)));
    }
    if let Some(since) = query.viewed_since {
        select = select.filter(viewing_history::Column::ViewedAt.gte(since));
    }
    if let Some(until) = query.viewed_until {
        select = select.filter(viewing_history::Column::ViewedAt.lte(until));
    }
    select
}

/// The value an order compares before the id; none for the order entries were written in.
fn order_key(order: HistoryOrder) -> Option<SimpleExpr> {
    match order {
        HistoryOrder::Recorded => None,
        HistoryOrder::ViewedAt => {
            Some(Expr::col((viewing_history::Entity, viewing_history::Column::ViewedAt)).into())
        }
        HistoryOrder::Library => Some(
            Expr::col((
                metadata_item::Entity,
                metadata_item::Column::LibrarySectionId,
            ))
            .into(),
        ),
    }
}

/// The entries past a cursor: after it in ascending order when `upward`, before it
/// otherwise. Neither the key nor the id is ever absent, so no `NULL` needs placing.
fn beyond(key: Option<&SimpleExpr>, cursor: &Cursor, upward: bool) -> Condition {
    let past = |expr: SimpleExpr, value: i64| {
        if upward {
            Expr::expr(expr).gt(value)
        } else {
            Expr::expr(expr).lt(value)
        }
    };
    let past_id = past(
        Expr::col((viewing_history::Entity, viewing_history::Column::Id)).into(),
        cursor.id,
    );
    match (key, cursor.keys.first()) {
        (Some(key), Some(CursorKey::Int(value))) => {
            Condition::any().add(past(key.clone(), *value)).add(
                Condition::all()
                    .add(Expr::expr(key.clone()).eq(*value))
                    .add(past_id),
            )
        }
        _ => Condition::all().add(past_id),
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
