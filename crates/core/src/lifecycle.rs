//! A playback session from its start to its end: the stored session and the stream behind it,
//! which begin together and end together.
//!
//! Whatever ends a session — the player stopping, an owner terminating it, a client asking
//! to stop the stream, the session expiring, its stream going idle, a stream read whose
//! session is gone — comes through here and takes both down: the stream (its process, its
//! addresses, what it wrote) and the row. Each change to one key is made under that key's
//! lock, and an automatic end checks again under it that its reason still holds, so that a
//! player restarting under the same key never loses half of what it just started.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use crate::playback::{
    NewPlaybackSession, PlaybackError, PlaybackRepository, PlaybackSession, PlaybackState,
};
use crate::stream::{StartedStream, StreamEngine, StreamError, StreamOrder};

/// One lock per session key, held while that key's stream and row change.
#[derive(Debug, Default)]
pub struct SessionLocks {
    held: Mutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>,
}

impl SessionLocks {
    async fn lock(&self, key: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = {
            let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
            // A lock nobody holds or waits for is dropped with its last guard: forget it.
            held.retain(|_, lock| lock.strong_count() > 0);
            if let Some(lock) = held.get(key).and_then(Weak::upgrade) {
                lock
            } else {
                let lock = Arc::new(tokio::sync::Mutex::new(()));
                held.insert(key.to_owned(), Arc::downgrade(&lock));
                lock
            }
        };
        lock.lock_owned().await
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error(transparent)]
    Stream(#[from] StreamError),
    /// The key names a session an owner terminated, which its player has not stopped yet.
    #[error("this session was terminated")]
    Terminated,
    #[error(transparent)]
    Storage(#[from] PlaybackError),
}

/// What a sweep ended.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Swept {
    /// Sessions past their expiry.
    pub expired: usize,
    /// Streams nobody read for the engine's idle limit, or whose conversion was gone.
    pub idle: usize,
}

/// The sessions, the streams, and the locks between them.
#[derive(Debug, Clone, Copy)]
pub struct Lifecycle<'a> {
    pub sessions: &'a dyn PlaybackRepository,
    pub streams: &'a dyn StreamEngine,
    pub locks: &'a SessionLocks,
}

impl Lifecycle<'_> {
    /// Starts a stream and records its session, `record` describing it from what was started.
    /// A session that cannot be recorded, or that was terminated, leaves no stream behind.
    ///
    /// # Errors
    ///
    /// The engine's refusal, a terminated session, or the storage's failure.
    pub async fn start(
        &self,
        order: StreamOrder,
        record: impl FnOnce(&StartedStream) -> NewPlaybackSession + Send,
        now: i64,
    ) -> Result<(StartedStream, PlaybackSession), StartError> {
        let key = order.key.clone();
        let (started, recorded) = {
            let _held = self.locks.lock(&key).await;
            let started = self.streams.start(order).await?;
            let recorded = self.sessions.start(record(&started), now).await;
            let terminated = matches!(&recorded, Ok(session) if session.termination.is_some());
            if recorded.is_err() || terminated {
                self.streams.stop(&key);
            }
            if terminated {
                return Err(StartError::Terminated);
            }
            (started, recorded?)
        };
        // Outside this key's lock: two starts of one playback may replace each other.
        for replaced in &started.replaced {
            if let Err(error) = self.end(replaced).await {
                // Its row expires by itself, its stream being stopped already.
                tracing::warn!(%error, "could not close a replaced playback session");
            }
        }
        Ok((started, recorded))
    }

    /// Ends a session: its stream, then its row. Whether there was either.
    ///
    /// # Errors
    ///
    /// The storage's failure, the stream being stopped by then.
    pub async fn end(&self, key: &str) -> Result<bool, PlaybackError> {
        let _held = self.locks.lock(key).await;
        let stopped = self.streams.stop(key);
        Ok(self.sessions.end(key).await? || stopped)
    }

    /// Ends every session of a playback: the player has stopped. Whether there was any.
    ///
    /// # Errors
    ///
    /// The storage's failure.
    pub async fn end_playback(&self, playback_id: &str) -> Result<bool, PlaybackError> {
        let mut ended = false;
        for key in self.sessions.keys_of_playback(playback_id).await? {
            ended |= self.end(&key).await?;
        }
        Ok(ended)
    }

    /// Terminates a session for an owner: its stream stops at once, and the row stays for
    /// its player to learn why. The session as it now stands, unless no live one has this key.
    ///
    /// # Errors
    ///
    /// The storage's failure.
    pub async fn terminate(
        &self,
        key: &str,
        reason: Option<String>,
        now: i64,
    ) -> Result<Option<PlaybackSession>, PlaybackError> {
        let _held = self.locks.lock(key).await;
        let terminated = self.sessions.terminate(key, reason, now).await?;
        if terminated.is_some() {
            self.streams.stop(key);
        }
        Ok(terminated)
    }

    /// A sign of life from a client: the session's and its stream's. Whether the session
    /// still plays.
    ///
    /// # Errors
    ///
    /// The storage's failure.
    pub async fn touch(&self, key: &str, now: i64) -> Result<bool, PlaybackError> {
        let alive = self.sessions.touch(key, now).await?;
        if alive {
            self.streams.touch(key);
        }
        Ok(alive)
    }

    /// Records where a player stands, which is a sign of life for each of its sessions and
    /// their streams. Whether the playback had any.
    ///
    /// # Errors
    ///
    /// The storage's failure.
    pub async fn report(
        &self,
        playback_id: &str,
        state: PlaybackState,
        position_ms: Option<i64>,
        now: i64,
    ) -> Result<bool, PlaybackError> {
        let reached = self
            .sessions
            .report(playback_id, state, position_ms, now)
            .await?;
        for key in &reached {
            self.streams.touch(key);
        }
        Ok(!reached.is_empty())
    }

    /// A stream being read keeps its session alive; a stream whose session is gone — ended,
    /// expired or terminated without its stream being reached — is stopped. Whether it plays.
    ///
    /// # Errors
    ///
    /// The storage's failure.
    pub async fn read(&self, key: &str, now: i64) -> Result<bool, PlaybackError> {
        if self.sessions.touch(key, now).await? {
            return Ok(true);
        }
        let _held = self.locks.lock(key).await;
        // A start under this key may have recorded its session while this waited.
        if self.sessions.touch(key, now).await? {
            return Ok(true);
        }
        self.streams.stop(key);
        Ok(false)
    }

    /// Ends what has stopped by itself: sessions past their expiry, and streams nobody reads.
    ///
    /// # Errors
    ///
    /// The storage's failure, what was ended until then staying ended.
    pub async fn sweep(&self, now: i64) -> Result<Swept, PlaybackError> {
        let mut swept = Swept::default();
        for key in self.sessions.expired(now).await? {
            let _held = self.locks.lock(&key).await;
            if self.sessions.end_if_expired(&key, now).await? {
                self.streams.stop(&key);
                swept.expired += 1;
            }
        }
        for key in self.streams.idle() {
            let _held = self.locks.lock(&key).await;
            if self.streams.stop_if_idle(&key) {
                self.sessions.end(&key).await?;
                swept.idle += 1;
            }
        }
        Ok(swept)
    }

    /// Ends every session: the server is stopping, or starting with nothing from before
    /// running. How many rows there were.
    ///
    /// # Errors
    ///
    /// The storage's failure, every stream being stopped by then.
    pub async fn end_all(&self) -> Result<u64, PlaybackError> {
        self.streams.stop_all().await;
        self.sessions.delete_all().await
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::collections::HashSet;
    use std::time::Duration;

    use super::*;
    use crate::playback::{PlaybackOutput, StreamDecision, SESSION_LIFETIME_SECONDS};
    use crate::stream::{Delivery, StreamLimits, StreamSource, StreamTrack};
    use crate::testing::InMemoryPlayback;
    use crate::{PartId, RatingKey};

    const NOW: i64 = 1_700_000_000;

    /// An engine that keeps a set of running keys, some of them idle.
    #[derive(Debug, Default)]
    struct Engine {
        running: Mutex<HashSet<String>>,
        idle: Mutex<HashSet<String>>,
        stopped: Mutex<Vec<String>>,
        touched: Mutex<Vec<String>>,
        replaces: Mutex<Vec<String>>,
    }

    impl Engine {
        fn stopped(&self) -> Vec<String> {
            self.stopped.lock().unwrap().clone()
        }

        fn is_running(&self, key: &str) -> bool {
            self.running.lock().unwrap().contains(key)
        }
    }

    #[async_trait::async_trait]
    impl StreamEngine for Engine {
        async fn start(&self, order: StreamOrder) -> Result<StartedStream, StreamError> {
            self.running.lock().unwrap().insert(order.key);
            let replaced = std::mem::take(&mut *self.replaces.lock().unwrap());
            for key in &replaced {
                self.running.lock().unwrap().remove(key);
            }
            Ok(StartedStream {
                url: String::new(),
                subtitles_url: None,
                video: StreamDecision::Copy,
                audio: StreamDecision::Copy,
                output: PlaybackOutput::default(),
                replaced,
            })
        }

        fn stop(&self, key: &str) -> bool {
            self.stopped.lock().unwrap().push(key.to_owned());
            self.idle.lock().unwrap().remove(key);
            self.running.lock().unwrap().remove(key)
        }

        fn touch(&self, key: &str) -> bool {
            self.touched.lock().unwrap().push(key.to_owned());
            self.idle.lock().unwrap().remove(key);
            self.is_running(key)
        }

        fn idle(&self) -> Vec<String> {
            self.idle.lock().unwrap().iter().cloned().collect()
        }

        fn stop_if_idle(&self, key: &str) -> bool {
            let idle = self.idle.lock().unwrap().contains(key);
            idle && self.stop(key)
        }

        async fn stop_all(&self) {
            self.running.lock().unwrap().clear();
        }
    }

    fn order(key: &str) -> StreamOrder {
        let track = StreamTrack {
            position: 0,
            codec: "h264".to_owned(),
            language: None,
            bitrate: None,
        };
        StreamOrder {
            key: key.to_owned(),
            playback: Some("tab".to_owned()),
            source: StreamSource {
                path: String::new(),
                roots: Vec::new(),
                width: None,
                height: None,
                bitrate: None,
                frame_rate: None,
                duration_ms: None,
                video: track.clone(),
                audio: track,
            },
            delivery: Delivery::Original,
            limits: StreamLimits::default(),
            subtitle: None,
            position_ms: 0,
        }
    }

    fn recorded(key: &str) -> NewPlaybackSession {
        NewPlaybackSession {
            key: key.to_owned(),
            user: crate::UserId::new(1),
            playback_id: Some("tab".to_owned()),
            item: RatingKey::new(1),
            part: PartId::new(1),
            client: None,
            product: None,
            video: StreamDecision::Copy,
            audio: StreamDecision::Copy,
            position_ms: 0,
            output: PlaybackOutput::default(),
        }
    }

    struct Fixture {
        sessions: InMemoryPlayback,
        engine: Engine,
        locks: SessionLocks,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                sessions: InMemoryPlayback::default(),
                engine: Engine::default(),
                locks: SessionLocks::default(),
            }
        }

        fn lifecycle(&self) -> Lifecycle<'_> {
            Lifecycle {
                sessions: &self.sessions,
                streams: &self.engine,
                locks: &self.locks,
            }
        }

        async fn start(&self, key: &str, now: i64) {
            let session = recorded(key);
            self.lifecycle()
                .start(order(key), move |_| session, now)
                .await
                .unwrap();
        }

        async fn has_row(&self, key: &str) -> bool {
            self.sessions
                .keys_of_playback("tab")
                .await
                .unwrap()
                .iter()
                .any(|stored| stored == key)
        }
    }

    #[tokio::test]
    async fn an_end_takes_down_the_stream_and_the_row_once() {
        let fixture = Fixture::new();
        fixture.start("a", NOW).await;
        assert!(fixture.lifecycle().end("a").await.unwrap());
        assert!(!fixture.engine.is_running("a"));
        assert!(!fixture.has_row("a").await);
        assert!(!fixture.lifecycle().end("a").await.unwrap());
    }

    #[tokio::test]
    async fn a_start_ends_the_sessions_its_stream_replaced() {
        let fixture = Fixture::new();
        fixture.start("low", NOW).await;
        fixture
            .engine
            .replaces
            .lock()
            .unwrap()
            .push("low".to_owned());
        fixture.start("high", NOW).await;
        assert!(!fixture.has_row("low").await);
        assert!(fixture.has_row("high").await);
    }

    #[tokio::test]
    async fn a_terminated_session_cannot_be_started_again() {
        let fixture = Fixture::new();
        fixture.start("a", NOW).await;
        fixture
            .lifecycle()
            .terminate("a", None, NOW + 1)
            .await
            .unwrap();
        let again = fixture
            .lifecycle()
            .start(order("a"), |_| recorded("a"), NOW + 2)
            .await;
        assert!(matches!(again, Err(StartError::Terminated)));
        assert!(!fixture.engine.is_running("a"));
        // Once its player has stopped, the key is free again.
        fixture.lifecycle().end_playback("tab").await.unwrap();
        fixture.start("a", NOW + 3).await;
        assert!(fixture.engine.is_running("a"));
    }

    #[tokio::test]
    async fn a_report_keeps_the_streams_of_the_playback_running() {
        let fixture = Fixture::new();
        fixture.start("a", NOW).await;
        let reported = fixture
            .lifecycle()
            .report("tab", PlaybackState::Paused, Some(1000), NOW + 10)
            .await
            .unwrap();
        assert!(reported);
        assert_eq!(*fixture.engine.touched.lock().unwrap(), ["a"]);
    }

    #[tokio::test]
    async fn a_termination_stops_the_stream_and_keeps_the_row_for_the_player() {
        let fixture = Fixture::new();
        fixture.start("a", NOW).await;
        let terminated = fixture
            .lifecycle()
            .terminate("a", Some("Bedtime".to_owned()), NOW + 5)
            .await
            .unwrap();
        assert!(terminated.is_some());
        assert!(!fixture.engine.is_running("a"));
        assert!(fixture.has_row("a").await);
        assert!(!fixture.lifecycle().touch("a", NOW + 6).await.unwrap());
        // The player stopping removes it.
        assert!(fixture.lifecycle().end_playback("tab").await.unwrap());
        assert!(!fixture.has_row("a").await);
    }

    #[tokio::test]
    async fn the_sweep_ends_expired_sessions_with_their_streams() {
        let fixture = Fixture::new();
        fixture.start("old", NOW).await;
        fixture.start("new", NOW + 100).await;
        let swept = fixture
            .lifecycle()
            .sweep(NOW + SESSION_LIFETIME_SECONDS)
            .await
            .unwrap();
        assert_eq!(
            swept,
            Swept {
                expired: 1,
                idle: 0
            }
        );
        assert_eq!(fixture.engine.stopped(), ["old"]);
        assert!(fixture.engine.is_running("new"));
        assert!(fixture.has_row("new").await);
    }

    #[tokio::test]
    async fn the_sweep_ends_the_session_of_an_idle_stream() {
        let fixture = Fixture::new();
        fixture.start("a", NOW).await;
        fixture.start("b", NOW).await;
        fixture.engine.idle.lock().unwrap().insert("a".to_owned());
        let swept = fixture.lifecycle().sweep(NOW + 1).await.unwrap();
        assert_eq!(
            swept,
            Swept {
                expired: 0,
                idle: 1
            }
        );
        assert!(!fixture.has_row("a").await);
        assert!(fixture.has_row("b").await);
    }

    #[tokio::test]
    async fn a_read_whose_session_is_gone_stops_the_stream() {
        let fixture = Fixture::new();
        fixture.start("a", NOW).await;
        assert!(fixture.lifecycle().read("a", NOW + 10).await.unwrap());
        // Expired without the sweep having passed yet.
        let late = NOW + 10 + SESSION_LIFETIME_SECONDS;
        assert!(!fixture.lifecycle().read("a", late).await.unwrap());
        assert!(!fixture.engine.is_running("a"));
    }

    #[tokio::test]
    async fn ending_everything_leaves_nothing_running() {
        let fixture = Fixture::new();
        fixture.start("a", NOW).await;
        fixture.start("b", NOW).await;
        assert_eq!(fixture.lifecycle().end_all().await.unwrap(), 2);
        assert!(fixture.engine.running.lock().unwrap().is_empty());
        assert!(!fixture.has_row("a").await);
    }

    #[tokio::test]
    async fn one_key_changes_under_one_holder_at_a_time() {
        let locks = Arc::new(SessionLocks::default());
        let held = locks.lock("a").await;
        let waiting = {
            let locks = Arc::clone(&locks);
            tokio::spawn(async move {
                let _second = locks.lock("a").await;
            })
        };
        // Another key is not held up.
        let other = tokio::time::timeout(Duration::from_secs(1), locks.lock("b")).await;
        assert!(other.is_ok());
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished());
        drop(held);
        tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .unwrap()
            .unwrap();
        // Nobody holds either now: the next lock forgets them.
        drop(other);
        let _again = locks.lock("c").await;
        assert_eq!(locks.held.lock().unwrap().len(), 1);
    }
}
