//! What was recently added to a series library, as the reference groups it.
//!
//! A series library gains episodes, but a client showing them one by one would bury the
//! week's new series under its thirty episodes. The reference groups them into **batches**:
//! walking a series' episodes from the newest, an episode joins the batch when it was added
//! within twenty hours of the batch's oldest one. A batch spanning several seasons stands for
//! the series, several episodes of one season for the season, a lone episode for itself.
//! Batches are listed by their newest episode, and an item appears once, at its newest batch.
//!
//! Measured against the reference's own shelves on four series libraries, fifty entries each:
//! a window anywhere from 19 h 16 to 20 h 15 reproduces all 200 entries in place, 20 h 16
//! already misplaces 23, 19 h 10 misplaces 53. Twenty hours is the round value inside the
//! range. Chaining is what makes it a window per batch rather than per pair: two episodes 34
//! hours apart share a batch when a third sits between them.

use crate::ids::{RatingKey, SectionId};
use crate::metadata::{MetadataError, MetadataRepository};

/// The window within which an episode joins the batch of the one added after it.
pub const BATCH_WINDOW_SECONDS: i64 = 20 * 3600;

/// How many episode additions to read per storage call while grouping.
const ADDITIONS_PAGE: u32 = 500;

/// When an episode was added, and where it sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EpisodeAddition {
    pub episode: RatingKey,
    pub season: RatingKey,
    pub show: RatingKey,
    pub added_at: i64,
}

/// What one entry of a "recently added" shelf stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RecentEntry {
    Show(RatingKey),
    Season(RatingKey),
    Episode(RatingKey),
}

impl RecentEntry {
    pub const fn item(self) -> RatingKey {
        match self {
            Self::Show(id) | Self::Season(id) | Self::Episode(id) => id,
        }
    }
}

/// An entry, with when its newest episode was added.
///
/// That time, not the item's own, is the entry's: a season created months ago that gained
/// a batch of episodes today was added, as far as the shelf is concerned, today. Measured: the
/// reference dates a grouped season by its batch's newest episode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecentAddition {
    pub entry: RecentEntry,
    pub added_at: i64,
}

/// The first `count` entries of a series library's "recently added".
///
/// Reads episodes page by page until those entries are settled: an entry is settled once no
/// unread episode could still join its batch, which is when the oldest episode read is more
/// than the window older than the batch.
///
/// # Errors
///
/// Fails when storage does.
pub async fn recently_added_episodes(
    storage: &dyn MetadataRepository,
    section: SectionId,
    count: usize,
) -> Result<Vec<RecentAddition>, MetadataError> {
    let mut additions: Vec<EpisodeAddition> = Vec::new();
    loop {
        let after = additions.last().map(|last| (last.added_at, last.episode));
        let page = storage
            .episode_additions(section, after, ADDITIONS_PAGE)
            .await?;
        let exhausted = page.len() < ADDITIONS_PAGE as usize;
        additions.extend(page);

        let horizon = if exhausted {
            i64::MIN
        } else {
            additions.last().map_or(i64::MIN, |last| last.added_at)
        };
        let (entries, settled) = group(&additions, count, horizon);
        if settled || exhausted {
            return Ok(entries);
        }
    }
}

/// One batch while grouping: the newest and oldest addition times, and what it holds.
#[derive(Debug)]
struct Batch {
    newest: i64,
    oldest: i64,
    first_season: RatingKey,
    show: RatingKey,
    first_episode: RatingKey,
    episodes: usize,
    several_seasons: bool,
}

impl Batch {
    const fn entry(&self) -> RecentEntry {
        if self.several_seasons {
            RecentEntry::Show(self.show)
        } else if self.episodes > 1 {
            RecentEntry::Season(self.first_season)
        } else {
            RecentEntry::Episode(self.first_episode)
        }
    }
}

/// Groups additions, newest first, into the first `count` distinct entries.
///
/// `horizon` is the time of the oldest addition read, or `i64::MIN` when nothing is left to
/// read. The second value says whether those entries are final: they are when every batch
/// they come from closed before the horizon, so no unread episode can join it.
fn group(additions: &[EpisodeAddition], count: usize, horizon: i64) -> (Vec<RecentAddition>, bool) {
    let mut batches: Vec<Batch> = Vec::new();
    // The open batch of each series, by index into `batches`.
    let mut open: std::collections::HashMap<RatingKey, usize> = std::collections::HashMap::new();

    for addition in additions {
        let joins = open
            .get(&addition.show)
            .copied()
            .filter(|&index| batches[index].oldest - addition.added_at <= BATCH_WINDOW_SECONDS);
        if let Some(index) = joins {
            let batch = &mut batches[index];
            batch.oldest = addition.added_at;
            batch.episodes += 1;
            batch.several_seasons |= batch.first_season != addition.season;
        } else {
            open.insert(addition.show, batches.len());
            batches.push(Batch {
                newest: addition.added_at,
                oldest: addition.added_at,
                first_season: addition.season,
                show: addition.show,
                first_episode: addition.episode,
                episodes: 1,
                several_seasons: false,
            });
        }
    }

    // Additions arrive newest first, so batches are created in the order of their newest
    // episode already; the sort only settles ties.
    batches.sort_by(|left, right| right.newest.cmp(&left.newest));

    let mut seen = std::collections::HashSet::new();
    let mut entries = Vec::new();
    let mut settled = true;
    for batch in &batches {
        if entries.len() == count {
            break;
        }
        settled &= batch.oldest.saturating_sub(BATCH_WINDOW_SECONDS) > horizon;
        let entry = batch.entry();
        if seen.insert(entry) {
            entries.push(RecentAddition {
                entry,
                added_at: batch.newest,
            });
        }
    }
    // Fewer entries than asked means more additions could add some, unless none are left.
    settled &= entries.len() == count || horizon == i64::MIN;
    (entries, settled)
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    const HOUR: i64 = 3600;

    fn added(episode: i64, season: i64, show: i64, added_at: i64) -> EpisodeAddition {
        EpisodeAddition {
            episode: RatingKey::new(episode),
            season: RatingKey::new(season),
            show: RatingKey::new(show),
            added_at,
        }
    }

    fn entries(additions: &[EpisodeAddition]) -> Vec<RecentEntry> {
        group(additions, 50, i64::MIN)
            .0
            .into_iter()
            .map(|addition| addition.entry)
            .collect()
    }

    #[test]
    fn a_batch_is_dated_by_its_newest_episode() {
        let additions = [
            added(2, 10, 100, 1001 * HOUR),
            added(1, 10, 100, 1000 * HOUR),
        ];
        assert_eq!(group(&additions, 1, i64::MIN).0[0].added_at, 1001 * HOUR);
    }

    #[test]
    fn a_lone_episode_stands_for_itself() {
        assert_eq!(
            entries(&[added(1, 10, 100, 1000 * HOUR)]),
            [RecentEntry::Episode(RatingKey::new(1))]
        );
    }

    #[test]
    fn episodes_of_one_season_added_together_stand_for_the_season() {
        // Lupin, season 3: eight episodes within a second.
        let additions = [
            added(2, 10, 100, 1000 * HOUR),
            added(1, 10, 100, 1000 * HOUR),
        ];
        assert_eq!(
            entries(&additions),
            [RecentEntry::Season(RatingKey::new(10))]
        );
    }

    #[test]
    fn a_batch_across_seasons_stands_for_the_series() {
        let additions = [
            added(2, 11, 100, 1000 * HOUR),
            added(1, 10, 100, 1000 * HOUR),
        ];
        assert_eq!(
            entries(&additions),
            [RecentEntry::Show(RatingKey::new(100))]
        );
    }

    #[test]
    fn a_daily_series_lists_each_episode_on_its_own() {
        // Plus belle la vie: one episode a day, about a day apart.
        let additions = [
            added(3, 10, 100, 1048 * HOUR),
            added(2, 10, 100, 1024 * HOUR),
            added(1, 10, 100, 1000 * HOUR),
        ];
        assert_eq!(
            entries(&additions),
            [
                RecentEntry::Episode(RatingKey::new(3)),
                RecentEntry::Episode(RatingKey::new(2)),
                RecentEntry::Episode(RatingKey::new(1)),
            ]
        );
    }

    #[test]
    fn the_window_chains_from_the_oldest_episode_of_the_batch() {
        // 34 hours between the outer two, but each within 20 hours of the next.
        let additions = [
            added(3, 10, 100, 1034 * HOUR),
            added(2, 10, 100, 1017 * HOUR),
            added(1, 10, 100, 1000 * HOUR),
        ];
        assert_eq!(
            entries(&additions),
            [RecentEntry::Season(RatingKey::new(10))]
        );
    }

    #[test]
    fn an_item_appears_once_at_its_newest_batch() {
        // Two batches of the same season, days apart: the season is listed once.
        let additions = [
            added(4, 10, 100, 1100 * HOUR),
            added(3, 10, 100, 1100 * HOUR),
            added(2, 10, 100, 1000 * HOUR),
            added(1, 10, 100, 1000 * HOUR),
        ];
        assert_eq!(
            entries(&additions),
            [RecentEntry::Season(RatingKey::new(10))]
        );
    }

    #[test]
    fn other_series_interleave_without_breaking_a_batch() {
        let additions = [
            added(2, 10, 100, 1001 * HOUR),
            added(9, 90, 900, 1000 * HOUR + 1800),
            added(1, 10, 100, 1000 * HOUR),
        ];
        assert_eq!(
            entries(&additions),
            [
                RecentEntry::Season(RatingKey::new(10)),
                RecentEntry::Episode(RatingKey::new(9)),
            ]
        );
    }

    #[test]
    fn an_entry_is_not_settled_while_an_unread_episode_could_join_it() {
        let additions = [added(1, 10, 100, 1000 * HOUR)];
        // The oldest read is the batch itself: an older one within 20 hours may follow.
        assert!(!group(&additions, 1, 1000 * HOUR).1);
        // Nothing left to read.
        assert!(group(&additions, 1, i64::MIN).1);
    }
}
