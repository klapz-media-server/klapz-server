//! What each viewer has watched, and the rules that move it.
//!
//! The state belongs to the work, not to a copy of it: it is keyed by the item's `guid`, so
//! the same film in a French, an original-language and a 4K library is watched once for all
//! three, as the reference has it (four items in four libraries, one count and one date).
//!
//! The rules were measured on the reference, on a library of its own: a position under a
//! minute is not kept, one under 90% of the runtime is, and crossing 90% within one playback
//! counts a viewing and forgets the position. That crossing has to be seen: a playback whose
//! first report is already past 90% counts nothing, so a player resuming near the end does
//! not count a viewing it did not make.

use std::collections::HashMap;

use crate::history::{HistoryEntry, HistoryQuery, NewViewing, ViewingKind};
use crate::ids::{HistoryId, PartId, RatingKey, SectionId, StreamId, UserId};
use crate::metadata::{ItemKind, MetadataError, MetadataItem, MetadataRepository};
use crate::page::{Page, Window};
use crate::tracks::TrackChoice;
use crate::user_rating::RatingRecord;

/// A position kept, at the least. Shorter ones are taken for a false start.
pub const MINIMUM_PROGRESS_MS: i64 = 60_000;

/// The share of the runtime past which a playback counts as a viewing, in percent.
pub const WATCHED_PERCENT: i64 = 90;

/// How long a playback is remembered without a report: as long as a playback session, so a
/// player that pauses for a coffee and resumes is still the same playback.
pub const VIEWING_LIFETIME_SECONDS: i64 = crate::playback::SESSION_LIFETIME_SECONDS;

/// How far back Continue Watching looks: the reference's `OnDeckWindow`, 16 weeks. What was
/// last played longer ago is left out.
pub const DECK_WINDOW_SECONDS: i64 = 16 * 7 * 24 * 60 * 60;

/// The most recent states a Continue Watching list is built from: more than any viewer
/// plays in sixteen weeks, and a ceiling should one try.
const DECK_SOURCES: u32 = 1_000;

/// Why an item is in Continue Watching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeckReason {
    /// Started and not finished: a film, or an episode.
    InProgress,
    /// The episode after the one last watched.
    UpNext,
}

/// One item of Continue Watching.
#[derive(Debug, Clone)]
pub struct DeckEntry {
    pub item: MetadataItem,
    pub reason: DeckReason,
    /// When the work behind it was last played: the film's, or the series' for an episode,
    /// which is what the list is ordered by.
    pub last_viewed_at: i64,
}

/// What a viewer has watched of one work.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WatchState {
    /// How many times it was watched to the end.
    pub view_count: u32,
    /// Where an unfinished viewing stopped, in milliseconds.
    pub offset_ms: Option<i64>,
    /// When it was last watched or played, in seconds since the Unix epoch.
    pub last_viewed_at: Option<i64>,
}

impl WatchState {
    /// Nothing to keep: the state of a work nobody has touched.
    pub const fn is_empty(&self) -> bool {
        self.view_count == 0 && self.offset_ms.is_none() && self.last_viewed_at.is_none()
    }
}

/// One playback in progress, as far as its reports go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Viewing {
    pub item: RatingKey,
    /// The last position the player reported, kept whatever was recorded.
    pub position_ms: i64,
    /// Whether this playback has already counted its viewing.
    pub completed: bool,
}

/// What a report did to the state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportOutcome {
    /// Nothing changed: no runtime to compare with, or a playback that opened past the end.
    Ignored,
    /// Nothing kept yet: the playback is under way but short of a minute.
    Early,
    /// The position was kept.
    Progress,
    /// The playback counted as a viewing, now or earlier.
    Completed,
}

#[derive(Debug, thiserror::Error)]
pub enum WatchError {
    #[error("watch state storage failed")]
    Storage(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error(transparent)]
    Metadata(#[from] MetadataError),
}

#[async_trait::async_trait]
pub trait WatchRepository: Send + Sync + std::fmt::Debug {
    /// The states of these works for one viewer. A work never touched is absent.
    async fn states(
        &self,
        viewer: UserId,
        guids: &[String],
    ) -> Result<Vec<(String, WatchState)>, WatchError>;

    /// Writes these states, an empty one deleting the row.
    async fn save(&self, viewer: UserId, states: &[(String, WatchState)])
        -> Result<(), WatchError>;

    /// Writes these states and adds these viewings to the viewer's history, at once. The one
    /// way an entry is written: a viewing in the history is one the state counted.
    async fn record_viewings(
        &self,
        viewer: UserId,
        states: &[(String, WatchState)],
        viewings: &[NewViewing],
    ) -> Result<(), WatchError>;

    /// A window of the history a query reads. An entry whose item is gone is not read.
    async fn history(
        &self,
        query: &HistoryQuery,
        window: Window,
    ) -> Result<Page<HistoryEntry>, WatchError>;

    /// How many entries a query reads in all.
    async fn history_count(&self, query: &HistoryQuery) -> Result<u64, WatchError>;

    /// One entry of anyone's history.
    async fn history_entry(&self, id: HistoryId) -> Result<Option<HistoryEntry>, WatchError>;

    /// Deletes one entry, leaving the watch state as it is. Whether there was one.
    async fn delete_history_entry(&self, id: HistoryId) -> Result<bool, WatchError>;

    /// How many episodes below each series or season a viewer has watched to the end.
    async fn watched_leaves(
        &self,
        viewer: UserId,
        parents: &[RatingKey],
    ) -> Result<Vec<(RatingKey, u64)>, WatchError>;

    /// The states last viewed at `since` or later, newest first, at most `limit`.
    async fn recent(
        &self,
        viewer: UserId,
        since: i64,
        limit: u32,
    ) -> Result<Vec<(String, WatchState)>, WatchError>;

    /// A playback still remembered: reported within [`VIEWING_LIFETIME_SECONDS`].
    async fn viewing(
        &self,
        viewer: UserId,
        playback: &str,
        now: i64,
    ) -> Result<Option<Viewing>, WatchError>;

    /// Remembers a playback for [`VIEWING_LIFETIME_SECONDS`] from `now`, forgetting any of
    /// this viewer's that expired.
    async fn remember(
        &self,
        viewer: UserId,
        playback: &str,
        viewing: Viewing,
        now: i64,
    ) -> Result<(), WatchError>;

    /// The tracks a viewer chose on these files. A file it chose nothing on is absent.
    async fn track_choices(
        &self,
        viewer: UserId,
        files: &[PartId],
    ) -> Result<Vec<(PartId, TrackChoice)>, WatchError>;

    /// Records a viewer's choice on one file, an empty one deleting it.
    async fn choose_tracks(
        &self,
        viewer: UserId,
        file: PartId,
        choice: TrackChoice,
    ) -> Result<(), WatchError>;

    /// The offsets a viewer shifts these subtitles by, in milliseconds. A subtitle it never
    /// shifted is absent.
    async fn subtitle_offsets(
        &self,
        viewer: UserId,
        tracks: &[StreamId],
    ) -> Result<Vec<(StreamId, i32)>, WatchError>;

    /// Records the offset a viewer shifts a subtitle by, 0 deleting it.
    async fn set_subtitle_offset(
        &self,
        viewer: UserId,
        track: StreamId,
        offset_ms: i32,
    ) -> Result<(), WatchError>;

    /// The last change a viewer made to its rating of these works, a clearing included. A
    /// work never rated is absent.
    async fn ratings(
        &self,
        viewer: UserId,
        guids: &[String],
    ) -> Result<Vec<(String, RatingRecord)>, WatchError>;

    /// Records a change to a viewer's rating of a work, replacing the one held.
    async fn save_rating(
        &self,
        viewer: UserId,
        guid: &str,
        record: RatingRecord,
    ) -> Result<(), WatchError>;
}

/// Whether a position is past the share of the runtime that counts a viewing.
///
/// Compared in whole seconds, as the reference compares: 5,428,410 ms of a 6,031,567 ms film
/// counts, though it is a hair short of 90% in milliseconds, because 5,428 s is 90% of
/// 6,031 s.
const fn past_the_end(position_ms: i64, duration_ms: i64) -> bool {
    (position_ms / 1000).saturating_mul(100) >= (duration_ms / 1000).saturating_mul(WATCHED_PERCENT)
}

/// One viewer's watch state, with the library it is about.
#[derive(Debug, Clone, Copy)]
pub struct Watching<'a> {
    pub metadata: &'a dyn MetadataRepository,
    pub watch: &'a dyn WatchRepository,
    pub viewer: UserId,
}

impl Watching<'_> {
    /// Records a player's report on one playback.
    ///
    /// `duration_ms` is the player's: the reference compares with the runtime the player
    /// states, and keeps nothing when it states none.
    ///
    /// # Errors
    ///
    /// Fails when storage does.
    pub async fn report(
        &self,
        item: &MetadataItem,
        playback: &str,
        position_ms: i64,
        duration_ms: Option<i64>,
        now: i64,
    ) -> Result<ReportOutcome, WatchError> {
        let previous = self
            .watch
            .viewing(self.viewer, playback, now)
            .await?
            .filter(|viewing| viewing.item == item.id);
        let Some(duration) = duration_ms.filter(|duration| *duration > 0) else {
            return Ok(ReportOutcome::Ignored);
        };

        let ended = past_the_end(position_ms, duration);
        let outcome = if ended {
            match previous {
                Some(viewing) if viewing.completed => ReportOutcome::Completed,
                // The crossing is seen: the last report of this playback fell short of the end.
                Some(viewing) if !past_the_end(viewing.position_ms, duration) => {
                    self.watched(item, ViewingKind::Played, now).await?;
                    ReportOutcome::Completed
                }
                _ => ReportOutcome::Ignored,
            }
        } else if position_ms >= MINIMUM_PROGRESS_MS {
            self.progressed(item, position_ms, now).await?;
            ReportOutcome::Progress
        } else {
            ReportOutcome::Early
        };

        let viewing = Viewing {
            item: item.id,
            position_ms,
            completed: outcome == ReportOutcome::Completed,
        };
        self.watch
            .remember(self.viewer, playback, viewing, now)
            .await?;
        Ok(outcome)
    }

    /// Keeps a position without asking whether the work is finished, as an explicit
    /// "remember where I stopped" does: the reference keeps 95% that way. Whether it was
    /// kept.
    ///
    /// # Errors
    ///
    /// Fails when storage does.
    pub async fn keep_position(
        &self,
        item: &MetadataItem,
        position_ms: i64,
        now: i64,
    ) -> Result<bool, WatchError> {
        if position_ms < MINIMUM_PROGRESS_MS {
            return Ok(false);
        }
        self.progressed(item, position_ms, now).await?;
        Ok(true)
    }

    /// Marks an item watched: a film or an episode once more, a season or a series by each
    /// of its episodes once more. The containers themselves are left as they are, as on the
    /// reference: their own count is of episodes watched one by one. Each film or episode
    /// marked is an entry in the history.
    ///
    /// # Errors
    ///
    /// Fails when storage does.
    pub async fn mark_watched(&self, item: &MetadataItem, now: i64) -> Result<(), WatchError> {
        if is_leaf(item.kind) {
            return self.watched(item, ViewingKind::Marked, now).await;
        }
        let leaves: Vec<MetadataItem> = self
            .metadata
            .descendants(item.id)
            .await?
            .into_iter()
            .filter(|below| is_leaf(below.kind))
            .collect();
        let guids: Vec<String> = leaves.iter().map(|leaf| leaf.guid.clone()).collect();
        let mut states = self.current(&guids).await?;
        for (_, state) in &mut states {
            *state = finished(*state, now);
        }
        let viewings: Vec<NewViewing> = leaves
            .iter()
            .map(|leaf| NewViewing {
                item: leaf.id,
                kind: ViewingKind::Marked,
                viewed_at: now,
            })
            .collect();
        self.watch
            .record_viewings(self.viewer, &states, &viewings)
            .await
    }

    /// Forgets what was watched of an item: an episode alone, a season with its episodes, a
    /// series with its seasons and episodes. A season's series keeps its own count.
    ///
    /// # Errors
    ///
    /// Fails when storage does.
    pub async fn mark_unwatched(&self, item: &MetadataItem) -> Result<(), WatchError> {
        let mut forgotten = vec![(item.guid.clone(), WatchState::default())];
        if !is_leaf(item.kind) {
            forgotten.extend(
                self.metadata
                    .descendants(item.id)
                    .await?
                    .into_iter()
                    .map(|below| (below.guid, WatchState::default())),
            );
        }
        self.watch.save(self.viewer, &forgotten).await
    }

    /// What the viewer has started and what comes next, newest first: every film in
    /// progress, and for every series an episode — the one in progress, else the one after
    /// the episode last watched. Played within [`DECK_WINDOW_SECONDS`] only.
    ///
    /// Every copy of a work is listed, as the reference lists the French and the original
    /// cut of the same film side by side.
    ///
    /// # Errors
    ///
    /// Fails when storage does.
    pub async fn continue_watching(
        &self,
        sections: Option<&[SectionId]>,
        now: i64,
    ) -> Result<Vec<DeckEntry>, WatchError> {
        let recent = self
            .watch
            .recent(self.viewer, now - DECK_WINDOW_SECONDS, DECK_SOURCES)
            .await?;
        let guids: Vec<String> = recent.iter().map(|(guid, _)| guid.clone()).collect();
        let items = self.metadata.items_by_guid(&guids, sections).await?;
        let viewed = |guid: &str| {
            recent
                .iter()
                .find(|(recent, _)| recent == guid)
                .and_then(|(_, state)| state.last_viewed_at)
        };

        let mut entries = Vec::new();
        // A series is under way when it or any of its episodes was played: marking a whole
        // season dates its episodes, not the series.
        let mut series: HashMap<RatingKey, i64> = HashMap::new();
        let mut seasons_of_episodes: HashMap<RatingKey, i64> = HashMap::new();
        for item in items {
            let Some(last_viewed_at) = viewed(&item.guid) else {
                continue;
            };
            match item.kind {
                ItemKind::Movie if in_progress(&recent, &item.guid) => {
                    entries.push(DeckEntry {
                        item,
                        reason: DeckReason::InProgress,
                        last_viewed_at,
                    });
                }
                ItemKind::Show => latest(&mut series, item.id, last_viewed_at),
                ItemKind::Episode => {
                    if let Some(season) = item.parent_id {
                        latest(&mut seasons_of_episodes, season, last_viewed_at);
                    }
                }
                _ => {}
            }
        }
        let season_ids: Vec<RatingKey> = seasons_of_episodes.keys().copied().collect();
        for season in self.metadata.items_by_id(&season_ids).await? {
            if let (Some(show), Some(at)) = (season.parent_id, seasons_of_episodes.get(&season.id))
            {
                latest(&mut series, show, *at);
            }
        }
        let show_ids: Vec<RatingKey> = series.keys().copied().collect();
        for show in self.metadata.items_by_id(&show_ids).await? {
            let last_viewed_at = series.get(&show.id).copied().unwrap_or_default();
            if let Some((item, reason)) = self.up_next(&show).await? {
                entries.push(DeckEntry {
                    item,
                    reason,
                    last_viewed_at,
                });
            }
        }
        entries.sort_by(|a, b| {
            b.last_viewed_at
                .cmp(&a.last_viewed_at)
                .then(a.item.id.cmp(&b.item.id))
        });
        Ok(entries)
    }

    /// What to play once an episode ends: its series' entry in Continue Watching, unless that
    /// entry is the episode itself — one still in progress, or the next of a series whose
    /// state has not moved yet. Measured on the reference's post-play shelf, which offers the
    /// series' entry from any of its episodes, not the episode after the one played: S9E1 of
    /// a series watched up to S24E2 offers S24E3. None when the series was not played within
    /// [`DECK_WINDOW_SECONDS`], or has nothing left, as Continue Watching has nothing then.
    ///
    /// # Errors
    ///
    /// Fails when storage does.
    pub async fn after_episode(
        &self,
        episode: &MetadataItem,
        now: i64,
    ) -> Result<Option<MetadataItem>, WatchError> {
        let Some(season) = episode.parent_id else {
            return Ok(None);
        };
        let Some(show) = self.metadata.item(season).await?.parent_id else {
            return Ok(None);
        };
        let show = self.metadata.item(show).await?;
        let below = self.metadata.descendants(show.id).await?;
        let recent = self
            .watch
            .recent(self.viewer, now - DECK_WINDOW_SECONDS, DECK_SOURCES)
            .await?;
        let under_way = std::iter::once(&show)
            .chain(&below)
            .any(|item| recent.iter().any(|(guid, _)| *guid == item.guid));
        if !under_way {
            return Ok(None);
        }
        Ok(self
            .up_next(&show)
            .await?
            .map(|(next, _)| next)
            .filter(|next| next.id != episode.id))
    }

    /// The episode of a series to play next, as the reference picks it: one in progress if
    /// any, the most recently played; else the episode after the one most recently watched,
    /// the next season's first after a season's last; else, when none was watched, the
    /// first. Nothing once the series' last episode is the one most recently watched, even
    /// with earlier ones unwatched.
    ///
    /// Specials (season 0) stand outside the sequence: unmeasured, since the reference's
    /// test library has none, and a special is never what "next" means.
    async fn up_next(
        &self,
        show: &MetadataItem,
    ) -> Result<Option<(MetadataItem, DeckReason)>, WatchError> {
        let below = self.metadata.descendants(show.id).await?;
        let seasons: HashMap<RatingKey, i32> = below
            .iter()
            .filter(|item| item.kind == ItemKind::Season)
            .map(|season| (season.id, season.position.unwrap_or(0)))
            .collect();
        let season_of = |episode: &MetadataItem| {
            episode
                .parent_id
                .and_then(|season| seasons.get(&season).copied())
                .unwrap_or(0)
        };
        let mut episodes: Vec<MetadataItem> = below
            .iter()
            .filter(|item| item.kind == ItemKind::Episode && season_of(item) > 0)
            .cloned()
            .collect();
        episodes.sort_by_key(|episode| {
            (
                season_of(episode),
                episode.position.unwrap_or(0),
                episode.id,
            )
        });

        let guids: Vec<String> = episodes
            .iter()
            .map(|episode| episode.guid.clone())
            .collect();
        let states = self.current(&guids).await?;
        let started = (0..episodes.len())
            .filter(|index| states[*index].1.offset_ms.is_some())
            .max_by_key(|index| (states[*index].1.last_viewed_at, *index));
        if let Some(index) = started {
            return Ok(Some((episodes.swap_remove(index), DeckReason::InProgress)));
        }
        // Ties — a whole season marked at once — go to the latest in order.
        let last_watched = (0..episodes.len())
            .filter(|index| states[*index].1.view_count > 0)
            .max_by_key(|index| (states[*index].1.last_viewed_at, *index));
        let next = last_watched.map_or(0, |index| index + 1);
        Ok((next < episodes.len()).then(|| (episodes.swap_remove(next), DeckReason::UpNext)))
    }

    /// One more viewing of a film or an episode, counted on its season and series too, and
    /// one entry in the history.
    async fn watched(
        &self,
        item: &MetadataItem,
        kind: ViewingKind,
        now: i64,
    ) -> Result<(), WatchError> {
        let mut states = self.current(&self.lineage(item).await?).await?;
        for (_, state) in &mut states {
            *state = finished(*state, now);
        }
        let viewing = NewViewing {
            item: item.id,
            kind,
            viewed_at: now,
        };
        self.watch
            .record_viewings(self.viewer, &states, &[viewing])
            .await
    }

    /// A position kept on the item, and the date of it on its season and series.
    async fn progressed(
        &self,
        item: &MetadataItem,
        position_ms: i64,
        now: i64,
    ) -> Result<(), WatchError> {
        let mut states = self.current(&self.lineage(item).await?).await?;
        for (index, (_, state)) in states.iter_mut().enumerate() {
            state.last_viewed_at = Some(now);
            if index == 0 {
                state.offset_ms = Some(position_ms);
            }
        }
        self.watch.save(self.viewer, &states).await
    }

    /// The item's work, then its season's and its series', nearest first.
    async fn lineage(&self, item: &MetadataItem) -> Result<Vec<String>, WatchError> {
        let mut guids = vec![item.guid.clone()];
        let mut parent = item.parent_id;
        while let Some(id) = parent {
            let ancestor = self.metadata.item(id).await?;
            guids.push(ancestor.guid);
            parent = ancestor.parent_id;
        }
        Ok(guids)
    }

    /// The stored states of these works, in their order, an untouched one empty.
    async fn current(&self, guids: &[String]) -> Result<Vec<(String, WatchState)>, WatchError> {
        let stored = self.watch.states(self.viewer, guids).await?;
        Ok(guids
            .iter()
            .map(|guid| {
                let state = stored
                    .iter()
                    .find(|(stored, _)| stored == guid)
                    .map(|(_, state)| *state)
                    .unwrap_or_default();
                (guid.clone(), state)
            })
            .collect())
    }
}

/// Whether a work is started and not finished.
fn in_progress(recent: &[(String, WatchState)], guid: &str) -> bool {
    recent
        .iter()
        .any(|(recent, state)| recent == guid && state.offset_ms.is_some())
}

/// Keeps the latest of two dates under a key.
fn latest(dates: &mut HashMap<RatingKey, i64>, key: RatingKey, at: i64) {
    let kept = dates.entry(key).or_insert(at);
    *kept = (*kept).max(at);
}

const fn is_leaf(kind: ItemKind) -> bool {
    matches!(kind, ItemKind::Movie | ItemKind::Episode)
}

const fn finished(state: WatchState, now: i64) -> WatchState {
    WatchState {
        view_count: state.view_count.saturating_add(1),
        offset_ms: None,
        last_viewed_at: Some(now),
    }
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    #[test]
    fn the_end_starts_at_ninety_percent_inclusive() {
        // Measured: 89.9% keeps a position, 90% of the runtime counts a viewing.
        assert!(!past_the_end(5_422_378, 6_031_567));
        assert!(past_the_end(5_428_410, 6_031_567));
    }

    use crate::history::HistoryScope;
    use crate::ids::SectionId;
    use crate::testing::{InMemoryMetadata, InMemoryWatch};

    const SOUL_MS: i64 = 6_031_567;
    const NOW: i64 = 1_794_318_000;
    const VIEWER: UserId = UserId::new(1);

    fn item(id: i64, kind: ItemKind, parent: Option<i64>) -> MetadataItem {
        MetadataItem {
            id: RatingKey::new(id),
            section_id: SectionId::new(27),
            parent_id: parent.map(RatingKey::new),
            kind,
            guid: format!("tv.plex.agents.none://{id}"),
            title: None,
            title_sort: None,
            original_title: None,
            summary: None,
            tagline: None,
            studio: None,
            content_rating: None,
            content_rating_age: None,
            position: Some(1),
            year: None,
            released_on: None,
            duration_ms: None,
            rating: None,
            audience_rating: None,
            added_at: 0,
            updated_at: 0,
        }
    }

    /// The reference's test library: Soul, and Bull with one season of two episodes.
    fn library() -> Vec<MetadataItem> {
        vec![
            item(577_986, ItemKind::Movie, None),
            item(577_987, ItemKind::Show, None),
            item(577_988, ItemKind::Season, Some(577_987)),
            item(577_989, ItemKind::Episode, Some(577_988)),
            item(577_990, ItemKind::Episode, Some(577_988)),
        ]
    }

    struct Fixture {
        metadata: InMemoryMetadata,
        watch: InMemoryWatch,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                metadata: InMemoryMetadata::new(library()),
                watch: InMemoryWatch::over(library()),
            }
        }

        fn watching(&self) -> Watching<'_> {
            Watching {
                metadata: &self.metadata,
                watch: &self.watch,
                viewer: VIEWER,
            }
        }

        async fn state(&self, id: i64) -> WatchState {
            let guid = format!("tv.plex.agents.none://{id}");
            self.watch
                .states(VIEWER, &[guid])
                .await
                .unwrap()
                .first()
                .map(|(_, state)| *state)
                .unwrap_or_default()
        }

        async fn report(&self, playback: &str, position_ms: i64) -> ReportOutcome {
            let soul = library().remove(0);
            self.watching()
                .report(&soul, playback, position_ms, Some(SOUL_MS), NOW)
                .await
                .unwrap()
        }
    }

    fn percent(share: i64) -> i64 {
        SOUL_MS * share / 100
    }

    #[tokio::test]
    async fn keeps_a_position_past_a_minute_and_short_of_the_end() {
        let fixture = Fixture::new();
        assert_eq!(fixture.report("a", 30_000).await, ReportOutcome::Early);
        assert!(fixture.state(577_986).await.is_empty());
        assert_eq!(fixture.report("a", 120_000).await, ReportOutcome::Progress);
        let state = fixture.state(577_986).await;
        assert_eq!(state.offset_ms, Some(120_000));
        assert_eq!(state.last_viewed_at, Some(NOW));
        assert_eq!(state.view_count, 0);
    }

    #[tokio::test]
    async fn crossing_the_end_counts_once_and_forgets_the_position() {
        let fixture = Fixture::new();
        fixture.report("a", percent(89)).await;
        assert_eq!(
            fixture.report("a", percent(91)).await,
            ReportOutcome::Completed
        );
        // Still complete, not counted again.
        assert_eq!(
            fixture.report("a", percent(95)).await,
            ReportOutcome::Completed
        );
        let state = fixture.state(577_986).await;
        assert_eq!((state.view_count, state.offset_ms), (1, None));
    }

    #[tokio::test]
    async fn a_playback_that_opens_past_the_end_counts_nothing() {
        // Measured: a first report at 95% records nothing, and a new playback past the end
        // does not count off another's position.
        let fixture = Fixture::new();
        assert_eq!(
            fixture.report("a", percent(95)).await,
            ReportOutcome::Ignored
        );
        assert!(fixture.state(577_986).await.is_empty());
        fixture.report("b", percent(50)).await;
        assert_eq!(
            fixture.report("c", percent(95)).await,
            ReportOutcome::Ignored
        );
        assert_eq!(fixture.state(577_986).await.view_count, 0);
        // Even a report too early to keep is a crossing seen.
        fixture.report("d", 30_000).await;
        assert_eq!(
            fixture.report("d", percent(95)).await,
            ReportOutcome::Completed
        );
    }

    #[tokio::test]
    async fn a_report_without_a_runtime_keeps_nothing() {
        let fixture = Fixture::new();
        let soul = library().remove(0);
        let outcome = fixture
            .watching()
            .report(&soul, "a", 120_000, None, NOW)
            .await
            .unwrap();
        assert_eq!(outcome, ReportOutcome::Ignored);
        assert!(fixture.state(577_986).await.is_empty());
    }

    #[tokio::test]
    async fn an_explicit_position_skips_the_end_but_not_the_minute() {
        let fixture = Fixture::new();
        let soul = library().remove(0);
        let watching = fixture.watching();
        assert!(watching
            .keep_position(&soul, percent(95), NOW)
            .await
            .unwrap());
        assert_eq!(fixture.state(577_986).await.offset_ms, Some(percent(95)));
        assert!(!watching.keep_position(&soul, 30_000, NOW).await.unwrap());
        assert_eq!(fixture.state(577_986).await.offset_ms, Some(percent(95)));
    }

    #[tokio::test]
    async fn an_episode_watched_counts_on_its_season_and_series() {
        let fixture = Fixture::new();
        let items = library();
        let watching = fixture.watching();
        watching.mark_watched(&items[4], NOW).await.unwrap();
        watching.mark_watched(&items[4], NOW).await.unwrap();
        watching.mark_watched(&items[3], NOW).await.unwrap();
        // As measured: episode 2 twice, episode 1 once, three on the season and the series.
        assert_eq!(fixture.state(577_990).await.view_count, 2);
        assert_eq!(fixture.state(577_989).await.view_count, 1);
        assert_eq!(fixture.state(577_988).await.view_count, 3);
        assert_eq!(fixture.state(577_987).await.view_count, 3);

        // Marking the season marks its episodes, not itself.
        watching.mark_watched(&items[2], NOW).await.unwrap();
        assert_eq!(fixture.state(577_990).await.view_count, 3);
        assert_eq!(fixture.state(577_988).await.view_count, 3);
    }

    #[tokio::test]
    async fn forgetting_reaches_down_and_never_up() {
        let fixture = Fixture::new();
        let items = library();
        let watching = fixture.watching();
        watching.mark_watched(&items[3], NOW).await.unwrap();
        watching.mark_watched(&items[4], NOW).await.unwrap();

        watching.mark_unwatched(&items[3]).await.unwrap();
        assert!(fixture.state(577_989).await.is_empty());
        assert_eq!(fixture.state(577_988).await.view_count, 2);

        watching.mark_unwatched(&items[2]).await.unwrap();
        assert!(fixture.state(577_990).await.is_empty());
        assert!(fixture.state(577_988).await.is_empty());
        // The series keeps its own count until it is forgotten itself.
        assert_eq!(fixture.state(577_987).await.view_count, 2);
        watching.mark_unwatched(&items[1]).await.unwrap();
        assert!(fixture.state(577_987).await.is_empty());
    }

    async fn history(watch: &InMemoryWatch) -> Vec<(i64, ViewingKind)> {
        let query = HistoryQuery::of(HistoryScope::Viewer(VIEWER));
        let window = Window::by_id(None, None, Some(50), None).unwrap();
        watch
            .history(&query, window)
            .await
            .unwrap()
            .items
            .into_iter()
            .map(|entry| (entry.item.get(), entry.kind))
            .collect()
    }

    #[tokio::test]
    async fn only_a_viewing_counted_is_an_entry_in_the_history() {
        // Measured: crossing the end writes one entry, later reports of the same playback
        // none, and a playback stopped short or a position kept none either.
        let fixture = Fixture::new();
        fixture.report("a", percent(30)).await;
        fixture.report("a", percent(50)).await;
        let soul = library().remove(0);
        fixture
            .watching()
            .keep_position(&soul, percent(60), NOW)
            .await
            .unwrap();
        assert!(history(&fixture.watch).await.is_empty());
        fixture.report("b", percent(50)).await;
        fixture.report("b", percent(91)).await;
        fixture.report("b", percent(95)).await;
        assert_eq!(
            history(&fixture.watch).await,
            vec![(577_986, ViewingKind::Played)]
        );
    }

    #[tokio::test]
    async fn marking_a_season_writes_one_entry_per_episode_and_forgetting_none() {
        let fixture = Fixture::new();
        let items = library();
        let watching = fixture.watching();
        watching.mark_watched(&items[0], NOW).await.unwrap();
        watching.mark_watched(&items[2], NOW).await.unwrap();
        let marked = vec![
            (577_986, ViewingKind::Marked),
            (577_989, ViewingKind::Marked),
            (577_990, ViewingKind::Marked),
        ];
        assert_eq!(history(&fixture.watch).await, marked);
        // Measured: `/:/unscrobble` leaves the history as it is.
        watching.mark_unwatched(&items[1]).await.unwrap();
        watching.mark_unwatched(&items[0]).await.unwrap();
        assert_eq!(history(&fixture.watch).await, marked);
    }

    #[tokio::test]
    async fn an_episode_played_dates_its_season_and_series() {
        let fixture = Fixture::new();
        let items = library();
        fixture
            .watching()
            .report(&items[3], "a", 120_000, Some(2_467_160), NOW)
            .await
            .unwrap();
        assert_eq!(fixture.state(577_989).await.offset_ms, Some(120_000));
        let season = fixture.state(577_988).await;
        assert_eq!((season.last_viewed_at, season.offset_ms), (Some(NOW), None));
        assert_eq!(fixture.state(577_987).await.last_viewed_at, Some(NOW));
    }

    /// The reference's test series once given five episodes: season 5 of four, season 6 of
    /// one, and Soul beside it.
    fn series() -> Vec<MetadataItem> {
        let mut season_5 = item(577_988, ItemKind::Season, Some(577_987));
        season_5.position = Some(5);
        let mut season_6 = item(578_171, ItemKind::Season, Some(577_987));
        season_6.position = Some(6);
        let episode = |id: i64, season: i64, number: i32| {
            let mut episode = item(id, ItemKind::Episode, Some(season));
            episode.position = Some(number);
            episode
        };
        vec![
            item(577_986, ItemKind::Movie, None),
            item(577_987, ItemKind::Show, None),
            season_5,
            season_6,
            episode(578_173, 577_988, 1),
            episode(577_990, 577_988, 2),
            episode(578_174, 577_988, 3),
            episode(578_175, 577_988, 4),
            episode(578_172, 578_171, 1),
        ]
    }

    struct Deck {
        metadata: InMemoryMetadata,
        watch: InMemoryWatch,
    }

    impl Deck {
        fn new() -> Self {
            Self {
                metadata: InMemoryMetadata::new(series()),
                watch: InMemoryWatch::over(series()),
            }
        }

        fn watching(&self) -> Watching<'_> {
            Watching {
                metadata: &self.metadata,
                watch: &self.watch,
                viewer: VIEWER,
            }
        }

        fn item(id: i64) -> MetadataItem {
            series()
                .into_iter()
                .find(|item| item.id.get() == id)
                .unwrap()
        }

        /// Watches the episodes in this order, a second apart.
        async fn watch(&self, ids: &[i64]) {
            for (second, id) in (0_i64..).zip(ids) {
                self.watching()
                    .mark_watched(&Self::item(*id), NOW + second)
                    .await
                    .unwrap();
            }
        }

        async fn start(&self, id: i64, at: i64) {
            self.watching()
                .keep_position(&Self::item(id), 300_000, at)
                .await
                .unwrap();
        }

        async fn next(&self) -> Vec<(i64, DeckReason)> {
            self.watching()
                .continue_watching(None, NOW + 100)
                .await
                .unwrap()
                .into_iter()
                .map(|entry| (entry.item.id.get(), entry.reason))
                .collect()
        }
    }

    const S5E1: i64 = 578_173;
    const S5E2: i64 = 577_990;
    const S5E3: i64 = 578_174;
    const S5E4: i64 = 578_175;
    const S6E1: i64 = 578_172;

    #[tokio::test]
    async fn the_next_episode_follows_the_one_most_recently_watched() {
        // Each case as measured on the reference.
        for (watched, expected) in [
            (vec![S5E1], Some(S5E2)),
            (vec![S5E1, S5E2], Some(S5E3)),
            (vec![S5E3], Some(S5E4)),
            (vec![S5E3, S5E1], Some(S5E2)),
            (vec![S5E1, S5E3], Some(S5E4)),
            // A season's last leads to the next season's first.
            (vec![S5E1, S5E2, S5E3, S5E4], Some(S6E1)),
            (vec![S5E4], Some(S6E1)),
            // The series' last watched most recently: nothing, earlier ones unwatched or not.
            (vec![S6E1], None),
            (vec![S5E1, S5E2, S5E3, S5E4, S6E1], None),
        ] {
            let deck = Deck::new();
            deck.watch(&watched).await;
            let expected: Vec<(i64, DeckReason)> = expected
                .map(|id| (id, DeckReason::UpNext))
                .into_iter()
                .collect();
            assert_eq!(deck.next().await, expected, "after {watched:?}");
        }
    }

    #[tokio::test]
    async fn an_episode_in_progress_comes_first() {
        let deck = Deck::new();
        deck.start(S5E3, NOW).await;
        deck.watch(&[S5E1]).await;
        // Watched after the other was started, and still the started one.
        assert_eq!(deck.next().await, vec![(S5E3, DeckReason::InProgress)]);
    }

    #[tokio::test]
    async fn a_season_marked_at_once_leads_to_the_next() {
        let deck = Deck::new();
        deck.watching()
            .mark_watched(&Deck::item(577_988), NOW)
            .await
            .unwrap();
        assert_eq!(deck.next().await, vec![(S6E1, DeckReason::UpNext)]);
    }

    #[tokio::test]
    async fn a_film_is_listed_while_started_and_newest_first() {
        let deck = Deck::new();
        deck.start(577_986, NOW + 50).await;
        deck.watch(&[S5E1]).await;
        assert_eq!(
            deck.next().await,
            vec![
                (577_986, DeckReason::InProgress),
                (S5E2, DeckReason::UpNext)
            ]
        );
        // Watched to the end, it leaves.
        deck.watching()
            .mark_watched(&Deck::item(577_986), NOW + 60)
            .await
            .unwrap();
        assert_eq!(deck.next().await, vec![(S5E2, DeckReason::UpNext)]);
    }

    #[tokio::test]
    async fn what_was_last_played_sixteen_weeks_ago_is_left_out() {
        let deck = Deck::new();
        deck.start(577_986, NOW - DECK_WINDOW_SECONDS).await;
        assert!(deck.next().await.is_empty());
    }

    async fn after(deck: &Deck, id: i64) -> Option<i64> {
        deck.watching()
            .after_episode(&Deck::item(id), NOW + 100)
            .await
            .unwrap()
            .map(|next| next.id.get())
    }

    #[tokio::test]
    async fn after_an_episode_comes_its_series_entry_wherever_it_stands() {
        // Measured on the reference's test series: with S5E1 in progress, S5E2 offers S5E1
        // and S5E1 offers nothing; once S5E1 is watched, S5E1 offers S5E2.
        let deck = Deck::new();
        deck.start(S5E1, NOW).await;
        assert_eq!(after(&deck, S5E2).await, Some(S5E1));
        assert_eq!(after(&deck, S5E1).await, None);

        deck.watch(&[S5E1]).await;
        assert_eq!(after(&deck, S5E1).await, Some(S5E2));
        // Not the episode after the one played: the series' entry.
        assert_eq!(after(&deck, S5E4).await, Some(S5E2));
    }

    #[tokio::test]
    async fn after_a_finished_or_untouched_series_comes_nothing() {
        let deck = Deck::new();
        assert_eq!(after(&deck, S5E1).await, None);
        deck.watch(&[S6E1]).await;
        assert_eq!(after(&deck, S5E1).await, None);
    }

    #[tokio::test]
    async fn after_a_series_played_long_ago_comes_nothing() {
        let deck = Deck::new();
        deck.watching()
            .mark_watched(&Deck::item(S5E1), NOW - DECK_WINDOW_SECONDS)
            .await
            .unwrap();
        assert_eq!(after(&deck, S5E1).await, None);
    }

    #[test]
    fn an_untouched_state_is_empty() {
        assert!(WatchState::default().is_empty());
        assert!(!WatchState {
            last_viewed_at: Some(1),
            ..WatchState::default()
        }
        .is_empty());
    }
}
