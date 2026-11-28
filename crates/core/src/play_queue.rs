//! What a player will play next.
//!
//! A queue is built from a source — the items a client named, in their natural order — and
//! edited afterwards: items queued by hand, moved, removed. Shuffling rebuilds it from that
//! source, as the reference does, so what was edited is lost. Every rule below was measured on
//! the reference (`docs/plex/spec-gaps.md`).

use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;

use crate::ids::{PlaylistId, RatingKey, UserId};
use crate::page::{Page, Window, MAX_ITEMS};

/// How long a queue is kept. Nothing says when a player is done with one.
pub const QUEUE_RETENTION_SECONDS: i64 = 7 * 24 * 3600;

/// What a queue plays. A client keeps one queue of each kind: creating another replaces it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlayQueueKind {
    #[default]
    Video,
    Audio,
    Photo,
}

impl PlayQueueKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Video => "video",
            Self::Audio => "audio",
            Self::Photo => "photo",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "video" => Some(Self::Video),
            "audio" => Some(Self::Audio),
            "photo" => Some(Self::Photo),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPlayQueue {
    pub user: UserId,
    /// The player it was made for. Its previous queue of the same kind is deleted; a queue
    /// made for no player in particular replaces nothing.
    pub client: Option<String>,
    pub kind: PlayQueueKind,
    /// Where the items came from, as the client named it.
    pub source_uri: String,
    /// The items, in their natural order. Shuffling and unshuffling rebuild the queue from
    /// them, so they are kept for its whole life.
    pub source: Vec<RatingKey>,
    /// The position in `source` of the item to play first. Ignored by an empty queue.
    pub selected: usize,
    /// Starts the queue in random order, with the selected item first.
    pub shuffle: bool,
    /// The playlist it plays, when it plays one.
    pub playlist: Option<PlaylistId>,
}

impl NewPlayQueue {
    /// The entries to store, and which one is selected.
    #[must_use]
    pub fn initial_contents(&self) -> PlayQueueContents {
        let source = &self.source[..self.source.len().min(MAX_ITEMS as usize)];
        let selected = (!source.is_empty()).then(|| self.selected.min(source.len() - 1));
        let mut contents = PlayQueueContents {
            slots: source.iter().map(|item| Slot::new(*item)).collect(),
            selected,
            last_added: None,
            shuffled: false,
        };
        if self.shuffle && selected.is_some() {
            contents.rebuild(source, true);
        }
        contents
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
    pub user: UserId,
    pub client: Option<String>,
    pub kind: PlayQueueKind,
    pub source_uri: String,
    pub shuffled: bool,
    pub version: i32,
    pub total: u64,
    /// `None` for an empty queue.
    pub selected: Option<PlayQueueEntry>,
    /// The end of what was queued by hand, where the next addition goes.
    pub last_added: Option<PlayQueueEntry>,
    pub playlist: Option<PlaylistId>,
}

/// One entry as an edit sees it: stored already, or about to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    /// `None` until it is stored and given an id.
    pub entry: Option<i64>,
    pub item: RatingKey,
}

impl Slot {
    #[must_use]
    pub const fn new(item: RatingKey) -> Self {
        Self { entry: None, item }
    }
}

/// A queue's entries in play order, with the two positions its edits steer by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayQueueContents {
    pub slots: Vec<Slot>,
    /// Index of the entry playing. `None` only when the queue is empty.
    pub selected: Option<usize>,
    /// Index of the last entry queued by hand. While there is one, the queue has an "up next"
    /// area and cannot be shuffled.
    pub last_added: Option<usize>,
    pub shuffled: bool,
}

/// An edit to a queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayQueueChange {
    /// Queues items by hand: after the last ones queued that way, or right after the entry
    /// playing with `next`.
    Add {
        items: Vec<RatingKey>,
        next: bool,
    },
    /// Moves an entry after another, or first.
    Move {
        entry: i64,
        after: Option<i64>,
    },
    Remove {
        entry: i64,
    },
    /// Removes every entry but the one playing.
    Clear,
    /// Makes the first entry the one playing.
    Reset,
    /// Rebuilds the queue from its source in random order, the entry playing first.
    Shuffle,
    /// Rebuilds the queue from its source in natural order, the entry playing where it falls.
    Unshuffle,
}

/// Why an edit did nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ChangeRefusal {
    #[error("no such entry in this queue")]
    EntryNotFound,
    #[error("the entry to move after is not in this queue")]
    AfterNotFound,
    #[error("an entry cannot be moved after itself")]
    OntoItself,
    #[error("a queue with entries queued by hand cannot be reordered")]
    UpNext,
    #[error("an empty queue cannot be reordered")]
    Empty,
}

impl PlayQueueChange {
    /// Applies the edit, and says whether it raises the queue's version: everything does but a
    /// reset, and an addition of nothing.
    ///
    /// # Errors
    /// The edit names an entry the queue does not hold, or cannot apply to this queue.
    pub fn apply(
        &self,
        contents: &mut PlayQueueContents,
        source: &[RatingKey],
    ) -> Result<bool, ChangeRefusal> {
        match self {
            Self::Add { items, next } => Ok(contents.add(items, *next)),
            Self::Move { entry, after } => contents.move_entry(*entry, *after).map(|()| true),
            Self::Remove { entry } => {
                let index = contents.index_of(*entry)?;
                contents.remove_at(index);
                Ok(true)
            }
            Self::Clear => {
                contents.clear();
                Ok(true)
            }
            Self::Reset => {
                if !contents.slots.is_empty() {
                    contents.selected = Some(0);
                }
                Ok(false)
            }
            Self::Shuffle | Self::Unshuffle => {
                if contents.last_added.is_some() {
                    return Err(ChangeRefusal::UpNext);
                }
                if contents.selected.is_none() {
                    return Err(ChangeRefusal::Empty);
                }
                contents.rebuild(source, matches!(self, Self::Shuffle));
                Ok(true)
            }
        }
    }
}

impl PlayQueueContents {
    fn index_of(&self, entry: i64) -> Result<usize, ChangeRefusal> {
        self.slots
            .iter()
            .position(|slot| slot.entry == Some(entry))
            .ok_or(ChangeRefusal::EntryNotFound)
    }

    fn add(&mut self, items: &[RatingKey], next: bool) -> bool {
        let room = (MAX_ITEMS as usize).saturating_sub(self.slots.len());
        let items = &items[..items.len().min(room)];
        if items.is_empty() {
            return false;
        }
        let anchor = if next {
            self.selected
        } else {
            self.last_added.or(self.selected)
        };
        let at = anchor.map_or(self.slots.len(), |index| index + 1);
        self.insert_at(at, items);
        let last = at + items.len() - 1;
        if !next || self.last_added.is_none() {
            self.last_added = Some(last);
        }
        if self.selected.is_none() {
            self.selected = Some(at);
        }
        true
    }

    fn insert_at(&mut self, at: usize, items: &[RatingKey]) {
        let shift = |index: Option<usize>| index.map(|i| if i >= at { i + items.len() } else { i });
        self.selected = shift(self.selected);
        self.last_added = shift(self.last_added);
        self.slots
            .splice(at..at, items.iter().map(|item| Slot::new(*item)));
    }

    /// Takes an entry out. The entry after a removed selection plays instead (the one before,
    /// for the last), and the entry before the last queued by hand ends that area instead.
    fn remove_at(&mut self, index: usize) -> Slot {
        let slot = self.slots.remove(index);
        let remaining = self.slots.len();
        self.selected = self.selected.and_then(|i| match i.cmp(&index) {
            std::cmp::Ordering::Less => Some(i),
            std::cmp::Ordering::Equal => (remaining > 0).then(|| index.min(remaining - 1)),
            std::cmp::Ordering::Greater => Some(i - 1),
        });
        self.last_added = self.last_added.and_then(|i| match i.cmp(&index) {
            std::cmp::Ordering::Less => Some(i),
            std::cmp::Ordering::Equal => index.checked_sub(1),
            std::cmp::Ordering::Greater => Some(i - 1),
        });
        slot
    }

    fn move_entry(&mut self, entry: i64, after: Option<i64>) -> Result<(), ChangeRefusal> {
        let from = self.index_of(entry)?;
        if after == Some(entry) {
            return Err(ChangeRefusal::OntoItself);
        }
        if let Some(after) = after {
            self.index_of(after)
                .map_err(|_| ChangeRefusal::AfterNotFound)?;
        }
        let was_selected = self.selected == Some(from);
        let slot = self.remove_at(from);
        let after_index = after.map(|after| self.index_of(after)).transpose()?;
        let at = after_index.map_or(0, |index| index + 1);
        let extends_up_next = after_index.is_some() && after_index == self.last_added;
        self.selected = self.selected.map(|i| if i >= at { i + 1 } else { i });
        self.last_added = self.last_added.map(|i| if i >= at { i + 1 } else { i });
        self.slots.insert(at, slot);
        if was_selected {
            self.selected = Some(at);
        }
        if extends_up_next {
            self.last_added = Some(at);
        }
        Ok(())
    }

    fn clear(&mut self) {
        self.slots = self
            .selected
            .and_then(|index| self.slots.get(index).copied())
            .into_iter()
            .collect();
        self.selected = (!self.slots.is_empty()).then_some(0);
        self.last_added = None;
    }

    /// The source again, the entry playing kept: first when shuffled, at the first place its
    /// item holds otherwise. Every other entry is new, as on the reference, where they come
    /// back with new ids. An entry playing that the source never held — one queued by hand —
    /// keeps its id and takes the item that comes first instead, as the reference does.
    fn rebuild(&mut self, source: &[RatingKey], shuffle: bool) {
        let Some(mut selected) = self
            .selected
            .and_then(|index| self.slots.get(index).copied())
        else {
            return;
        };
        let source = &source[..source.len().min(MAX_ITEMS as usize)];
        let mut rest: Vec<RatingKey> = source.to_vec();
        let natural = rest.iter().position(|item| *item == selected.item);
        if let Some(index) = natural {
            rest.remove(index);
        }
        if shuffle {
            let order = RandomState::new();
            rest.sort_by_cached_key(|item| order.hash_one(item.get()));
        }
        if natural.is_none() && !rest.is_empty() {
            selected.item = rest.remove(0);
        }
        let at = if shuffle { 0 } else { natural.unwrap_or(0) };
        self.slots = rest.into_iter().map(Slot::new).collect();
        self.slots.insert(at, selected);
        self.selected = Some(at);
        self.last_added = None;
        self.shuffled = shuffle;
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PlayQueueError {
    #[error("play queue storage failed")]
    Storage(#[source] Box<dyn std::error::Error + Send + Sync>),
}

/// The outcome of an edit: `None` when the queue does not exist.
pub type ChangeOutcome = Option<Result<PlayQueue, ChangeRefusal>>;

#[async_trait::async_trait]
pub trait PlayQueueRepository: Send + Sync + std::fmt::Debug {
    /// Stores a queue, deleting the previous one of its user, player and kind.
    async fn create(&self, queue: NewPlayQueue, now: i64) -> Result<PlayQueue, PlayQueueError>;

    async fn queue(&self, id: i64) -> Result<Option<PlayQueue>, PlayQueueError>;

    async fn entry(&self, id: i64) -> Result<Option<PlayQueueEntry>, PlayQueueError>;

    /// A queue's entries, in play order.
    async fn entries(
        &self,
        queue: i64,
        window: Window,
    ) -> Result<Page<PlayQueueEntry>, PlayQueueError>;

    /// Applies an edit atomically, raising the version when the edit says so.
    async fn change(
        &self,
        queue: i64,
        change: &PlayQueueChange,
    ) -> Result<ChangeOutcome, PlayQueueError>;

    /// Deletes the queues created before `before`, and says how many there were.
    async fn delete_created_before(&self, before: i64) -> Result<u64, PlayQueueError>;
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn keys(values: &[i64]) -> Vec<RatingKey> {
        values.iter().copied().map(RatingKey::new).collect()
    }

    /// A stored queue whose entry ids are 100 + the item, for reading the order back.
    fn stored(items: &[i64], selected: usize) -> PlayQueueContents {
        PlayQueueContents {
            slots: items
                .iter()
                .map(|item| Slot {
                    entry: Some(100 + item),
                    item: RatingKey::new(*item),
                })
                .collect(),
            selected: Some(selected),
            last_added: None,
            shuffled: false,
        }
    }

    fn items(contents: &PlayQueueContents) -> Vec<i64> {
        contents.slots.iter().map(|slot| slot.item.get()).collect()
    }

    fn add(contents: &mut PlayQueueContents, items: &[i64], next: bool) {
        PlayQueueChange::Add {
            items: keys(items),
            next,
        }
        .apply(contents, &[])
        .unwrap();
        for slot in &mut contents.slots {
            slot.entry.get_or_insert(100 + slot.item.get());
        }
    }

    fn move_entry(contents: &mut PlayQueueContents, item: i64, after: Option<i64>) {
        PlayQueueChange::Move {
            entry: 100 + item,
            after: after.map(|after| 100 + after),
        }
        .apply(contents, &[])
        .unwrap();
    }

    #[test]
    fn a_new_queue_selects_what_it_was_told_to() {
        let queue = NewPlayQueue {
            user: crate::UserId::new(1),
            client: None,
            kind: PlayQueueKind::Video,
            source_uri: String::new(),
            source: keys(&[1, 2, 3]),
            selected: 1,
            shuffle: false,
            playlist: None,
        };
        let contents = queue.initial_contents();
        assert_eq!(items(&contents), [1, 2, 3]);
        assert_eq!(contents.selected, Some(1));
    }

    #[test]
    fn shuffling_on_creation_plays_the_selected_item_first() {
        let queue = NewPlayQueue {
            user: crate::UserId::new(1),
            client: None,
            kind: PlayQueueKind::Video,
            source_uri: String::new(),
            source: (1..=50).map(RatingKey::new).collect(),
            selected: 7,
            shuffle: true,
            playlist: None,
        };
        let contents = queue.initial_contents();
        assert!(contents.shuffled);
        assert_eq!(contents.selected, Some(0));
        assert_eq!(contents.slots[0].item, RatingKey::new(8));
        let mut sorted = items(&contents);
        sorted.sort_unstable();
        assert_eq!(sorted, (1..=50).collect::<Vec<_>>());
    }

    // The sequences below replay queue 413711 on the reference: Bull's two episodes, then Soul
    // added at the end, Soul played next, the second episode at the end.
    #[test]
    fn additions_go_after_the_last_queued_by_hand_and_next_right_after_the_selection() {
        let mut contents = stored(&[1, 2], 0);
        add(&mut contents, &[9], false);
        assert_eq!(items(&contents), [1, 9, 2]);
        add(&mut contents, &[8], true);
        assert_eq!(items(&contents), [1, 8, 9, 2]);
        assert_eq!(
            contents.last_added,
            Some(2),
            "next leaves the area's end alone"
        );
        add(&mut contents, &[7], false);
        assert_eq!(items(&contents), [1, 8, 9, 7, 2]);
        assert_eq!(contents.last_added, Some(3));
    }

    #[test]
    fn playing_next_in_a_queue_without_additions_starts_the_area() {
        let mut contents = stored(&[1, 2], 0);
        add(&mut contents, &[9], true);
        add(&mut contents, &[8], false);
        assert_eq!(items(&contents), [1, 9, 8, 2]);
    }

    #[test]
    fn several_items_keep_their_order() {
        let mut contents = stored(&[1, 2], 0);
        add(&mut contents, &[7, 8], true);
        add(&mut contents, &[5, 6], false);
        assert_eq!(items(&contents), [1, 7, 8, 5, 6, 2]);
        assert_eq!(contents.last_added, Some(4));
    }

    #[test]
    fn an_addition_follows_the_area_even_behind_the_selection() {
        let mut contents = stored(&[1, 2], 0);
        add(&mut contents, &[9], false);
        move_entry(&mut contents, 1, Some(2));
        assert_eq!(items(&contents), [9, 2, 1]);
        assert_eq!(contents.last_added, Some(0));
        add(&mut contents, &[8], false);
        assert_eq!(items(&contents), [9, 8, 2, 1]);
    }

    #[test]
    fn an_empty_queue_plays_the_first_item_added() {
        let mut contents = PlayQueueContents {
            slots: Vec::new(),
            selected: None,
            last_added: None,
            shuffled: false,
        };
        add(&mut contents, &[1, 2], true);
        assert_eq!(contents.selected, Some(0));
        assert_eq!(contents.last_added, Some(1));
    }

    #[test]
    fn adding_nothing_raises_nothing() {
        let mut contents = stored(&[1], 0);
        let raised = PlayQueueChange::Add {
            items: Vec::new(),
            next: false,
        }
        .apply(&mut contents, &[])
        .unwrap();
        assert!(!raised);
    }

    #[test]
    fn moving_keeps_the_selection_on_its_entry() {
        let mut contents = stored(&[1, 2, 3], 0);
        move_entry(&mut contents, 3, None);
        assert_eq!(items(&contents), [3, 1, 2]);
        assert_eq!(contents.selected, Some(1));
        move_entry(&mut contents, 1, Some(2));
        assert_eq!(items(&contents), [3, 2, 1]);
        assert_eq!(contents.selected, Some(2));
    }

    #[test]
    fn moving_after_the_area_s_end_extends_it_and_moving_its_end_shortens_it() {
        let mut contents = stored(&[1, 2, 3], 0);
        add(&mut contents, &[8, 9], false);
        assert_eq!(items(&contents), [1, 8, 9, 2, 3]);
        move_entry(&mut contents, 9, None);
        assert_eq!(contents.last_added, Some(2), "8, before it");
        move_entry(&mut contents, 3, Some(8));
        assert_eq!(items(&contents), [9, 1, 8, 3, 2]);
        assert_eq!(contents.last_added, Some(3), "3, now after 8");
    }

    #[test]
    fn a_move_is_refused_for_an_unknown_entry_or_onto_itself() {
        let mut contents = stored(&[1, 2], 0);
        let refusal = |change: PlayQueueChange, contents: &mut PlayQueueContents| {
            change.apply(contents, &[]).unwrap_err()
        };
        assert_eq!(
            refusal(
                PlayQueueChange::Move {
                    entry: 999,
                    after: None
                },
                &mut contents
            ),
            ChangeRefusal::EntryNotFound
        );
        assert_eq!(
            refusal(
                PlayQueueChange::Move {
                    entry: 101,
                    after: Some(101)
                },
                &mut contents
            ),
            ChangeRefusal::OntoItself
        );
        assert_eq!(
            refusal(
                PlayQueueChange::Move {
                    entry: 101,
                    after: Some(999)
                },
                &mut contents
            ),
            ChangeRefusal::AfterNotFound
        );
        assert_eq!(items(&contents), [1, 2]);
    }

    #[test]
    fn removing_the_selection_plays_the_next_entry() {
        let mut contents = stored(&[1, 2, 3], 1);
        PlayQueueChange::Remove { entry: 102 }
            .apply(&mut contents, &[])
            .unwrap();
        assert_eq!(items(&contents), [1, 3]);
        assert_eq!(contents.selected, Some(1));
        PlayQueueChange::Remove { entry: 103 }
            .apply(&mut contents, &[])
            .unwrap();
        assert_eq!(contents.selected, Some(0));
    }

    #[test]
    fn removing_the_area_s_end_moves_it_back_one() {
        let mut contents = stored(&[1, 2], 0);
        add(&mut contents, &[8, 9], false);
        assert_eq!(items(&contents), [1, 8, 9, 2]);
        PlayQueueChange::Remove { entry: 109 }
            .apply(&mut contents, &[])
            .unwrap();
        assert_eq!(contents.last_added, Some(1));
    }

    #[test]
    fn clearing_keeps_the_entry_playing_and_ends_the_area() {
        let mut contents = stored(&[1, 2, 3], 1);
        add(&mut contents, &[9], false);
        PlayQueueChange::Clear.apply(&mut contents, &[]).unwrap();
        assert_eq!(items(&contents), [2]);
        assert_eq!(contents.selected, Some(0));
        assert_eq!(contents.last_added, None);
    }

    #[test]
    fn a_reset_selects_the_first_entry_without_raising_the_version() {
        let mut contents = stored(&[1, 2, 3], 2);
        let raised = PlayQueueChange::Reset.apply(&mut contents, &[]).unwrap();
        assert!(!raised);
        assert_eq!(contents.selected, Some(0));
    }

    #[test]
    fn shuffling_rebuilds_from_the_source_with_the_selection_first() {
        let source = keys(&[1, 2, 3, 4]);
        let mut contents = stored(&[3], 0);
        PlayQueueChange::Shuffle
            .apply(&mut contents, &source)
            .unwrap();
        assert!(contents.shuffled);
        assert_eq!(contents.slots[0].entry, Some(103));
        assert_eq!(contents.selected, Some(0));
        assert!(contents.slots[1..].iter().all(|slot| slot.entry.is_none()));
        let mut sorted = items(&contents);
        sorted.sort_unstable();
        assert_eq!(sorted, [1, 2, 3, 4]);
    }

    #[test]
    fn unshuffling_puts_the_selection_back_in_its_place() {
        let source = keys(&[1, 2, 3, 4]);
        let mut contents = stored(&[3, 1, 4, 2], 0);
        contents.shuffled = true;
        PlayQueueChange::Unshuffle
            .apply(&mut contents, &source)
            .unwrap();
        assert!(!contents.shuffled);
        assert_eq!(items(&contents), [1, 2, 3, 4]);
        assert_eq!(contents.selected, Some(2));
        assert_eq!(contents.slots[2].entry, Some(103));
    }

    // Queue 413740's successor on the reference: Soul queued by hand, then everything else
    // cleared, then shuffled — Soul's entry came back playing the first episode.
    #[test]
    fn an_entry_playing_outside_the_source_takes_its_first_item() {
        let source = keys(&[1, 2]);
        let mut contents = stored(&[9], 0);
        PlayQueueChange::Unshuffle
            .apply(&mut contents, &source)
            .unwrap();
        assert_eq!(items(&contents), [1, 2]);
        assert_eq!(contents.slots[0].entry, Some(109));
        assert_eq!(contents.selected, Some(0));
    }

    #[test]
    fn a_queue_with_an_up_next_area_or_nothing_cannot_be_reordered() {
        let mut contents = stored(&[1, 2], 0);
        add(&mut contents, &[9], false);
        assert_eq!(
            PlayQueueChange::Shuffle.apply(&mut contents, &[]),
            Err(ChangeRefusal::UpNext)
        );
        let mut empty = PlayQueueContents {
            slots: Vec::new(),
            selected: None,
            last_added: None,
            shuffled: false,
        };
        assert_eq!(
            PlayQueueChange::Unshuffle.apply(&mut empty, &[]),
            Err(ChangeRefusal::Empty)
        );
    }

    #[test]
    fn additions_stop_at_the_queue_s_capacity() {
        let full: Vec<i64> = (1..i64::from(MAX_ITEMS)).collect();
        let mut contents = stored(&full, 0);
        add(&mut contents, &[9001, 9002], false);
        assert_eq!(contents.slots.len(), MAX_ITEMS as usize);
    }
}
