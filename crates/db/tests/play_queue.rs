//! Stored play queues, against a real database.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use open_plex_core::page::Window;
use open_plex_core::play_queue::{
    ChangeRefusal, NewPlayQueue, PlayQueueChange, PlayQueueKind, PlayQueueRepository,
};
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
        client: None,
        kind: PlayQueueKind::Video,
        source_uri: "library://x/item/1".to_owned(),
        source: (1..=count).map(RatingKey::new).collect(),
        selected,
        shuffle: false,
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

fn for_client(client: &str, kind: PlayQueueKind) -> NewPlayQueue {
    NewPlayQueue {
        client: Some(client.to_owned()),
        kind,
        ..queue(2, 0)
    }
}

async fn items(repository: &SqlPlayQueueRepository, queue: i64) -> Vec<(i64, i64)> {
    repository
        .entries(queue, page(50, 0))
        .await
        .expect("page")
        .items
        .iter()
        .map(|entry| (entry.id, entry.item.get()))
        .collect()
}

#[tokio::test]
async fn a_player_s_new_queue_replaces_its_previous_one_of_the_same_kind() {
    let repository = repository().await;
    let first = repository
        .create(for_client("a", PlayQueueKind::Video), NOW)
        .await
        .expect("create");
    let other_player = repository
        .create(for_client("b", PlayQueueKind::Video), NOW)
        .await
        .expect("create");
    let other_kind = repository
        .create(for_client("a", PlayQueueKind::Audio), NOW)
        .await
        .expect("create");
    let anonymous = repository.create(queue(1, 0), NOW).await.expect("create");
    let second = repository
        .create(for_client("a", PlayQueueKind::Video), NOW)
        .await
        .expect("create");
    assert!(repository.queue(first.id).await.expect("read").is_none());
    assert!(items(&repository, first.id).await.is_empty());
    for kept in [other_player.id, other_kind.id, anonymous.id, second.id] {
        assert!(repository.queue(kept).await.expect("read").is_some());
    }
    let mut another_user = for_client("a", PlayQueueKind::Video);
    another_user.user = Some(open_plex_core::UserId::new(7));
    repository.create(another_user, NOW).await.expect("create");
    assert!(repository.queue(second.id).await.expect("read").is_some());
}

#[tokio::test]
async fn an_edit_keeps_the_ids_of_the_entries_it_keeps() {
    let repository = repository().await;
    let created = repository.create(queue(3, 0), NOW).await.expect("create");
    let before = items(&repository, created.id).await;
    let edited = repository
        .change(
            created.id,
            &PlayQueueChange::Add {
                items: vec![RatingKey::new(9)],
                next: false,
            },
        )
        .await
        .expect("change")
        .expect("queue")
        .expect("applied");
    assert_eq!(edited.version, 2);
    assert_eq!(edited.total, 4);
    let after = items(&repository, created.id).await;
    assert_eq!(after[0], before[0]);
    assert_eq!(after[1].1, 9);
    assert_eq!(&after[2..], &before[1..]);
    let added = edited.last_added.expect("an up next area");
    assert_eq!((added.id, added.position), (after[1].0, 1));

    let moved = repository
        .change(
            created.id,
            &PlayQueueChange::Move {
                entry: before[0].0,
                after: Some(before[2].0),
            },
        )
        .await
        .expect("change")
        .expect("queue")
        .expect("applied");
    let selected = moved.selected.expect("selection");
    assert_eq!((selected.id, selected.position), (before[0].0, 3));
}

#[tokio::test]
async fn shuffling_rebuilds_the_queue_from_its_source() {
    let repository = repository().await;
    let created = repository.create(queue(4, 1), NOW).await.expect("create");
    let selected = created.selected.expect("selection");
    let removed = items(&repository, created.id).await[3].0;
    repository
        .change(created.id, &PlayQueueChange::Remove { entry: removed })
        .await
        .expect("change")
        .expect("queue")
        .expect("applied");
    let shuffled = repository
        .change(created.id, &PlayQueueChange::Shuffle)
        .await
        .expect("change")
        .expect("queue")
        .expect("applied");
    assert!(shuffled.shuffled);
    assert_eq!(shuffled.total, 4, "the removed entry comes back");
    assert_eq!(shuffled.selected.expect("selection").id, selected.id);
    assert_eq!(shuffled.version, 3);

    let restored = repository
        .change(created.id, &PlayQueueChange::Unshuffle)
        .await
        .expect("change")
        .expect("queue")
        .expect("applied");
    let order: Vec<i64> = items(&repository, created.id)
        .await
        .iter()
        .map(|(_, item)| *item)
        .collect();
    assert_eq!(order, [1, 2, 3, 4]);
    assert_eq!(restored.selected.expect("selection").position, 1);
}

#[tokio::test]
async fn a_refused_edit_changes_nothing() {
    let repository = repository().await;
    let created = repository.create(queue(2, 0), NOW).await.expect("create");
    repository
        .change(
            created.id,
            &PlayQueueChange::Add {
                items: vec![RatingKey::new(9)],
                next: true,
            },
        )
        .await
        .expect("change");
    let refused = repository
        .change(created.id, &PlayQueueChange::Shuffle)
        .await
        .expect("change")
        .expect("queue");
    assert_eq!(refused, Err(ChangeRefusal::UpNext));
    assert_eq!(
        repository
            .queue(created.id)
            .await
            .expect("read")
            .expect("queue")
            .version,
        2
    );
    assert!(repository
        .change(99, &PlayQueueChange::Reset)
        .await
        .expect("change")
        .is_none());
}

#[tokio::test]
async fn a_reset_selects_the_first_entry_and_keeps_the_version() {
    let repository = repository().await;
    let created = repository.create(queue(3, 2), NOW).await.expect("create");
    let reset = repository
        .change(created.id, &PlayQueueChange::Reset)
        .await
        .expect("change")
        .expect("queue")
        .expect("applied");
    assert_eq!(reset.version, 1);
    assert_eq!(reset.selected.expect("selection").position, 0);
}
