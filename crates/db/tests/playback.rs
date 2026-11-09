//! Stored playback sessions, against a real database.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use open_plex_core::page::Window;
use open_plex_core::playback::{
    NewPlaybackSession, PlaybackRepository, PlaybackState, StreamDecision, SESSION_LIFETIME_SECONDS,
};
use open_plex_core::{PartId, RatingKey};
use open_plex_db::{connect, SqlPlaybackRepository};

const NOW: i64 = 1_700_000_000;

async fn repository() -> SqlPlaybackRepository {
    SqlPlaybackRepository::new(connect("sqlite::memory:").await.expect("database"))
}

fn session(key: &str) -> NewPlaybackSession {
    NewPlaybackSession {
        key: key.to_owned(),
        playback_id: Some("player".to_owned()),
        item: RatingKey::new(10),
        part: PartId::new(20),
        client: Some("client".to_owned()),
        product: Some("Plex Web".to_owned()),
        video: StreamDecision::Copy,
        audio: StreamDecision::Transcode,
        position_ms: 5000,
    }
}

async fn active_keys(repository: &SqlPlaybackRepository, now: i64) -> Vec<String> {
    repository
        .active(
            now,
            Window::new(None, None, Some(10), None).expect("window"),
        )
        .await
        .expect("page")
        .items
        .into_iter()
        .map(|session| session.key)
        .collect()
}

#[tokio::test]
async fn a_started_session_is_playing_and_expires_later() {
    let repository = repository().await;
    let stored = repository.start(session("a"), NOW).await.expect("start");
    assert_eq!(stored.state, PlaybackState::Playing);
    assert_eq!(stored.video, StreamDecision::Copy);
    assert_eq!(stored.audio, StreamDecision::Transcode);
    assert_eq!(stored.position_ms, 5000);
    assert_eq!(stored.expires_at, NOW + SESSION_LIFETIME_SECONDS);
    assert_eq!(active_keys(&repository, NOW).await, ["a"]);
}

#[tokio::test]
async fn an_expired_session_is_no_longer_read() {
    let repository = repository().await;
    let id = repository.start(session("a"), NOW).await.expect("start").id;
    let after = NOW + SESSION_LIFETIME_SECONDS;
    assert!(active_keys(&repository, after).await.is_empty());
    assert!(repository.session(id, after).await.expect("read").is_none());
    assert!(repository
        .session(id, after - 1)
        .await
        .expect("read")
        .is_some());
}

#[tokio::test]
async fn a_sign_of_life_pushes_the_expiry_forward() {
    let repository = repository().await;
    repository.start(session("a"), NOW).await.expect("start");
    let later = NOW + SESSION_LIFETIME_SECONDS - 1;
    assert!(repository.touch("a", later).await.expect("touch"));
    assert_eq!(
        active_keys(&repository, NOW + SESSION_LIFETIME_SECONDS + 10).await,
        ["a"]
    );
}

#[tokio::test]
async fn an_expired_session_cannot_be_revived() {
    let repository = repository().await;
    repository.start(session("a"), NOW).await.expect("start");
    let after = NOW + SESSION_LIFETIME_SECONDS;
    assert!(!repository.touch("a", after).await.expect("touch"));
    assert!(!repository.touch("unknown", NOW).await.expect("touch"));
}

#[tokio::test]
async fn the_sweep_deletes_only_what_has_expired() {
    let repository = repository().await;
    repository.start(session("old"), NOW).await.expect("start");
    repository
        .start(session("new"), NOW + 100)
        .await
        .expect("start");
    let sweep = NOW + SESSION_LIFETIME_SECONDS;
    assert_eq!(repository.delete_expired(sweep).await.expect("sweep"), 1);
    assert_eq!(active_keys(&repository, sweep).await, ["new"]);
    assert_eq!(repository.delete_expired(sweep).await.expect("sweep"), 0);
}

#[tokio::test]
async fn starting_the_same_key_again_restarts_it() {
    let repository = repository().await;
    let first = repository.start(session("a"), NOW).await.expect("start");
    let mut again = session("a");
    again.position_ms = 90_000;
    let second = repository.start(again, NOW + 50).await.expect("start");
    assert_eq!(first.id, second.id);
    assert_eq!(second.position_ms, 90_000);
    assert_eq!(second.expires_at, NOW + 50 + SESSION_LIFETIME_SECONDS);
    assert_eq!(active_keys(&repository, NOW + 50).await, ["a"]);
}

#[tokio::test]
async fn ending_a_session_removes_it() {
    let repository = repository().await;
    repository.start(session("a"), NOW).await.expect("start");
    assert!(repository.end("a").await.expect("end"));
    assert!(!repository.end("a").await.expect("end"));
    assert!(active_keys(&repository, NOW).await.is_empty());
}

#[tokio::test]
async fn a_report_moves_every_session_of_its_playback_and_no_other() {
    let repository = repository().await;
    let first = repository.start(session("a"), NOW).await.expect("start");
    let second = repository.start(session("b"), NOW).await.expect("start");
    let other = NewPlaybackSession {
        playback_id: Some("other player".to_owned()),
        ..session("c")
    };
    let other = repository.start(other, NOW).await.expect("start");

    let later = NOW + 10;
    assert!(repository
        .report("player", PlaybackState::Paused, Some(9000), later)
        .await
        .expect("report"));

    let read = |id| {
        let repository = &repository;
        async move {
            repository
                .session(id, later)
                .await
                .expect("read")
                .expect("session")
        }
    };
    let first = read(first.id).await;
    assert_eq!(first.state, PlaybackState::Paused);
    assert_eq!(first.position_ms, 9000);
    assert_eq!(first.expires_at, later + SESSION_LIFETIME_SECONDS);
    assert_eq!(read(second.id).await.state, PlaybackState::Paused);
    let untouched = read(other.id).await;
    assert_eq!(untouched.state, PlaybackState::Playing);
    assert_eq!(untouched.position_ms, 5000);
}

#[tokio::test]
async fn a_report_without_a_position_keeps_the_last_one() {
    let repository = repository().await;
    let id = repository.start(session("a"), NOW).await.expect("start").id;
    repository
        .report("player", PlaybackState::Buffering, None, NOW)
        .await
        .expect("report");
    let stored = repository
        .session(id, NOW)
        .await
        .expect("read")
        .expect("session");
    assert_eq!(stored.state, PlaybackState::Buffering);
    assert_eq!(stored.position_ms, 5000);
}

#[tokio::test]
async fn a_report_does_not_revive_an_expired_session() {
    let repository = repository().await;
    repository.start(session("a"), NOW).await.expect("start");
    let after = NOW + SESSION_LIFETIME_SECONDS;
    assert!(!repository
        .report("player", PlaybackState::Playing, Some(1), after)
        .await
        .expect("report"));
    assert!(active_keys(&repository, after).await.is_empty());
}

#[tokio::test]
async fn ending_a_playback_closes_all_its_sessions_and_only_those() {
    let repository = repository().await;
    repository.start(session("a"), NOW).await.expect("start");
    repository.start(session("b"), NOW).await.expect("start");
    let other = NewPlaybackSession {
        playback_id: None,
        ..session("c")
    };
    repository.start(other, NOW).await.expect("start");

    assert!(repository.end_playback("player").await.expect("end"));
    assert!(!repository.end_playback("player").await.expect("end"));
    assert_eq!(active_keys(&repository, NOW).await, ["c"]);
}

#[tokio::test]
async fn startup_clears_everything_left_behind() {
    let repository = repository().await;
    repository.start(session("a"), NOW).await.expect("start");
    repository.start(session("b"), NOW).await.expect("start");
    assert_eq!(repository.delete_all().await.expect("clear"), 2);
    assert!(active_keys(&repository, NOW).await.is_empty());
}
