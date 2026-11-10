//! Listing arguments: what to keep, and in what order.

use async_graphql::{Enum, InputObject};
use open_plex_core::metadata::{
    FrameRange, FrameSize, ItemDepth, ItemFilter, ItemKind, ItemSort, TagCondition,
};
use open_plex_core::page::Direction;
use open_plex_core::watch::Viewer;
use open_plex_core::RatingKey;

use crate::tag::{TagFilterError, TagFilterInput};

/// The kind of media an item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum ItemType {
    /// A film.
    Movie,
    /// A television series.
    Show,
    /// A season of a series.
    Season,
    /// An episode of a series.
    Episode,
}

impl From<ItemType> for ItemKind {
    fn from(kind: ItemType) -> Self {
        match kind {
            ItemType::Movie => Self::Movie,
            ItemType::Show => Self::Show,
            ItemType::Season => Self::Season,
            ItemType::Episode => Self::Episode,
        }
    }
}

impl From<ItemKind> for ItemType {
    fn from(kind: ItemKind) -> Self {
        match kind {
            ItemKind::Movie => Self::Movie,
            ItemKind::Show => Self::Show,
            ItemKind::Season => Self::Season,
            ItemKind::Episode => Self::Episode,
        }
    }
}

/// How deep into a library's hierarchy a listing reaches.
///
/// A series library holds three generations in one place — series, seasons and episodes —
/// and listing all of them side by side is almost never what a caller wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Enum)]
pub enum ItemDepthInput {
    /// Entries a library shows on its front page: the films of a film library, the series of
    /// a series library. Seasons and episodes are reached through `Show.seasons` and
    /// `Season.episodes`.
    #[default]
    TopLevel,
    /// Every item in the library, at any depth, side by side.
    All,
}

impl From<ItemDepthInput> for ItemDepth {
    fn from(depth: ItemDepthInput) -> Self {
        match depth {
            ItemDepthInput::TopLevel => Self::TopLevel,
            ItemDepthInput::All => Self::All,
        }
    }
}

/// How a library listing is ordered.
///
/// An enum rather than a string because the value becomes a SQL identifier, which cannot be
/// bound as a parameter: an exhaustive match is the only form that is both injection-proof
/// and verifiable by reading.
///
/// Under every order but `TITLE`, items with equal values follow in title order, ascending
/// whichever the direction, as on the reference server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Enum)]
pub enum ItemOrder {
    /// By sort title, falling back to the title. Case-insensitive. Equal titles follow by id, in the same direction.
    #[default]
    Title,
    /// By when the item was added to the library.
    AddedAt,
    /// By release date. Items without one come first ascending, last descending — as for every order below.
    ReleasedOn,
    /// By critics' rating.
    Rating,
    /// By audience rating.
    AudienceRating,
    /// By running time: an item ranks by its shortest version ascending, by its longest descending.
    Duration,
    /// By frame height, the same way: an item ranks by its smallest version ascending, by its largest descending.
    Height,
    /// By when the caller last played the item. A caller without watch state has played nothing.
    LastViewedAt,
    /// By when a series' episodes were added: a series ranks by its first episode ascending, by its latest descending.
    EpisodeAddedAt,
    /// Shuffled, by the `seed` argument: the same seed gives the same order, so pages follow one another.
    Random,
}

impl ItemOrder {
    /// The storage ordering, for a caller who watches as `viewer`.
    pub(crate) const fn sort(self, viewer: Option<Viewer>, seed: u32) -> ItemSort {
        match self {
            Self::Title => ItemSort::Title,
            Self::AddedAt => ItemSort::AddedAt,
            Self::ReleasedOn => ItemSort::ReleasedOn,
            Self::Rating => ItemSort::Rating,
            Self::AudienceRating => ItemSort::AudienceRating,
            Self::Duration => ItemSort::Duration,
            Self::Height => ItemSort::Height,
            Self::LastViewedAt => ItemSort::LastViewedAt(viewer),
            Self::EpisodeAddedAt => ItemSort::EpisodeAddedAt,
            Self::Random => ItemSort::Shuffled(seed),
        }
    }
}

/// Sort direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Enum)]
pub enum SortDirection {
    /// Smallest or earliest first.
    #[default]
    Ascending,
    /// Largest or latest first.
    Descending,
}

impl From<SortDirection> for Direction {
    fn from(direction: SortDirection) -> Self {
        match direction {
            SortDirection::Ascending => Self::Ascending,
            SortDirection::Descending => Self::Descending,
        }
    }
}

/// Narrows a listing to a subset of a library.
///
/// Every field is combined with `AND`; an absent field places no constraint.
#[derive(Debug, Clone, Default, InputObject)]
pub struct ItemFilterInput {
    /// Keep only items of this kind.
    ///
    /// Overrides `depth`, because naming a kind is already a statement about depth: asking
    /// for episodes and getting nothing because episodes have parents would be obtuse.
    pub kind: Option<ItemType>,
    /// How deep to look. Defaults to the library's top level.
    pub depth: Option<ItemDepthInput>,
    /// Keep only items released in or after this year.
    pub year_from: Option<i32>,
    /// Keep only items released on or after this date, written `2025-11-10`.
    pub released_from: Option<String>,
    /// Keep only items whose audience rating, on its ten-point scale, is at least this.
    pub audience_rating_from: Option<f64>,
    /// Keep only items released in or before this year.
    pub year_to: Option<i32>,
    /// Keep only items with exactly this age-rating label, as the metadata source expresses
    /// it — for example `fr/Tous publics`. Not comparable across countries.
    pub content_rating: Option<String>,
    /// Keep only items whose studio contains this text, case-insensitively.
    ///
    /// A substring rather than a name, which is what a caller typing "HBO" means and what
    /// the Plex surface does. It cannot use an index — a leading wildcard has nothing to
    /// seek to — so it is applied after the library predicate and its cost is bounded by
    /// the library, not the table.
    pub studio_contains: Option<String>,
    /// Keep only items having a word that starts with this text.
    ///
    /// Matches the title, the sort title and the original title, with accents folded: `le`
    /// finds both "Le Nouveau Jouet" and "Sans Laisser de Traces". A word prefix rather than
    /// full-text search, because full-text indexing is the least portable feature across
    /// database engines and the engine is meant to stay swappable.
    pub title_word_prefix: Option<String>,
    /// Keep only items with a version whose frame falls in this range: `{reaching: {width:
    /// 2880, height: 1540}}` for 4K.
    ///
    /// The one filter that reaches past the item table, so the one that can slow a listing.
    pub frame: Option<FrameRangeInput>,
    /// Keep only items carrying the named tags. Every condition must hold.
    pub tags: Option<Vec<TagFilterInput>>,
    /// Leave out this item, as returned in `id`: "the rest of this director's films".
    pub excluded_id: Option<async_graphql::ID>,
    /// Keep only items the caller watched to the end at least once, or never. A series counts
    /// its own viewings, not its episodes': one whose watched episode was unmarked since
    /// stays watched. A caller without watch state has watched nothing.
    pub watched: Option<bool>,
    /// Keep only items the caller has a position in, or has none in.
    pub in_progress: Option<bool>,
    /// Keep only items every episode of which the caller watched, or not. An item without
    /// episodes has none unwatched.
    pub all_episodes_watched: Option<bool>,
    /// Keep only items with an episode the caller watched or started, or with none: the
    /// series not yet begun.
    pub any_episode_started: Option<bool>,
}

/// Why a filter cannot be honored.
#[derive(Debug, thiserror::Error)]
pub enum FilterError {
    #[error(transparent)]
    Tag(#[from] TagFilterError),
    #[error("`{0}` is not a date written YYYY-MM-DD")]
    NotADate(String),
}

/// Whether `text` is a calendar date as stored: four digits, two, two, by hyphens. Compared
/// as text in storage, so a looser form would compare wrongly rather than fail.
fn is_iso_date(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 10
        && bytes.iter().enumerate().all(|(position, byte)| {
            if position == 4 || position == 7 {
                *byte == b'-'
            } else {
                byte.is_ascii_digit()
            }
        })
}

/// A frame size, in pixels.
#[derive(Debug, Clone, Copy, InputObject)]
pub struct FrameSizeInput {
    /// Width in pixels.
    pub width: i32,
    /// Height in pixels.
    pub height: i32,
}

/// The frames a resolution filter keeps: those reaching one size, by either dimension, and
/// short of another, by both.
#[derive(Debug, Clone, Copy, Default, InputObject)]
pub struct FrameRangeInput {
    /// Keep a frame at least this wide or at least this tall: 1920×800 reaches 1902×1070.
    pub reaching: Option<FrameSizeInput>,
    /// Keep a frame narrower and shorter than this.
    pub short_of: Option<FrameSizeInput>,
}

impl From<FrameSizeInput> for FrameSize {
    fn from(size: FrameSizeInput) -> Self {
        Self {
            width: size.width,
            height: size.height,
        }
    }
}

impl From<FrameRangeInput> for FrameRange {
    fn from(range: FrameRangeInput) -> Self {
        Self {
            reaching: range.reaching.map(Into::into),
            short_of: range.short_of.map(Into::into),
        }
    }
}

impl TryFrom<ItemFilterInput> for ItemFilter {
    type Error = FilterError;

    fn try_from(input: ItemFilterInput) -> Result<Self, Self::Error> {
        if let Some(date) = input
            .released_from
            .as_ref()
            .filter(|date| !is_iso_date(date))
        {
            return Err(FilterError::NotADate(date.clone()));
        }
        let tags = input
            .tags
            .unwrap_or_default()
            .into_iter()
            .map(TagCondition::try_from)
            .collect::<Result<_, _>>()?;
        Ok(Self {
            kind: input.kind.map(Into::into),
            depth: input.depth.unwrap_or_default().into(),
            year_from: input.year_from,
            year_to: input.year_to,
            released_from: input.released_from,
            audience_rating_from: input.audience_rating_from,
            content_rating: input.content_rating,
            studio_contains: input.studio_contains,
            // Blank input is no constraint: an empty search box must not become a filter
            // that matches everything.
            title_word_prefix: input
                .title_word_prefix
                .filter(|text| !text.trim().is_empty()),
            frame: input.frame.map(Into::into),
            tags,
            excluded: input
                .excluded_id
                .and_then(|id| crate::GlobalId::decode(&id).ok())
                .map(|decoded| RatingKey::new(decoded.key)),
            watched: input.watched,
            in_progress: input.in_progress,
            episodes_watched: input.all_episodes_watched,
            episodes_started: input.any_episode_started,
            // The caller's, which only the resolver knows.
            viewer: None,
        })
    }
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    #[test]
    fn a_blank_prefix_is_no_filter() {
        let input = ItemFilterInput {
            title_word_prefix: Some("   ".to_owned()),
            ..ItemFilterInput::default()
        };
        assert_eq!(ItemFilter::try_from(input).unwrap().title_word_prefix, None);
    }

    #[test]
    fn a_listing_covers_the_top_level_unless_told_otherwise() {
        // What `/library/sections/{id}/all` shows on a series library: 1,980 series rather
        // than the 86,655 rows the section holds.
        assert!(ItemFilter::try_from(ItemFilterInput::default())
            .unwrap()
            .top_level_only());

        let deep = ItemFilterInput {
            depth: Some(ItemDepthInput::All),
            ..ItemFilterInput::default()
        };
        assert!(!ItemFilter::try_from(deep).unwrap().top_level_only());

        // A named kind wins over the default depth, or asking for episodes would answer
        // with nothing.
        let episodes = ItemFilterInput {
            kind: Some(ItemType::Episode),
            ..ItemFilterInput::default()
        };
        assert!(!ItemFilter::try_from(episodes).unwrap().top_level_only());
    }

    #[test]
    fn carries_every_field_through() {
        let input = ItemFilterInput {
            kind: Some(ItemType::Show),
            year_from: Some(2000),
            year_to: Some(2010),
            content_rating: Some("fr/16".to_owned()),
            studio_contains: Some("HBO".to_owned()),
            title_word_prefix: Some("the".to_owned()),
            frame: Some(FrameRangeInput {
                reaching: Some(FrameSizeInput {
                    width: 2880,
                    height: 1540,
                }),
                short_of: None,
            }),
            depth: Some(ItemDepthInput::All),
            excluded_id: None,
            tags: Some(vec![TagFilterInput {
                kind: crate::tag::TagType::Genre,
                ids: None,
                names: Some(vec!["Drama".to_owned()]),
            }]),
            watched: Some(false),
            in_progress: Some(true),
            all_episodes_watched: Some(false),
            released_from: Some("2025-11-10".to_owned()),
            audience_rating_from: Some(7.0),
            any_episode_started: Some(false),
        };
        let filter = ItemFilter::try_from(input).unwrap();
        assert_eq!(filter.kind, Some(ItemKind::Show));
        assert_eq!(filter.year_from, Some(2000));
        assert_eq!(filter.year_to, Some(2010));
        assert_eq!(filter.content_rating.as_deref(), Some("fr/16"));
        assert_eq!(filter.studio_contains.as_deref(), Some("HBO"));
        assert_eq!(filter.title_word_prefix.as_deref(), Some("the"));
        assert!(filter
            .frame
            .is_some_and(|range| range.contains(Some(3840), Some(1600))));
        assert_eq!(filter.depth, ItemDepth::All);
        assert!(filter.needs_media());
        assert_eq!(filter.tags.len(), 1);
        assert_eq!(filter.watched, Some(false));
        assert_eq!(filter.in_progress, Some(true));
        assert_eq!(filter.episodes_watched, Some(false));
        assert_eq!(filter.released_from.as_deref(), Some("2025-11-10"));
        assert_eq!(filter.audience_rating_from, Some(7.0));
        assert_eq!(filter.episodes_started, Some(false));
    }

    #[test]
    fn a_date_in_another_form_is_refused() {
        for date in ["2025-1-10", "10/11/2025", "2025-11-10T00:00", "-1y"] {
            let input = ItemFilterInput {
                released_from: Some(date.to_owned()),
                ..ItemFilterInput::default()
            };
            assert!(
                matches!(ItemFilter::try_from(input), Err(FilterError::NotADate(_))),
                "{date}"
            );
        }
    }

    #[test]
    fn item_kinds_round_trip() {
        for kind in [
            ItemKind::Movie,
            ItemKind::Show,
            ItemKind::Season,
            ItemKind::Episode,
        ] {
            assert_eq!(ItemKind::from(ItemType::from(kind)), kind);
        }
    }
}
