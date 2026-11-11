//! Playlists: what a viewer keeps to play later, either items it chose or what a saved
//! listing finds.
//!
//! A playlist belongs to one viewer, and the items in it stay subject to that viewer's
//! rights: an entry whose item lies in a library the viewer cannot see is not shown, not
//! counted, and comes back if the library is given again.

use crate::ids::{PlaylistEntryId, PlaylistId, RatingKey, SectionId};
use crate::page::{Cursor, CursorKey, Direction, Page, Window};
use crate::watch::Viewer;

/// Distance between the positions of entries added one after another. Wide, so that many
/// moves between two neighbours fit before the playlist has to be renumbered.
pub const POSITION_GAP: i64 = 1024;

/// What a playlist plays, which decides what it may hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlaylistKind {
    Video,
    Audio,
    Photo,
}

/// A smart playlist's rule: a listing of one library, saved rather than its result.
///
/// `listing` is opaque here. The official API stores its own description of the listing —
/// filter, order and direction, in the shape its clients send them — and reads it back the
/// same way, so the rule can never say something that API cannot list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmartRule {
    pub section: SectionId,
    pub listing: String,
}

/// A field an editor can lock. Nothing refreshes a playlist's metadata here; the lock is kept
/// because clients show and set it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PlaylistField {
    Title,
    Summary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPlaylist {
    pub owner: Viewer,
    pub title: String,
    pub summary: String,
    pub kind: PlaylistKind,
    /// The first entries, in order. Repeats are dropped. Empty for a smart playlist.
    pub items: Vec<RatingKey>,
    pub rule: Option<SmartRule>,
    /// Where the content came from, as the client named it.
    pub source_uri: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Playlist {
    pub id: PlaylistId,
    pub owner: Viewer,
    /// Stable and unique across servers, unlike the id.
    pub guid: String,
    pub title: String,
    pub summary: String,
    pub kind: PlaylistKind,
    /// `None` for a playlist of chosen items.
    pub rule: Option<SmartRule>,
    pub source_uri: Option<String>,
    /// Ascending.
    pub locked: Vec<PlaylistField>,
    /// How many times it was played, counting its creation, as the reference counts it.
    pub view_count: u64,
    pub last_viewed_at: i64,
    pub added_at: i64,
    pub updated_at: i64,
}

impl Playlist {
    pub const fn is_smart(&self) -> bool {
        self.rule.is_some()
    }

    /// What a listing by title orders it by.
    pub fn sort_key(&self) -> String {
        sort_key(&self.title)
    }
}

/// A new playlist's guid. Random, since nothing about a playlist identifies it.
pub fn new_guid() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Folded so that ordering does not depend on a backend's collation.
pub fn sort_key(title: &str) -> String {
    title.to_lowercase()
}

/// One item at one place in a playlist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlaylistEntry {
    pub id: PlaylistEntryId,
    pub playlist: PlaylistId,
    pub item: RatingKey,
    /// Orders the entries; neither dense nor stable across moves.
    pub position: i64,
}

impl PlaylistEntry {
    /// Where a page resumes after this entry. A move changes it, as it changes the order.
    pub fn cursor(&self) -> Cursor {
        Cursor::new(CursorKey::Int(self.position), self.id.get())
    }
}

/// What a playlist of chosen items holds, of what its reader may see.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlaylistTotals {
    pub entries: u64,
    /// The items' own durations, summed. Items with none count for nothing.
    pub duration_ms: i64,
}

/// How a list of playlists is ordered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PlaylistSort {
    /// Smart playlists first, whichever the direction, then by title: the reference's order.
    #[default]
    Title,
    AddedAt,
    UpdatedAt,
    LastViewedAt,
}

/// Which of a viewer's playlists to list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlaylistFilter {
    pub kind: Option<PlaylistKind>,
    pub smart: Option<bool>,
    /// The playlists a library's page shows: those of the kind it plays, a smart one only if
    /// its rule lists that library. The kind is the caller's to set.
    pub library: Option<SectionId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistQuery {
    pub owner: Viewer,
    pub filter: PlaylistFilter,
    pub sort: PlaylistSort,
    pub direction: Direction,
}

impl PlaylistQuery {
    /// Where a page resumes after this playlist: its leading value, its title, its id.
    pub fn cursor(&self, playlist: &Playlist) -> Cursor {
        let leading = match self.sort {
            PlaylistSort::Title => i64::from(playlist.is_smart()),
            PlaylistSort::AddedAt => playlist.added_at,
            PlaylistSort::UpdatedAt => playlist.updated_at,
            PlaylistSort::LastViewedAt => playlist.last_viewed_at,
        };
        Cursor {
            keys: vec![
                CursorKey::Int(leading),
                CursorKey::Text(playlist.sort_key()),
            ],
            id: playlist.id.get(),
        }
    }
}

/// Changes to a playlist's own fields. `None` leaves a field alone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlaylistEdit {
    pub title: Option<String>,
    pub summary: Option<String>,
    pub lock: Vec<PlaylistField>,
    pub unlock: Vec<PlaylistField>,
}

/// Where a moved entry goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Destination {
    First,
    After(PlaylistEntryId),
}

#[derive(Debug, thiserror::Error)]
pub enum PlaylistError {
    #[error("no playlist with id {0}")]
    NotFound(PlaylistId),
    #[error("no entry {0} in this playlist")]
    EntryNotFound(PlaylistEntryId),
    #[error("playlist storage failed")]
    Storage(#[source] Box<dyn std::error::Error + Send + Sync>),
}

/// Where an entry goes between two neighbours, or `None` when they leave no room and the
/// playlist must be renumbered first.
pub fn position_between(previous: Option<i64>, next: Option<i64>) -> Option<i64> {
    match (previous, next) {
        (None, None) => Some(POSITION_GAP),
        (Some(previous), None) => previous.checked_add(POSITION_GAP),
        (None, Some(next)) => next.checked_sub(POSITION_GAP),
        (Some(previous), Some(next)) => {
            let middle = previous + (next - previous) / 2;
            (middle > previous && middle < next).then_some(middle)
        }
    }
}

/// The items to append, in order: those not in the playlist already, each once.
pub fn new_entries(items: &[RatingKey], present: &[RatingKey]) -> Vec<RatingKey> {
    let mut seen: std::collections::HashSet<RatingKey> = present.iter().copied().collect();
    items
        .iter()
        .copied()
        .filter(|item| seen.insert(*item))
        .collect()
}

#[async_trait::async_trait]
pub trait PlaylistRepository: Send + Sync + std::fmt::Debug {
    async fn create(&self, playlist: NewPlaylist, now: i64) -> Result<Playlist, PlaylistError>;

    async fn playlist(&self, id: PlaylistId) -> Result<Option<Playlist>, PlaylistError>;

    async fn playlists(
        &self,
        query: &PlaylistQuery,
        window: Window,
    ) -> Result<Page<Playlist>, PlaylistError>;

    async fn count(&self, query: &PlaylistQuery) -> Result<u64, PlaylistError>;

    async fn edit(
        &self,
        id: PlaylistId,
        edit: &PlaylistEdit,
        now: i64,
    ) -> Result<Playlist, PlaylistError>;

    /// Says whether there was one to delete.
    async fn delete(&self, id: PlaylistId) -> Result<bool, PlaylistError>;

    async fn entry(&self, id: PlaylistEntryId) -> Result<Option<PlaylistEntry>, PlaylistError>;

    /// A playlist's entries in order, of those in `sections`; `None` is every library.
    async fn entries(
        &self,
        playlist: PlaylistId,
        sections: Option<&[SectionId]>,
        window: Window,
    ) -> Result<Page<PlaylistEntry>, PlaylistError>;

    /// Totals per playlist, of the entries in `sections`. A playlist with no entry there has
    /// none in the answer.
    async fn totals(
        &self,
        playlists: &[PlaylistId],
        sections: Option<&[SectionId]>,
    ) -> Result<Vec<(PlaylistId, PlaylistTotals)>, PlaylistError>;

    /// Appends the items not in it already, and says how many that was.
    async fn append(
        &self,
        playlist: PlaylistId,
        items: &[RatingKey],
        now: i64,
    ) -> Result<u64, PlaylistError>;

    async fn remove(&self, entry: PlaylistEntryId, now: i64) -> Result<(), PlaylistError>;

    async fn move_entry(
        &self,
        entry: PlaylistEntryId,
        destination: Destination,
        now: i64,
    ) -> Result<(), PlaylistError>;

    async fn clear(&self, playlist: PlaylistId, now: i64) -> Result<(), PlaylistError>;

    async fn replace_rule(
        &self,
        playlist: PlaylistId,
        rule: SmartRule,
        source_uri: Option<String>,
        now: i64,
    ) -> Result<Playlist, PlaylistError>;

    /// Counts one more play.
    async fn record_play(&self, playlist: PlaylistId, now: i64) -> Result<(), PlaylistError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_after_the_last_and_inserts_before_the_first() {
        assert_eq!(position_between(None, None), Some(POSITION_GAP));
        assert_eq!(
            position_between(Some(2048), None),
            Some(2048 + POSITION_GAP)
        );
        assert_eq!(position_between(None, Some(1024)), Some(0));
    }

    #[test]
    fn a_move_between_two_entries_takes_the_middle() {
        assert_eq!(position_between(Some(1024), Some(2048)), Some(1536));
        assert_eq!(position_between(Some(-3), Some(1)), Some(-1));
    }

    #[test]
    fn neighbours_with_no_room_ask_for_a_renumbering() {
        assert_eq!(position_between(Some(5), Some(6)), None);
        assert_eq!(position_between(Some(i64::MAX), None), None);
    }

    #[test]
    fn keeps_each_new_item_once_in_the_order_given() {
        let keys = |ids: &[i64]| ids.iter().copied().map(RatingKey::new).collect::<Vec<_>>();
        assert_eq!(
            new_entries(&keys(&[3, 1, 3, 2, 1]), &keys(&[2])),
            keys(&[3, 1])
        );
    }
}
