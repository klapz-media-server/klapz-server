//! What a player will play next.
//!
//! A queue is a snapshot: the items it was created from, in order, whatever happens to the
//! library afterwards.

use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;

use crate::ids::RatingKey;
use crate::page::{Page, Window};

/// How long a queue is kept. Nothing says when a player is done with one.
pub const QUEUE_RETENTION_SECONDS: i64 = 7 * 24 * 3600;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPlayQueue {
    /// Where the items came from, as the client named it.
    pub source_uri: String,
    pub items: Vec<RatingKey>,
    /// The position of the entry to play first. Ignored by an empty queue.
    pub selected: usize,
    pub shuffled: bool,
}

impl NewPlayQueue {
    /// Puts the items in random order. The first is then the one played first.
    #[must_use]
    pub fn shuffle(mut self) -> Self {
        let order = RandomState::new();
        self.items
            .sort_by_cached_key(|item| order.hash_one(item.get()));
        self.selected = 0;
        self.shuffled = true;
        self
    }
}

/// One position in a queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayQueueEntry {
    pub id: i64,
    pub queue: i64,
    pub position: i64,
    pub item: RatingKey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayQueue {
    pub id: i64,
    pub source_uri: String,
    pub shuffled: bool,
    pub version: i32,
    pub total: u64,
    /// `None` for an empty queue.
    pub selected: Option<PlayQueueEntry>,
}

#[derive(Debug, thiserror::Error)]
pub enum PlayQueueError {
    #[error("play queue storage failed")]
    Storage(#[source] Box<dyn std::error::Error + Send + Sync>),
}

#[async_trait::async_trait]
pub trait PlayQueueRepository: Send + Sync + std::fmt::Debug {
    async fn create(&self, queue: NewPlayQueue, now: i64) -> Result<PlayQueue, PlayQueueError>;

    async fn queue(&self, id: i64) -> Result<Option<PlayQueue>, PlayQueueError>;

    async fn entry(&self, id: i64) -> Result<Option<PlayQueueEntry>, PlayQueueError>;

    /// A queue's entries, in play order.
    async fn entries(
        &self,
        queue: i64,
        window: Window,
    ) -> Result<Page<PlayQueueEntry>, PlayQueueError>;

    /// Deletes the queues created before `before`, and says how many there were.
    async fn delete_created_before(&self, before: i64) -> Result<u64, PlayQueueError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shuffling_keeps_every_item_and_plays_the_first() {
        let items: Vec<RatingKey> = (1..=50).map(RatingKey::new).collect();
        let queue = NewPlayQueue {
            source_uri: String::new(),
            items: items.clone(),
            selected: 7,
            shuffled: false,
        }
        .shuffle();
        assert!(queue.shuffled);
        assert_eq!(queue.selected, 0);
        assert_ne!(queue.items, items);
        let mut sorted = queue.items;
        sorted.sort_by_key(|item| item.get());
        assert_eq!(sorted, items);
    }
}
