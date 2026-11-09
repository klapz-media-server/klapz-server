//! Tokens issued by an account service rather than by this server.
//!
//! A client that found the server through plex.tv presents a plex.tv token, and only plex.tv can
//! say whose it is. The token comes from whoever connects, so asking is bounded twice: an answer
//! is remembered, a refusal included, and only so many new tokens are looked up per minute. Past
//! that limit an unknown token is refused without being remembered, so tokens already known keep
//! working during a flood.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use open_plex_core::access::{token_digest, AccountDirectory};

/// How long an answer stands. Also how long a token revoked on the service keeps working here.
const REMEMBERED_FOR: Duration = Duration::from_secs(300);
const REMEMBERED_AT_MOST: usize = 4096;
const LOOKUPS_PER_MINUTE: u32 = 60;
/// Longer than any token plex.tv issues, and short enough not to be forwarded for nothing.
const LONGEST_TOKEN: usize = 4096;

#[derive(Debug)]
pub struct RemoteTokens {
    directory: Arc<dyn AccountDirectory>,
    state: Mutex<State>,
}

#[derive(Debug)]
struct State {
    /// By digest: the raw tokens of everyone connected need not sit in memory.
    answers: HashMap<String, (Instant, Option<String>)>,
    minute_started: Instant,
    lookups_this_minute: u32,
}

enum Known {
    Answer(Option<String>),
    Unknown,
    OverLimit,
}

impl RemoteTokens {
    pub fn new(directory: Arc<dyn AccountDirectory>) -> Self {
        Self {
            directory,
            state: Mutex::new(State {
                answers: HashMap::new(),
                minute_started: Instant::now(),
                lookups_this_minute: 0,
            }),
        }
    }

    /// The identifier of the account a token belongs to.
    pub async fn account_of(&self, token: &str) -> Option<String> {
        if token.is_empty() || token.len() > LONGEST_TOKEN {
            return None;
        }
        let digest = token_digest(token);
        match self.recall(&digest) {
            Known::Answer(account) => return account,
            Known::OverLimit => return None,
            Known::Unknown => {}
        }
        match self.directory.account_of(token).await {
            Ok(account) => {
                let id = account.map(|account| account.id);
                self.remember(digest, id.clone());
                id
            }
            // Not remembered: an outage says nothing about the token.
            Err(error) => {
                tracing::warn!(%error, "could not ask whose a token is");
                None
            }
        }
    }

    fn recall(&self, digest: &str) -> Known {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        if let Some((at, account)) = state.answers.get(digest) {
            if now.duration_since(*at) < REMEMBERED_FOR {
                return Known::Answer(account.clone());
            }
        }
        if now.duration_since(state.minute_started) >= Duration::from_secs(60) {
            state.minute_started = now;
            state.lookups_this_minute = 0;
        }
        if state.lookups_this_minute >= LOOKUPS_PER_MINUTE {
            return Known::OverLimit;
        }
        state.lookups_this_minute += 1;
        Known::Unknown
    }

    fn remember(&self, digest: String, account: Option<String>) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        if state.answers.len() >= REMEMBERED_AT_MOST {
            state
                .answers
                .retain(|_, (at, _)| now.duration_since(*at) < REMEMBERED_FOR);
        }
        if state.answers.len() < REMEMBERED_AT_MOST {
            state.answers.insert(digest, (now, account));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use open_plex_core::testing::StubDirectory;

    use super::*;

    fn tokens() -> (Arc<StubDirectory>, RemoteTokens) {
        let directory = Arc::new(StubDirectory::default());
        let tokens = RemoteTokens::new(Arc::clone(&directory) as Arc<dyn AccountDirectory>);
        (directory, tokens)
    }

    #[tokio::test]
    async fn a_known_token_is_asked_about_once() {
        let (directory, tokens) = tokens();
        for _ in 0..3 {
            assert_eq!(
                tokens
                    .account_of(StubDirectory::ACCOUNT_TOKEN)
                    .await
                    .as_deref(),
                Some("linked")
            );
        }
        assert_eq!(directory.lookups.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_refused_token_is_remembered_too() {
        let (directory, tokens) = tokens();
        for _ in 0..3 {
            assert_eq!(tokens.account_of("nobody").await, None);
        }
        assert_eq!(directory.lookups.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn an_outage_is_not_remembered() {
        let (directory, tokens) = tokens();
        directory.offline.store(true, Ordering::SeqCst);
        assert_eq!(tokens.account_of(StubDirectory::ACCOUNT_TOKEN).await, None);
        directory.offline.store(false, Ordering::SeqCst);
        assert_eq!(
            tokens
                .account_of(StubDirectory::ACCOUNT_TOKEN)
                .await
                .as_deref(),
            Some("linked")
        );
    }

    #[tokio::test]
    async fn a_flood_of_new_tokens_stops_reaching_the_service_but_not_known_ones() {
        let (directory, tokens) = tokens();
        tokens.account_of(StubDirectory::ACCOUNT_TOKEN).await;
        for attempt in 0..LOOKUPS_PER_MINUTE * 2 {
            tokens.account_of(&format!("guess-{attempt}")).await;
        }
        assert_eq!(
            directory.lookups.load(Ordering::SeqCst),
            LOOKUPS_PER_MINUTE as usize
        );
        assert_eq!(
            tokens
                .account_of(StubDirectory::ACCOUNT_TOKEN)
                .await
                .as_deref(),
            Some("linked")
        );
    }

    #[tokio::test]
    async fn an_absurd_token_is_not_forwarded() {
        let (directory, tokens) = tokens();
        assert_eq!(
            tokens.account_of(&"x".repeat(LONGEST_TOKEN + 1)).await,
            None
        );
        assert_eq!(tokens.account_of("").await, None);
        assert_eq!(directory.lookups.load(Ordering::SeqCst), 0);
    }
}
