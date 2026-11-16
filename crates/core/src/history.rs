//! What each viewer watched, one entry per viewing.
//!
//! Measured on the reference: an entry is written when a viewing is counted, and only then —
//! a playback crossing 90% of the runtime, or an item marked watched, each episode of a
//! season or series marked at once. A playback stopped short writes none, nor does a
//! position kept. Forgetting what was watched leaves the history as it is, and deleting an
//! entry leaves the watch state as it is: the two are written together, by
//! [`WatchRepository::record_viewings`](crate::watch::WatchRepository::record_viewings), and
//! read apart.

use crate::ids::{HistoryId, RatingKey, SectionId, UserId};
use crate::page::{Cursor, CursorKey, Direction};

/// How a viewing came about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewingKind {
    /// A playback crossed the end.
    Played,
    /// Marked watched, without playing.
    Marked,
}

/// One viewing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryEntry {
    pub id: HistoryId,
    pub viewer: UserId,
    /// The copy watched: the item, not the work, so the entry names the library it was in.
    pub item: RatingKey,
    /// The item's library.
    pub section: SectionId,
    pub kind: ViewingKind,
    /// In seconds since the Unix epoch.
    pub viewed_at: i64,
}

/// A viewing to add to a viewer's history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewViewing {
    pub item: RatingKey,
    pub kind: ViewingKind,
    pub viewed_at: i64,
}

/// Whose viewings a query reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryScope {
    /// Every viewer's: an owner's view.
    Everyone,
    Viewer(UserId),
}

/// How a history is ordered. Ties go to the order entries were written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HistoryOrder {
    /// The order entries were written in, which is the reference's default.
    #[default]
    Recorded,
    ViewedAt,
    /// By the library's identifier.
    Library,
}

/// Which entries to read, and in what order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryQuery {
    pub scope: HistoryScope,
    /// This item's entries; for a series or a season, its episodes' too.
    pub item: Option<RatingKey>,
    /// Only entries of items in these libraries. `None` is no restriction.
    pub sections: Option<Vec<SectionId>>,
    /// Only viewings of this kind. `None` is both.
    pub kind: Option<ViewingKind>,
    /// Only viewings at or after this second.
    pub viewed_since: Option<i64>,
    /// Only viewings at or before this second.
    pub viewed_until: Option<i64>,
    pub order: HistoryOrder,
    pub direction: Direction,
}

impl HistoryQuery {
    /// Every entry within the scope, in the order written.
    pub const fn of(scope: HistoryScope) -> Self {
        Self {
            scope,
            item: None,
            sections: None,
            kind: None,
            viewed_since: None,
            viewed_until: None,
            order: HistoryOrder::Recorded,
            direction: Direction::Ascending,
        }
    }

    /// Where an entry stands in this order, for a cursor to resume from.
    pub fn cursor(&self, entry: &HistoryEntry) -> Cursor {
        match self.order {
            HistoryOrder::Recorded => Cursor::by_id(entry.id.get()),
            HistoryOrder::ViewedAt => Cursor::new(CursorKey::Int(entry.viewed_at), entry.id.get()),
            HistoryOrder::Library => {
                Cursor::new(CursorKey::Int(entry.section.get()), entry.id.get())
            }
        }
    }

    /// Whether an entry is one this query reads, the item's lineage aside: `parents` are the
    /// entry's item's parent and grandparent, nearest first.
    pub fn admits(&self, entry: &HistoryEntry, parents: &[RatingKey]) -> bool {
        let in_scope = match self.scope {
            HistoryScope::Everyone => true,
            HistoryScope::Viewer(viewer) => entry.viewer == viewer,
        };
        in_scope
            && self
                .item
                .is_none_or(|item| entry.item == item || parents.contains(&item))
            && self
                .sections
                .as_ref()
                .is_none_or(|sections| sections.contains(&entry.section))
            && self.kind.is_none_or(|kind| entry.kind == kind)
            && self
                .viewed_since
                .is_none_or(|since| entry.viewed_at >= since)
            && self
                .viewed_until
                .is_none_or(|until| entry.viewed_at <= until)
    }
}

/// How many viewings one viewer has of a work.
///
/// What the reference's `/library/metadata/{id}/users/top` ranks, read from its binary: the
/// viewings of every copy of the work, and of the episodes below it for a series or a season,
/// counted per account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewerCount {
    pub viewer: UserId,
    pub viewings: u64,
}

impl ViewerCount {
    /// Where this count stands in a ranking: most viewings first, then by user.
    pub fn cursor(&self) -> Cursor {
        Cursor::new(
            CursorKey::Int(i64::try_from(self.viewings).unwrap_or(i64::MAX)),
            self.viewer.get(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(viewer: UserId, item: i64, kind: ViewingKind, viewed_at: i64) -> HistoryEntry {
        HistoryEntry {
            id: HistoryId::new(1),
            viewer,
            item: RatingKey::new(item),
            section: SectionId::new(28),
            kind,
            viewed_at,
        }
    }

    #[test]
    fn a_series_reads_its_episodes_entries() {
        let mut query = HistoryQuery::of(HistoryScope::Viewer(UserId::new(1)));
        query.item = Some(RatingKey::new(577_987));
        let episode = entry(UserId::new(1), 577_989, ViewingKind::Played, 10);
        let lineage = [RatingKey::new(577_988), RatingKey::new(577_987)];
        assert!(query.admits(&episode, &lineage));
        assert!(!query.admits(&episode, &[]));
    }

    #[test]
    fn one_viewer_reads_only_its_own() {
        let query = HistoryQuery::of(HistoryScope::Viewer(UserId::new(1)));
        let theirs = entry(UserId::new(7), 577_986, ViewingKind::Played, 10);
        assert!(!query.admits(&theirs, &[]));
        assert!(HistoryQuery::of(HistoryScope::Everyone).admits(&theirs, &[]));
    }

    #[test]
    fn dates_bound_inclusively() {
        let mut query = HistoryQuery::of(HistoryScope::Everyone);
        query.viewed_since = Some(10);
        query.viewed_until = Some(10);
        query.kind = Some(ViewingKind::Marked);
        assert!(query.admits(&entry(UserId::new(1), 1, ViewingKind::Marked, 10), &[]));
        assert!(!query.admits(&entry(UserId::new(1), 1, ViewingKind::Played, 10), &[]));
        assert!(!query.admits(&entry(UserId::new(1), 1, ViewingKind::Marked, 11), &[]));
    }
}
