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

use crate::ids::{RatingKey, UserId};
use crate::metadata::{ItemKind, MetadataError, MetadataItem, MetadataRepository};
use crate::{Principal, Scope};

/// A position kept, at the least. Shorter ones are taken for a false start.
pub const MINIMUM_PROGRESS_MS: i64 = 60_000;

/// The share of the runtime past which a playback counts as a viewing, in percent.
pub const WATCHED_PERCENT: i64 = 90;

/// How long a playback is remembered without a report: as long as a playback session, so a
/// player that pauses for a coffee and resumes is still the same playback.
pub const VIEWING_LIFETIME_SECONDS: i64 = crate::playback::SESSION_LIFETIME_SECONDS;

/// Whose state it is. The operator watches as itself, though it is nobody's user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Viewer {
    Operator,
    User(UserId),
}

impl Viewer {
    /// Whose state a caller reads and writes: a user its own, the operator its own, and
    /// anyone else none. A caller with no user is the operator only when it is an
    /// administrator: a visitor at a published address has no user either, and must not be
    /// handed the operator's history.
    pub fn of(principal: &Principal) -> Option<Self> {
        match (principal.user, principal.scope) {
            (Some(user), _) => Some(Self::User(user)),
            (None, Scope::Admin) => Some(Self::Operator),
            (None, _) => None,
        }
    }
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
        viewer: Viewer,
        guids: &[String],
    ) -> Result<Vec<(String, WatchState)>, WatchError>;

    /// Writes these states, an empty one deleting the row.
    async fn save(&self, viewer: Viewer, states: &[(String, WatchState)])
        -> Result<(), WatchError>;

    /// How many episodes below each series or season a viewer has watched to the end.
    async fn watched_leaves(
        &self,
        viewer: Viewer,
        parents: &[RatingKey],
    ) -> Result<Vec<(RatingKey, u64)>, WatchError>;

    /// A playback still remembered: reported within [`VIEWING_LIFETIME_SECONDS`].
    async fn viewing(
        &self,
        viewer: Viewer,
        playback: &str,
        now: i64,
    ) -> Result<Option<Viewing>, WatchError>;

    /// Remembers a playback for [`VIEWING_LIFETIME_SECONDS`] from `now`, forgetting any of
    /// this viewer's that expired.
    async fn remember(
        &self,
        viewer: Viewer,
        playback: &str,
        viewing: Viewing,
        now: i64,
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
    pub viewer: Viewer,
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
                    self.watched(item, now).await?;
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
    /// reference: their own count is of episodes watched one by one.
    ///
    /// # Errors
    ///
    /// Fails when storage does.
    pub async fn mark_watched(&self, item: &MetadataItem, now: i64) -> Result<(), WatchError> {
        if is_leaf(item.kind) {
            return self.watched(item, now).await;
        }
        let leaves: Vec<String> = self
            .metadata
            .guids_below(item.id)
            .await?
            .into_iter()
            .filter(|(kind, _)| is_leaf(*kind))
            .map(|(_, guid)| guid)
            .collect();
        let mut states = self.current(&leaves).await?;
        for (_, state) in &mut states {
            *state = finished(*state, now);
        }
        self.watch.save(self.viewer, &states).await
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
                    .guids_below(item.id)
                    .await?
                    .into_iter()
                    .map(|(_, guid)| (guid, WatchState::default())),
            );
        }
        self.watch.save(self.viewer, &forgotten).await
    }

    /// One more viewing of a film or an episode, counted on its season and series too.
    async fn watched(&self, item: &MetadataItem, now: i64) -> Result<(), WatchError> {
        let mut states = self.current(&self.lineage(item).await?).await?;
        for (_, state) in &mut states {
            *state = finished(*state, now);
        }
        self.watch.save(self.viewer, &states).await
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

    use crate::ids::SectionId;
    use crate::testing::{InMemoryMetadata, InMemoryWatch};

    const SOUL_MS: i64 = 6_031_567;
    const NOW: i64 = 1_794_318_000;

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
                viewer: Viewer::Operator,
            }
        }

        async fn state(&self, id: i64) -> WatchState {
            let guid = format!("tv.plex.agents.none://{id}");
            self.watch
                .states(Viewer::Operator, &[guid])
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

    #[test]
    fn only_an_administrator_without_a_user_watches_as_the_operator() {
        assert_eq!(
            Viewer::of(&Principal::operator("owner")),
            Some(Viewer::Operator)
        );
        assert_eq!(
            Viewer::of(&Principal::visitor_of("server".to_owned())),
            None
        );
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
