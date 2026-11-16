//! A viewer's own rating of a work, out of ten.
//!
//! Kept per viewer and per `guid`, as watch state is: the reference stores it beside the watch
//! state, in one row per account and work, and of the works its owner rated, ten are held by
//! several libraries and rated once for all of them.
//!
//! The rules were measured on the reference's test libraries. Every kind can be rated — a film,
//! a series, a season, an episode — and none adds up its children's: a series rated 4 over
//! episodes rated 8 says 4, and one not rated says nothing. A change carries the time it was
//! made, now unless the client says otherwise, and one made before the change it would replace
//! is ignored: a rating dated before the one held, or before the rating was cleared, leaves it
//! as it is. Clearing a rating that is not there does nothing at all.

use crate::ids::UserId;
use crate::watch::{WatchError, WatchRepository};

/// The top of the scale. Plex's stars are halves of it: three and a half stars is 7.
pub const MAX_RATING: f64 = 10.0;

/// A rating, from 0 to 10, at the precision it was given.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct UserRating(f64);

impl UserRating {
    /// # Errors
    ///
    /// Fails for a value off the scale, which includes every value that is not a number.
    pub fn new(value: f64) -> Result<Self, RatingError> {
        if (0.0..=MAX_RATING).contains(&value) {
            Ok(Self(value))
        } else {
            Err(RatingError::OffTheScale)
        }
    }

    pub const fn get(self) -> f64 {
        self.0
    }
}

/// The last change a viewer made to its rating of a work: a rating, or clearing it, and when.
///
/// A cleared rating is remembered with its time, so that a change older than the clearing,
/// arriving late from another device, does not bring the rating back.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RatingRecord {
    /// `None` once cleared.
    pub rating: Option<UserRating>,
    /// When the change was made, in seconds since the Unix epoch.
    pub at: i64,
}

/// What a change did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateOutcome {
    /// The rating is now the one given.
    Rated,
    /// The rating was cleared.
    Cleared,
    /// Nothing changed: the change is older than the one held.
    Superseded,
    /// Nothing changed: there was no rating to clear.
    NothingToClear,
}

#[derive(Debug, thiserror::Error)]
pub enum RatingError {
    #[error("a rating is from 0 to {MAX_RATING}")]
    OffTheScale,
    #[error("a rating cannot be dated in the future")]
    InTheFuture,
    #[error(transparent)]
    Storage(#[from] WatchError),
}

/// Refuses a change dated after `now`, as the reference does.
///
/// # Errors
///
/// [`RatingError::InTheFuture`].
pub const fn check_time(at: i64, now: i64) -> Result<(), RatingError> {
    if at > now {
        Err(RatingError::InTheFuture)
    } else {
        Ok(())
    }
}

/// Rates a work, or clears its rating with `None`, as of `at`.
///
/// # Errors
///
/// Fails for a change dated after `now`, or when storage does.
pub async fn rate(
    watch: &dyn WatchRepository,
    viewer: UserId,
    guid: &str,
    rating: Option<UserRating>,
    at: i64,
    now: i64,
) -> Result<RateOutcome, RatingError> {
    check_time(at, now)?;
    let held = watch
        .ratings(viewer, &[guid.to_owned()])
        .await?
        .into_iter()
        .next()
        .map(|(_, record)| record);
    let outcome = match (rating, held) {
        (_, Some(held)) if at < held.at => return Ok(RateOutcome::Superseded),
        (None, None | Some(RatingRecord { rating: None, .. })) => {
            return Ok(RateOutcome::NothingToClear)
        }
        (Some(_), _) => RateOutcome::Rated,
        (None, Some(_)) => RateOutcome::Cleared,
    };
    watch
        .save_rating(viewer, guid, RatingRecord { rating, at })
        .await?;
    Ok(outcome)
}

/// Which items a listing keeps by the viewer's rating. Every part given must hold; a rating
/// cleared is no rating.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RatingCondition {
    /// Rated one of these, or — with `unrated` true — not rated at all.
    pub any_of: Vec<f64>,
    /// `true`: not rated, or rated one of `any_of` when there are any. `false`: rated.
    pub unrated: Option<bool>,
    /// Rated, and none of these.
    pub none_of: Vec<f64>,
    /// Rated this or more.
    pub at_least: Option<f64>,
    /// Rated more than this.
    pub above: Option<f64>,
    /// Rated this or less.
    pub at_most: Option<f64>,
    /// Rated less than this.
    pub below: Option<f64>,
}

impl RatingCondition {
    /// Whether an item rated `rating`, or not rated, is kept. Storage expresses the same in
    /// SQL.
    pub fn holds(&self, rating: Option<f64>) -> bool {
        let alternatives = !self.any_of.is_empty() || self.unrated == Some(true);
        if alternatives
            && !(rating.is_some_and(|value| self.any_of.contains(&value))
                || (self.unrated == Some(true) && rating.is_none()))
        {
            return false;
        }
        if self.unrated == Some(false) && rating.is_none() {
            return false;
        }
        if !self.requires_rating() {
            return true;
        }
        rating.is_some_and(|value| {
            !self.none_of.contains(&value)
                && self.at_least.is_none_or(|bound| value >= bound)
                && self.above.is_none_or(|bound| value > bound)
                && self.at_most.is_none_or(|bound| value <= bound)
                && self.below.is_none_or(|bound| value < bound)
        })
    }

    /// Whether the exclusions and bounds are in play, each of which keeps rated items only.
    pub fn requires_rating(&self) -> bool {
        !self.none_of.is_empty()
            || self.at_least.is_some()
            || self.above.is_some()
            || self.at_most.is_some()
            || self.below.is_some()
    }
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;
    use crate::testing::InMemoryWatch;

    const VIEWER: UserId = UserId::new(1);

    fn rating(value: f64) -> UserRating {
        UserRating::new(value).unwrap()
    }

    async fn held(watch: &InMemoryWatch) -> Option<RatingRecord> {
        watch
            .ratings(VIEWER, &["g".to_owned()])
            .await
            .unwrap()
            .into_iter()
            .next()
            .map(|(_, record)| record)
    }

    #[test]
    fn a_rating_is_from_zero_to_ten() {
        for value in [0.0, 0.04, 7.26, 10.0] {
            assert!(UserRating::new(value).is_ok(), "{value}");
        }
        // The reference keeps 11, 100 and infinity, and nulls the rating on any other
        // negative than -1; the spec bounds it to ten.
        for value in [-0.5, 10.01, 100.0, f64::INFINITY, f64::NAN] {
            assert!(UserRating::new(value).is_err(), "{value}");
        }
    }

    #[tokio::test]
    async fn the_latest_change_wins_and_an_older_one_is_ignored() {
        let watch = InMemoryWatch::default();
        let rate = |value, at| rate(&watch, VIEWER, "g", value, at, 1_000);
        // Never rated: any date will do.
        assert_eq!(
            rate(Some(rating(5.0)), 400).await.unwrap(),
            RateOutcome::Rated
        );
        assert_eq!(
            rate(Some(rating(6.0)), 500).await.unwrap(),
            RateOutcome::Rated
        );
        // The same date replaces, an earlier one does not.
        assert_eq!(
            rate(Some(rating(7.0)), 500).await.unwrap(),
            RateOutcome::Rated
        );
        assert_eq!(
            rate(Some(rating(8.0)), 499).await.unwrap(),
            RateOutcome::Superseded
        );
        assert_eq!(
            held(&watch).await,
            Some(RatingRecord {
                rating: Some(rating(7.0)),
                at: 500
            })
        );
    }

    #[tokio::test]
    async fn clearing_is_remembered_against_older_ratings() {
        let watch = InMemoryWatch::default();
        let rate = |value, at| rate(&watch, VIEWER, "g", value, at, 1_000);
        assert_eq!(rate(None, 100).await.unwrap(), RateOutcome::NothingToClear);
        assert_eq!(held(&watch).await, None);
        rate(Some(rating(6.0)), 600).await.unwrap();
        assert_eq!(rate(None, 700).await.unwrap(), RateOutcome::Cleared);
        assert_eq!(rate(None, 800).await.unwrap(), RateOutcome::NothingToClear);
        assert_eq!(
            rate(Some(rating(5.0)), 650).await.unwrap(),
            RateOutcome::Superseded
        );
        assert_eq!(
            held(&watch).await,
            Some(RatingRecord {
                rating: None,
                at: 700
            })
        );
        assert_eq!(
            rate(Some(rating(5.0)), 700).await.unwrap(),
            RateOutcome::Rated
        );
    }

    #[tokio::test]
    async fn a_change_dated_in_the_future_is_refused() {
        let watch = InMemoryWatch::default();
        let refused = rate(&watch, VIEWER, "g", Some(rating(5.0)), 1_001, 1_000).await;
        assert!(matches!(refused, Err(RatingError::InTheFuture)));
        assert_eq!(held(&watch).await, None);
    }

    #[test]
    fn a_condition_keeps_what_the_reference_keeps() {
        let condition = |condition: RatingCondition| {
            [Some(8.0), Some(6.0), None]
                .into_iter()
                .filter(|rating| condition.holds(*rating))
                .collect::<Vec<_>>()
        };
        // `userRating=8`, `=-1` and `=8,-1`.
        assert_eq!(
            condition(RatingCondition {
                any_of: vec![8.0],
                ..RatingCondition::default()
            }),
            [Some(8.0)]
        );
        assert_eq!(
            condition(RatingCondition {
                unrated: Some(true),
                ..RatingCondition::default()
            }),
            [None]
        );
        assert_eq!(
            condition(RatingCondition {
                any_of: vec![8.0],
                unrated: Some(true),
                ..RatingCondition::default()
            }),
            [Some(8.0), None]
        );
        // `!=8` and `<=8` leave the unrated out, as every comparison does.
        assert_eq!(
            condition(RatingCondition {
                none_of: vec![8.0],
                ..RatingCondition::default()
            }),
            [Some(6.0)]
        );
        assert_eq!(
            condition(RatingCondition {
                at_most: Some(8.0),
                ..RatingCondition::default()
            }),
            [Some(8.0), Some(6.0)]
        );
        assert_eq!(
            condition(RatingCondition {
                above: Some(6.0),
                below: Some(9.0),
                ..RatingCondition::default()
            }),
            [Some(8.0)]
        );
        assert_eq!(
            condition(RatingCondition {
                unrated: Some(false),
                ..RatingCondition::default()
            }),
            [Some(8.0), Some(6.0)]
        );
    }
}
