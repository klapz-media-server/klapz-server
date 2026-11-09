//! Stored playback sessions.

use open_plex_core::page::{Page, Window};
use open_plex_core::playback::{
    NewPlaybackSession, PlaybackError, PlaybackRepository, PlaybackSession, PlaybackState,
    StreamDecision, SESSION_LIFETIME_SECONDS,
};
use open_plex_core::{PartId, RatingKey};
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, Order, QueryFilter, QueryOrder,
    QuerySelect,
};

use crate::entity::playback_session::{self, Column, Entity};

#[derive(Debug, Clone)]
pub struct SqlPlaybackRepository {
    connection: DatabaseConnection,
}

impl SqlPlaybackRepository {
    #[must_use]
    pub const fn new(connection: DatabaseConnection) -> Self {
        Self { connection }
    }
}

fn storage(error: sea_orm::DbErr) -> PlaybackError {
    PlaybackError::Storage(Box::new(error))
}

const fn state_name(state: PlaybackState) -> &'static str {
    match state {
        PlaybackState::Playing => "playing",
        PlaybackState::Paused => "paused",
        PlaybackState::Buffering => "buffering",
    }
}

fn state_of(name: &str) -> PlaybackState {
    match name {
        "paused" => PlaybackState::Paused,
        "buffering" => PlaybackState::Buffering,
        _ => PlaybackState::Playing,
    }
}

const fn decision_name(decision: StreamDecision) -> &'static str {
    match decision {
        StreamDecision::Copy => "copy",
        StreamDecision::Transcode => "transcode",
    }
}

fn decision_of(name: &str) -> StreamDecision {
    if name == "transcode" {
        StreamDecision::Transcode
    } else {
        StreamDecision::Copy
    }
}

fn to_domain(row: playback_session::Model) -> PlaybackSession {
    PlaybackSession {
        id: row.id,
        key: row.session_key,
        playback_id: row.playback_id,
        item: RatingKey::new(row.item_id),
        part: PartId::new(row.part_id),
        client: row.client,
        product: row.product,
        state: state_of(&row.state),
        video: decision_of(&row.video_decision),
        audio: decision_of(&row.audio_decision),
        position_ms: row.position_ms,
        started_at: row.started_at,
        updated_at: row.updated_at,
        expires_at: row.expires_at,
    }
}

#[async_trait::async_trait]
impl PlaybackRepository for SqlPlaybackRepository {
    async fn start(
        &self,
        session: NewPlaybackSession,
        now: i64,
    ) -> Result<PlaybackSession, PlaybackError> {
        let row = playback_session::ActiveModel {
            id: ActiveValue::NotSet,
            session_key: ActiveValue::Set(session.key.clone()),
            playback_id: ActiveValue::Set(session.playback_id),
            item_id: ActiveValue::Set(session.item.get()),
            part_id: ActiveValue::Set(session.part.get()),
            client: ActiveValue::Set(session.client),
            product: ActiveValue::Set(session.product),
            state: ActiveValue::Set(state_name(PlaybackState::Playing).to_owned()),
            video_decision: ActiveValue::Set(decision_name(session.video).to_owned()),
            audio_decision: ActiveValue::Set(decision_name(session.audio).to_owned()),
            position_ms: ActiveValue::Set(session.position_ms),
            started_at: ActiveValue::Set(now),
            updated_at: ActiveValue::Set(now),
            expires_at: ActiveValue::Set(now + SESSION_LIFETIME_SECONDS),
        };
        Entity::insert(row)
            .on_conflict(
                OnConflict::column(Column::SessionKey)
                    .update_columns([
                        Column::PlaybackId,
                        Column::ItemId,
                        Column::PartId,
                        Column::Client,
                        Column::Product,
                        Column::State,
                        Column::VideoDecision,
                        Column::AudioDecision,
                        Column::PositionMs,
                        Column::StartedAt,
                        Column::UpdatedAt,
                        Column::ExpiresAt,
                    ])
                    .to_owned(),
            )
            .exec(&self.connection)
            .await
            .map_err(storage)?;

        let stored = Entity::find()
            .filter(Column::SessionKey.eq(session.key))
            .one(&self.connection)
            .await
            .map_err(storage)?
            .ok_or_else(|| storage(sea_orm::DbErr::RecordNotFound("playback session".into())))?;
        Ok(to_domain(stored))
    }

    async fn touch(&self, key: &str, now: i64) -> Result<bool, PlaybackError> {
        let result = Entity::update_many()
            .col_expr(Column::UpdatedAt, now.into())
            .col_expr(Column::ExpiresAt, (now + SESSION_LIFETIME_SECONDS).into())
            .filter(Column::SessionKey.eq(key))
            .filter(Column::ExpiresAt.gt(now))
            .exec(&self.connection)
            .await
            .map_err(storage)?;
        Ok(result.rows_affected > 0)
    }

    async fn report(
        &self,
        playback_id: &str,
        state: PlaybackState,
        position_ms: Option<i64>,
        now: i64,
    ) -> Result<bool, PlaybackError> {
        let mut update = Entity::update_many()
            .col_expr(Column::State, state_name(state).into())
            .col_expr(Column::UpdatedAt, now.into())
            .col_expr(Column::ExpiresAt, (now + SESSION_LIFETIME_SECONDS).into())
            .filter(Column::PlaybackId.eq(playback_id))
            .filter(Column::ExpiresAt.gt(now));
        if let Some(position_ms) = position_ms {
            update = update.col_expr(Column::PositionMs, position_ms.into());
        }
        let result = update.exec(&self.connection).await.map_err(storage)?;
        Ok(result.rows_affected > 0)
    }

    async fn end_playback(&self, playback_id: &str) -> Result<bool, PlaybackError> {
        let result = Entity::delete_many()
            .filter(Column::PlaybackId.eq(playback_id))
            .exec(&self.connection)
            .await
            .map_err(storage)?;
        Ok(result.rows_affected > 0)
    }

    async fn end(&self, key: &str) -> Result<bool, PlaybackError> {
        let result = Entity::delete_many()
            .filter(Column::SessionKey.eq(key))
            .exec(&self.connection)
            .await
            .map_err(storage)?;
        Ok(result.rows_affected > 0)
    }

    async fn active(
        &self,
        now: i64,
        window: Window,
    ) -> Result<Page<PlaybackSession>, PlaybackError> {
        let mut select = Entity::find().filter(Column::ExpiresAt.gt(now));
        if let Some(after) = window.after.as_ref() {
            select = select.filter(Column::Id.gt(after.id));
        }
        if let Some(before) = window.before.as_ref() {
            select = select.filter(Column::Id.lt(before.id));
        }
        let order = if window.is_reversed() {
            Order::Desc
        } else {
            Order::Asc
        };
        let rows = select
            .order_by(Column::Id, order)
            .offset(window.skip)
            .limit(window.fetch_limit())
            .all(&self.connection)
            .await
            .map_err(storage)?;

        let mut page = Page::from_overfetch(rows, &window).map(to_domain);
        if window.is_reversed() {
            page.items.reverse();
        }
        Ok(page)
    }

    async fn session(&self, id: i64, now: i64) -> Result<Option<PlaybackSession>, PlaybackError> {
        Ok(Entity::find_by_id(id)
            .filter(Column::ExpiresAt.gt(now))
            .one(&self.connection)
            .await
            .map_err(storage)?
            .map(to_domain))
    }

    async fn delete_expired(&self, now: i64) -> Result<u64, PlaybackError> {
        Ok(Entity::delete_many()
            .filter(Column::ExpiresAt.lte(now))
            .exec(&self.connection)
            .await
            .map_err(storage)?
            .rows_affected)
    }

    async fn delete_all(&self) -> Result<u64, PlaybackError> {
        Ok(Entity::delete_many()
            .exec(&self.connection)
            .await
            .map_err(storage)?
            .rows_affected)
    }
}
