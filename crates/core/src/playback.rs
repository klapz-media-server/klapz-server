//! What is being played right now.
//!
//! A session expires by itself: a player that vanishes sends nothing to say so, and a crash
//! leaves its rows behind. Every sign of life pushes `expires_at` forward; a session past it is
//! no longer read, and is deleted by the next sweep. Ending one is `crate::lifecycle`'s: the
//! row and the stream behind it go together.

use crate::ids::{PartId, RatingKey, UserId};
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
    pub user: UserId,
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

/// An owner ending a session. The session stays readable, and stops counting as playing,
/// until its player hears of it: a player that stops, or the session's expiry, removes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Termination {
    /// Epoch seconds.
    pub at: i64,
    /// What the owner told the player, if anything.
    pub reason: Option<String>,
}

/// Which sessions a listing holds. Expired ones are in neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    /// Still playing: not terminated.
    Playing,
    /// Terminated by an owner, and waiting for their player to hear of it.
    Terminated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaybackSession {
    pub id: i64,
    pub key: String,
    pub user: UserId,
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
    pub termination: Option<Termination>,
}

#[derive(Debug, thiserror::Error)]
pub enum PlaybackError {
    #[error("playback storage failed")]
    Storage(#[source] Box<dyn std::error::Error + Send + Sync>),
}

#[async_trait::async_trait]
pub trait PlaybackRepository: Send + Sync + std::fmt::Debug {
    /// Opens a session, or restarts the one already under this key: a player asks more than
    /// once for the same start. A restart keeps a termination: it was the session's.
    async fn start(
        &self,
        session: NewPlaybackSession,
        now: i64,
    ) -> Result<PlaybackSession, PlaybackError>;

    /// Pushes a session's expiry forward. Whether it exists, has not expired and was not
    /// terminated.
    async fn touch(&self, key: &str, now: i64) -> Result<bool, PlaybackError>;

    /// Records where a player stands, on every live session of its playback. Without a
    /// position, the last one stays. The keys of the sessions it reached.
    async fn report(
        &self,
        playback_id: &str,
        state: PlaybackState,
        position_ms: Option<i64>,
        now: i64,
    ) -> Result<Vec<String>, PlaybackError>;

    /// The keys of a playback's sessions, terminated and expired ones included, at most
    /// `page::MAX_ITEMS`.
    async fn keys_of_playback(&self, playback_id: &str) -> Result<Vec<String>, PlaybackError>;

    /// Closes a session. Whether it existed.
    async fn end(&self, key: &str) -> Result<bool, PlaybackError>;

    /// Closes a session if it has expired by `now`. Whether it did.
    async fn end_if_expired(&self, key: &str, now: i64) -> Result<bool, PlaybackError>;

    /// Marks a live session terminated, and gives its player one more lifetime to hear of it.
    /// The session as it now stands, unless there was no live one under this key.
    async fn terminate(
        &self,
        key: &str,
        reason: Option<String>,
        now: i64,
    ) -> Result<Option<PlaybackSession>, PlaybackError>;

    /// The sessions not expired in one status, oldest first. `only` narrows it to one
    /// user's.
    async fn sessions(
        &self,
        only: Option<UserId>,
        status: SessionStatus,
        now: i64,
        window: Window,
    ) -> Result<Page<PlaybackSession>, PlaybackError>;

    /// One session by its identifier, unless it has expired. A terminated one is still found.
    async fn session(&self, id: i64, now: i64) -> Result<Option<PlaybackSession>, PlaybackError>;

    /// The keys of the sessions expired by `now`, at most `page::MAX_ITEMS`.
    async fn expired(&self, now: i64) -> Result<Vec<String>, PlaybackError>;

    /// Deletes every session. At startup: no process from before is still running.
    async fn delete_all(&self) -> Result<u64, PlaybackError>;
}
