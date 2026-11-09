//! What is being played right now.
//!
//! A session expires by itself: a player that vanishes sends nothing to say so, and a crash
//! leaves its rows behind. Every sign of life pushes `expires_at` forward; a session past it is
//! no longer read, and is deleted by the next sweep.

use crate::ids::{PartId, RatingKey};
use crate::page::{Page, Window};

/// How long a session lives without a sign of life. The same as the transcoder's idle limit,
/// so that a session and its process go away together.
pub const SESSION_LIFETIME_SECONDS: i64 = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackState {
    Playing,
    Paused,
    Buffering,
}

/// What happens to one track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamDecision {
    Copy,
    Transcode,
}

/// What a session sends to its player. Each is unknown when the file was never measured.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlaybackOutput {
    /// In bits per second, as is the one below.
    pub video_bitrate: Option<i32>,
    pub audio_bitrate: Option<i32>,
    pub width: Option<i32>,
    pub height: Option<i32>,
}

/// A session being opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPlaybackSession {
    /// The client's own identifier for this conversion. Changes with the quality.
    pub key: String,
    /// The player's identifier for the whole playback, which is what its reports name.
    pub playback_id: Option<String>,
    pub item: RatingKey,
    pub part: PartId,
    pub client: Option<String>,
    pub product: Option<String>,
    pub video: StreamDecision,
    pub audio: StreamDecision,
    pub position_ms: i64,
    pub output: PlaybackOutput,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaybackSession {
    pub id: i64,
    pub key: String,
    pub playback_id: Option<String>,
    pub item: RatingKey,
    pub part: PartId,
    pub client: Option<String>,
    pub product: Option<String>,
    pub state: PlaybackState,
    pub video: StreamDecision,
    pub audio: StreamDecision,
    pub position_ms: i64,
    pub output: PlaybackOutput,
    /// Epoch seconds, as are the two below.
    pub started_at: i64,
    pub updated_at: i64,
    pub expires_at: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum PlaybackError {
    #[error("playback storage failed")]
    Storage(#[source] Box<dyn std::error::Error + Send + Sync>),
}

#[async_trait::async_trait]
pub trait PlaybackRepository: Send + Sync + std::fmt::Debug {
    /// Opens a session, or restarts the one already under this key: a player asks more than
    /// once for the same start.
    async fn start(
        &self,
        session: NewPlaybackSession,
        now: i64,
    ) -> Result<PlaybackSession, PlaybackError>;

    /// Pushes a session's expiry forward. Whether it exists and has not expired.
    async fn touch(&self, key: &str, now: i64) -> Result<bool, PlaybackError>;

    /// Records where a player stands, on every unexpired session of its playback. Without a
    /// position, the last one stays. Whether there was any.
    async fn report(
        &self,
        playback_id: &str,
        state: PlaybackState,
        position_ms: Option<i64>,
        now: i64,
    ) -> Result<bool, PlaybackError>;

    /// Closes every session of a playback. Whether there was any.
    async fn end_playback(&self, playback_id: &str) -> Result<bool, PlaybackError>;

    /// Closes a session. Whether it existed.
    async fn end(&self, key: &str) -> Result<bool, PlaybackError>;

    /// The sessions that have not expired, oldest first.
    async fn active(
        &self,
        now: i64,
        window: Window,
    ) -> Result<Page<PlaybackSession>, PlaybackError>;

    /// One session by its identifier, unless it has expired.
    async fn session(&self, id: i64, now: i64) -> Result<Option<PlaybackSession>, PlaybackError>;

    /// Deletes the expired sessions, and says how many there were.
    async fn delete_expired(&self, now: i64) -> Result<u64, PlaybackError>;

    /// Deletes every session. At startup: no process from before is still running.
    async fn delete_all(&self) -> Result<u64, PlaybackError>;
}
