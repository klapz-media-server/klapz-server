//! Stored play queues, against a real database.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use open_plex_core::page::Window;
use open_plex_core::play_queue::{NewPlayQueue, PlayQueueRepository};
use open_plex_core::RatingKey;
use open_plex_db::{connect, SqlPlayQueueRepository};

const NOW: i64 = 1_700_000_000;

async fn repository() -> SqlPlayQueueRepository {
    SqlPlayQueueRepository::new(connect("sqlite::memory:").await.expect("database"))
}

fn queue(count: i64, selected: usize) -> NewPlayQueue {
    NewPlayQueue {
        playlist: None,
        user: None,
        source_uri: "library://x/item/1".to_owned(),
        items: (1..=count).map(RatingKey::new).collect(),
        selected,
        shuffled: false,
    }
}

fn page(first: u32, skip: u64) -> Window {
    Window::new(None, None, Some(first), None)
        .expect("window")
        .skipping(skip)
}

#[tokio::test]
async fn a_queue_selects_the_entry_it_was_told_to() {
    let repository = repository().await;
    let created = repository.create(queue(5, 3), NOW).await.expect("create");
    assert_eq!(created.total, 5);
    assert_eq!(created.version, 1);
    let selected = created.selected.expect("a selection");
    assert_eq!(selected.position, 3);
    assert_eq!(selected.item, RatingKey::new(4));
    assert_eq!(
        repository.entry(selected.id).await.expect("entry"),
        Some(selected)
    );
}

#[tokio::test]
async fn an_empty_queue_has_no_selection() {
    let repository = repository().await;
    let created = repository.create(queue(0, 0), NOW).await.expect("create");
    assert_eq!(created.total, 0);
    assert!(created.selected.is_none());
}

#[tokio::test]
async fn entries_come_in_play_order_from_any_offset() {
    let repository = repository().await;
    let id = repository
        .create(queue(600, 0), NOW)
        .await
        .expect("create")
        .id;
    let found = repository.entries(id, page(3, 250)).await.expect("page");
    let positions: Vec<i64> = found.items.iter().map(|entry| entry.position).collect();
    assert_eq!(positions, [250, 251, 252]);
    assert!(found.has_next);
}

#[tokio::test]
async fn an_unknown_queue_is_absent() {
    let repository = repository().await;
    assert!(repository.queue(99).await.expect("read").is_none());
    assert!(repository.entry(99).await.expect("read").is_none());
}

#[tokio::test]
async fn old_queues_are_deleted_with_their_entries() {
    let repository = repository().await;
    let old = repository.create(queue(3, 0), NOW).await.expect("create");
    let recent = repository
        .create(queue(2, 0), NOW + 100)
        .await
        .expect("create");
    let deleted = repository
        .delete_created_before(NOW + 50)
        .await
        .expect("sweep");
    assert_eq!(deleted, 1);
    assert!(repository.queue(old.id).await.expect("read").is_none());
    assert!(repository
        .entries(old.id, page(10, 0))
        .await
        .expect("page")
        .items
        .is_empty());
    assert_eq!(
        repository
            .queue(recent.id)
            .await
            .expect("read")
            .expect("kept")
            .total,
        2
    );
}
