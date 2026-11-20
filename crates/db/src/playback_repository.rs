//! Stored playback sessions.

use klapz_core::page::{Page, Window, MAX_ITEMS};
use klapz_core::playback::{
    NewPlaybackSession, PlaybackError, PlaybackOutput, PlaybackRepository, PlaybackSession,
    PlaybackState, SessionStatus, StreamDecision, Termination, SESSION_LIFETIME_SECONDS,
};
use klapz_core::{PartId, RatingKey, SectionId, UserId};
use sea_orm::sea_query::{OnConflict, Query};
use sea_orm::{
    ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, Order, QueryFilter, QueryOrder,
    QuerySelect,
};

use crate::entity::metadata_item;
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
        user: UserId::new(row.user_id),
        playback_id: row.playback_id,
        item: RatingKey::new(row.item_id),
        part: PartId::new(row.part_id),
        client: row.client,
        product: row.product,
        state: state_of(&row.state),
        video: decision_of(&row.video_decision),
        audio: decision_of(&row.audio_decision),
        position_ms: row.position_ms,
        output: PlaybackOutput {
            video_bitrate: row.video_bitrate,
            audio_bitrate: row.audio_bitrate,
            width: row.width,
            height: row.height,
        },
        started_at: row.started_at,
        updated_at: row.updated_at,
        expires_at: row.expires_at,
        termination: row.terminated_at.map(|at| Termination {
            at,
            reason: row.termination_reason,
        }),
    }
}

/// A session still playing: not expired, not terminated.
fn live(now: i64) -> sea_orm::Condition {
    sea_orm::Condition::all()
        .add(Column::ExpiresAt.gt(now))
        .add(Column::TerminatedAt.is_null())
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
            user_id: ActiveValue::Set(session.user.get()),
            playback_id: ActiveValue::Set(session.playback_id),
            item_id: ActiveValue::Set(session.item.get()),
            part_id: ActiveValue::Set(session.part.get()),
            client: ActiveValue::Set(session.client),
            product: ActiveValue::Set(session.product),
            state: ActiveValue::Set(state_name(PlaybackState::Playing).to_owned()),
            video_decision: ActiveValue::Set(decision_name(session.video).to_owned()),
            audio_decision: ActiveValue::Set(decision_name(session.audio).to_owned()),
            position_ms: ActiveValue::Set(session.position_ms),
            video_bitrate: ActiveValue::Set(session.output.video_bitrate),
            audio_bitrate: ActiveValue::Set(session.output.audio_bitrate),
            width: ActiveValue::Set(session.output.width),
            height: ActiveValue::Set(session.output.height),
            started_at: ActiveValue::Set(now),
            updated_at: ActiveValue::Set(now),
            expires_at: ActiveValue::Set(now + SESSION_LIFETIME_SECONDS),
            terminated_at: ActiveValue::NotSet,
            termination_reason: ActiveValue::NotSet,
        };
        Entity::insert(row)
            .on_conflict(
                OnConflict::column(Column::SessionKey)
                    .update_columns([
                        Column::UserId,
                        Column::PlaybackId,
                        Column::ItemId,
                        Column::PartId,
                        Column::Client,
                        Column::Product,
                        Column::State,
                        Column::VideoDecision,
                        Column::AudioDecision,
                        Column::PositionMs,
                        Column::VideoBitrate,
                        Column::AudioBitrate,
                        Column::Width,
                        Column::Height,
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
            .filter(live(now))
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
    ) -> Result<Vec<String>, PlaybackError> {
        let reached: Vec<String> = Entity::find()
            .select_only()
            .column(Column::SessionKey)
            .filter(Column::PlaybackId.eq(playback_id))
            .filter(live(now))
            .limit(u64::from(MAX_ITEMS))
            .into_tuple()
            .all(&self.connection)
            .await
            .map_err(storage)?;
        if reached.is_empty() {
            return Ok(reached);
        }
        let mut update = Entity::update_many()
            .col_expr(Column::State, state_name(state).into())
            .col_expr(Column::UpdatedAt, now.into())
            .col_expr(Column::ExpiresAt, (now + SESSION_LIFETIME_SECONDS).into())
            .filter(Column::SessionKey.is_in(reached.iter().map(String::as_str)));
        if let Some(position_ms) = position_ms {
            update = update.col_expr(Column::PositionMs, position_ms.into());
        }
        update.exec(&self.connection).await.map_err(storage)?;
        Ok(reached)
    }

    async fn keys_of_playback(&self, playback_id: &str) -> Result<Vec<String>, PlaybackError> {
        Entity::find()
            .select_only()
            .column(Column::SessionKey)
            .filter(Column::PlaybackId.eq(playback_id))
            .limit(u64::from(MAX_ITEMS))
            .into_tuple()
            .all(&self.connection)
            .await
            .map_err(storage)
    }

    async fn keys_in_library(&self, section: SectionId) -> Result<Vec<String>, PlaybackError> {
        let items = Query::select()
            .column(metadata_item::Column::Id)
            .from(metadata_item::Entity)
            .and_where(metadata_item::Column::LibrarySectionId.eq(section.get()))
            .to_owned();
        Entity::find()
            .select_only()
            .column(Column::SessionKey)
            .filter(Column::ItemId.in_subquery(items))
            .into_tuple()
            .all(&self.connection)
            .await
            .map_err(storage)
    }

    async fn end(&self, key: &str) -> Result<bool, PlaybackError> {
        let result = Entity::delete_many()
            .filter(Column::SessionKey.eq(key))
            .exec(&self.connection)
            .await
            .map_err(storage)?;
        Ok(result.rows_affected > 0)
    }

    async fn end_if_expired(&self, key: &str, now: i64) -> Result<bool, PlaybackError> {
        let result = Entity::delete_many()
            .filter(Column::SessionKey.eq(key))
            .filter(Column::ExpiresAt.lte(now))
            .exec(&self.connection)
            .await
            .map_err(storage)?;
        Ok(result.rows_affected > 0)
    }

    async fn terminate(
        &self,
        key: &str,
        reason: Option<String>,
        now: i64,
    ) -> Result<Option<PlaybackSession>, PlaybackError> {
        let result = Entity::update_many()
            .col_expr(Column::TerminatedAt, now.into())
            .col_expr(Column::TerminationReason, reason.into())
            .col_expr(Column::UpdatedAt, now.into())
            .col_expr(Column::ExpiresAt, (now + SESSION_LIFETIME_SECONDS).into())
            .filter(Column::SessionKey.eq(key))
            .filter(live(now))
            .exec(&self.connection)
            .await
            .map_err(storage)?;
        if result.rows_affected == 0 {
            return Ok(None);
        }
        Ok(Entity::find()
            .filter(Column::SessionKey.eq(key))
            .one(&self.connection)
            .await
            .map_err(storage)?
            .map(to_domain))
    }

    async fn sessions(
        &self,
        only: Option<UserId>,
        status: SessionStatus,
        now: i64,
        window: Window,
    ) -> Result<Page<PlaybackSession>, PlaybackError> {
        let mut select = Entity::find().filter(match status {
            SessionStatus::Playing => live(now),
            SessionStatus::Terminated => sea_orm::Condition::all()
                .add(Column::ExpiresAt.gt(now))
                .add(Column::TerminatedAt.is_not_null()),
        });
        if let Some(user) = only {
            select = select.filter(Column::UserId.eq(user.get()));
        }
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

    async fn expired(&self, now: i64) -> Result<Vec<String>, PlaybackError> {
        Entity::find()
            .select_only()
            .column(Column::SessionKey)
            .filter(Column::ExpiresAt.lte(now))
            .limit(u64::from(MAX_ITEMS))
            .into_tuple()
            .all(&self.connection)
            .await
            .map_err(storage)
    }

    async fn delete_all(&self) -> Result<u64, PlaybackError> {
        Ok(Entity::delete_many()
            .exec(&self.connection)
            .await
            .map_err(storage)?
            .rows_affected)
    }
}
