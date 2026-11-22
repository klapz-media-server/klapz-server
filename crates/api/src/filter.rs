//! Listing arguments: what to keep, and in what order.

use async_graphql::{Enum, InputObject};
use klapz_core::collection::{CollectionCondition, CollectionRef};
use klapz_core::metadata::{
    FrameRange, FrameSize, ItemDepth, ItemFilter, ItemKind, ItemSort, TagCondition, TrackCondition,
    TrackField as DomainTrackField,
};
use klapz_core::page::{Direction, MAX_ITEMS};
use klapz_core::user_rating::RatingCondition;
use klapz_core::{CollectionId, RatingKey, UserId};

use crate::tag::{TagFilterError, TagFilterInput};
use crate::DateTime;

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
    /// A musician or a band. This server holds no music yet: a filter naming a music kind
    /// keeps nothing.
    Artist,
    /// An album of an artist.
    Album,
    /// A track of an album.
    Track,
}

impl ItemType {
    /// The kind of item this names, or `None` for a kind this server holds none of.
    pub(crate) const fn held(self) -> Option<ItemKind> {
        match self {
            Self::Movie => Some(ItemKind::Movie),
            Self::Show => Some(ItemKind::Show),
            Self::Season => Some(ItemKind::Season),
            Self::Episode => Some(ItemKind::Episode),
            Self::Artist | Self::Album | Self::Track => None,
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
    /// Items with nothing below them: the films of a film library, the episodes of a series
    /// library. A `kind` narrows this rather than replacing it, so `SHOW` finds nothing.
    Leaves,
}

impl From<ItemDepthInput> for ItemDepth {
    fn from(depth: ItemDepthInput) -> Self {
        match depth {
            ItemDepthInput::TopLevel => Self::TopLevel,
            ItemDepthInput::All => Self::All,
            ItemDepthInput::Leaves => Self::Leaves,
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
    /// By the caller's rating of the item. An item it has not rated has no value, and a caller without watch state has rated nothing.
    UserRating,
    /// By when a series' episodes were added: a series ranks by its first episode ascending, by its latest descending.
    EpisodeAddedAt,
    /// Episodes by series, season and episode: the series' sort title, the season's number, the episode's, its release date — each absent one last ascending — then the episode's own title. Anything else follows its title, as under `TITLE`.
    Series,
    /// Shuffled, by the `seed` argument: the same seed gives the same order, so pages follow one another.
    Random,
}

impl ItemOrder {
    /// The storage ordering, for a caller who watches as `viewer`.
    pub(crate) const fn sort(self, viewer: Option<UserId>, seed: u32) -> ItemSort {
        match self {
            Self::Title => ItemSort::Title,
            Self::AddedAt => ItemSort::AddedAt,
            Self::ReleasedOn => ItemSort::ReleasedOn,
            Self::Rating => ItemSort::Rating,
            Self::AudienceRating => ItemSort::AudienceRating,
            Self::Duration => ItemSort::Duration,
            Self::Height => ItemSort::Height,
            Self::LastViewedAt => ItemSort::LastViewedAt(viewer),
            Self::UserRating => ItemSort::UserRating(viewer),
            Self::EpisodeAddedAt => ItemSort::EpisodeAddedAt,
            Self::Series => ItemSort::Episodes,
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
    /// Overrides the default depth, because naming a kind is already a statement about depth:
    /// asking for episodes and getting nothing because episodes have parents would be obtuse.
    /// Under `LEAVES` both hold.
    pub kind: Option<ItemType>,
    /// How deep to look. Defaults to the library's top level.
    pub depth: Option<ItemDepthInput>,
    /// Keep only items released in or after this year.
    pub year_from: Option<i32>,
    /// Keep only items released at or after this time. A release date is its day's midnight,
    /// UTC.
    pub released_from: Option<DateTime>,
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
    /// Keep only items whose title this text completes, as a search field offers while it
    /// is typed into.
    ///
    /// The text is one phrase beginning a word of the title, a word beginning after a space:
    /// `first sl` completes "The First Slam Dunk", `aim` does not complete "Le mal-aimé".
    /// Case does not count and accents do, so `aime` does not complete "aimé". Only the
    /// title is read, not the sort or original title. Empty text completes nothing.
    pub title_completion: Option<String>,
    /// Keep only items with a version whose frame falls in this range: `{reaching: {width:
    /// 2880, height: 1540}}` for 4K.
    ///
    /// The one filter that reaches past the item table, so the one that can slow a listing.
    pub frame: Option<FrameRangeInput>,
    /// Keep only these items, as returned in `id`: at least one, and at most 500.
    pub ids: Option<Vec<async_graphql::ID>>,
    /// Keep only items whose own tracks hold the values named. Every condition must hold. A
    /// series has no tracks, its episodes holding them.
    pub tracks: Option<Vec<TrackFilterInput>>,
    /// Keep only items carrying the named tags. Every condition must hold.
    pub tags: Option<Vec<TagFilterInput>>,
    /// Keep only items in the named collections. Every condition must hold.
    pub collections: Option<Vec<CollectionFilterInput>>,
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
    /// Keep only items the caller last played at or after this time.
    pub last_viewed_from: Option<DateTime>,
    /// Keep only items the caller last played at or before this time.
    pub last_viewed_to: Option<DateTime>,
    /// Keep only items by the caller's rating of them.
    pub user_rating: Option<UserRatingFilterInput>,
    /// Keep only the caller's favorites (`true`), or what it did not make one (`false`).
    pub favorite: Option<bool>,
}

/// Keeps items by the caller's rating, out of ten. Every field given must hold. Each
/// comparison keeps rated items only: an item the caller has not rated, or whose rating it
/// cleared, has no value to compare.
#[derive(Debug, Clone, Default, InputObject)]
pub struct UserRatingFilterInput {
    /// Keep items rated one of these, or, with `unrated` true, not rated at all.
    pub any_of: Option<Vec<f64>>,
    /// True keeps the items not rated, with those rated one of `anyOf` when it is given; false
    /// keeps the items rated.
    pub unrated: Option<bool>,
    /// Keep items rated, and none of these.
    pub none_of: Option<Vec<f64>>,
    /// Keep items rated this or more.
    pub at_least: Option<f64>,
    /// Keep items rated more than this.
    pub above: Option<f64>,
    /// Keep items rated this or less.
    pub at_most: Option<f64>,
    /// Keep items rated less than this.
    pub below: Option<f64>,
}

impl TryFrom<UserRatingFilterInput> for RatingCondition {
    type Error = FilterError;

    fn try_from(input: UserRatingFilterInput) -> Result<Self, Self::Error> {
        let any_of = input.any_of.unwrap_or_default();
        let none_of = input.none_of.unwrap_or_default();
        if any_of.len() > TagCondition::MAX_VALUES || none_of.len() > TagCondition::MAX_VALUES {
            return Err(FilterError::RatingValueCount);
        }
        Ok(Self {
            any_of,
            unrated: input.unrated,
            none_of,
            at_least: input.at_least,
            above: input.above,
            at_most: input.at_most,
            below: input.below,
        })
    }
}

/// Keeps items in at least one of the collections named. Name them by id, by the number
/// their items carry them under, or by title; at least one, and at most 50 in all.
#[derive(Debug, Clone, Default, InputObject)]
pub struct CollectionFilterInput {
    /// Collections, as returned in `Collection.id`.
    pub ids: Option<Vec<async_graphql::ID>>,
    /// Collections by `Collection.tagNumber`.
    pub tag_numbers: Option<Vec<String>>,
    /// Collections by title, matched exactly, ignoring case.
    pub titles: Option<Vec<String>>,
}

impl TryFrom<CollectionFilterInput> for CollectionCondition {
    type Error = FilterError;

    fn try_from(input: CollectionFilterInput) -> Result<Self, Self::Error> {
        let mut any_of = Vec::new();
        for id in input.ids.unwrap_or_default() {
            let decoded = crate::GlobalId::decode(&id)
                .ok()
                .filter(|decoded| decoded.type_name == crate::collection::COLLECTION_TYPE)
                .ok_or(FilterError::NotACollection)?;
            any_of.push(CollectionRef::Id(CollectionId::new(decoded.key)));
        }
        for number in input.tag_numbers.unwrap_or_default() {
            let parsed = number.parse().map_err(|_| FilterError::NotACollection)?;
            any_of.push(CollectionRef::TagNumber(parsed));
        }
        any_of.extend(
            input
                .titles
                .unwrap_or_default()
                .into_iter()
                .map(CollectionRef::Title),
        );
        if any_of.is_empty() || any_of.len() > TagCondition::MAX_VALUES {
            return Err(FilterError::CollectionCount);
        }
        Ok(Self { any_of })
    }
}

/// A value a track holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum TrackField {
    /// The codec of a video track: `h264`, `hevc`.
    VideoCodec,
    /// The codec of an audio track: `ac3`, `eac3`, `truehd`.
    AudioCodec,
    /// The channel layout of an audio track, as the analysis names it: `5.1(side)`, `stereo`.
    AudioLayout,
    /// The format of a subtitle track, embedded or beside the file: `srt`, `pgs`.
    SubtitleCodec,
    /// The language an audio track states, as its file writes it: `fre` and `fra` apart. Only
    /// a scanned file states one.
    AudioLanguage,
    /// The language a subtitle track states, as its file writes it.
    SubtitleLanguage,
}

impl From<TrackField> for DomainTrackField {
    fn from(field: TrackField) -> Self {
        match field {
            TrackField::VideoCodec => Self::VideoCodec,
            TrackField::AudioCodec => Self::AudioCodec,
            TrackField::AudioLayout => Self::AudioLayout,
            TrackField::SubtitleCodec => Self::SubtitleCodec,
            TrackField::AudioLanguage => Self::AudioLanguage,
            TrackField::SubtitleLanguage => Self::SubtitleLanguage,
        }
    }
}

/// Keeps items with a track of any of their versions holding one of the values, matched
/// exactly, case included; or, negated, the items with none.
#[derive(Debug, Clone, InputObject)]
pub struct TrackFilterInput {
    /// Which value of the tracks to compare.
    pub field: TrackField,
    /// The values, at least one and at most 50, any of which a track may hold.
    pub any_of: Vec<String>,
    /// Keep the items none of whose tracks holds any of the values instead.
    #[graphql(default)]
    pub negated: bool,
}

impl TryFrom<TrackFilterInput> for TrackCondition {
    type Error = FilterError;

    fn try_from(input: TrackFilterInput) -> Result<Self, Self::Error> {
        if input.any_of.is_empty() || input.any_of.len() > TagCondition::MAX_VALUES {
            return Err(FilterError::TrackValueCount);
        }
        Ok(Self {
            field: input.field.into(),
            any_of: input.any_of,
            negated: input.negated,
        })
    }
}

/// Reads `ids`, which must name between one and `MAX_ITEMS` items: an empty list kept
/// everything would be the opposite of what it says.
fn item_ids(ids: Option<Vec<async_graphql::ID>>) -> Result<Vec<RatingKey>, FilterError> {
    let Some(ids) = ids else {
        return Ok(Vec::new());
    };
    if ids.is_empty() || ids.len() > MAX_ITEMS as usize {
        return Err(FilterError::ItemCount);
    }
    ids.iter()
        .map(|id| {
            crate::GlobalId::decode(id)
                .ok()
                .filter(|decoded| decoded.type_name == crate::item::ITEM_TYPE)
                .map(|decoded| RatingKey::new(decoded.key))
                .ok_or(FilterError::NotAnItem)
        })
        .collect()
}

/// Why a filter cannot be honored.
#[derive(Debug, thiserror::Error)]
pub enum FilterError {
    #[error(transparent)]
    Tag(#[from] TagFilterError),
    #[error("not a collection id or tag number")]
    NotACollection,
    #[error(
        "a collection filter names between one and {} collections",
        TagCondition::MAX_VALUES
    )]
    CollectionCount,
    #[error(
        "a track filter names between one and {} values",
        TagCondition::MAX_VALUES
    )]
    TrackValueCount,
    #[error("an item filter names between one and {MAX_ITEMS} items")]
    ItemCount,
    #[error(
        "a rating filter names at most {} values of each kind",
        TagCondition::MAX_VALUES
    )]
    RatingValueCount,
    #[error("not an item id")]
    NotAnItem,
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
        let tags = input
            .tags
            .unwrap_or_default()
            .into_iter()
            .map(TagCondition::try_from)
            .collect::<Result<_, _>>()?;
        let collections = input
            .collections
            .unwrap_or_default()
            .into_iter()
            .map(CollectionCondition::try_from)
            .collect::<Result<_, _>>()?;
        let tracks = input
            .tracks
            .unwrap_or_default()
            .into_iter()
            .map(TrackCondition::try_from)
            .collect::<Result<_, _>>()?;
        Ok(Self {
            ids: item_ids(input.ids)?,
            tracks,
            kind: input.kind.and_then(ItemType::held),
            kind_not_held: input.kind.is_some_and(|kind| kind.held().is_none()),
            depth: input.depth.unwrap_or_default().into(),
            year_from: input.year_from,
            year_to: input.year_to,
            released_from: input.released_from.map(DateTime::unix_seconds),
            audience_rating_from: input.audience_rating_from,
            content_rating: input.content_rating,
            studio_contains: input.studio_contains,
            // Blank input is no constraint: an empty search box must not become a filter
            // that matches everything.
            title_word_prefix: input
                .title_word_prefix
                .filter(|text| !text.trim().is_empty()),
            title_completion: input.title_completion,
            frame: input.frame.map(Into::into),
            tags,
            collections,
            // Set by the resolver listing a tag's or a person's works, never by a client.
            credited: None,
            excluded: input
                .excluded_id
                .and_then(|id| crate::GlobalId::decode(&id).ok())
                .map(|decoded| RatingKey::new(decoded.key)),
            watched: input.watched,
            in_progress: input.in_progress,
            episodes_watched: input.all_episodes_watched,
            episodes_started: input.any_episode_started,
            last_viewed_from: input.last_viewed_from.map(DateTime::unix_seconds),
            last_viewed_to: input.last_viewed_to.map(DateTime::unix_seconds),
            user_rating: input
                .user_rating
                .map(RatingCondition::try_from)
                .transpose()?,
            favorite: input.favorite,
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
            title_completion: Some("first sl".to_owned()),
            frame: Some(FrameRangeInput {
                reaching: Some(FrameSizeInput {
                    width: 2880,
                    height: 1540,
                }),
                short_of: None,
            }),
            depth: Some(ItemDepthInput::All),
            excluded_id: None,
            ids: Some(vec![async_graphql::ID(crate::GlobalId::encode(
                crate::item::ITEM_TYPE,
                46_616,
            ))]),
            tracks: Some(vec![TrackFilterInput {
                field: TrackField::AudioLayout,
                any_of: vec!["5.1".to_owned()],
                negated: true,
            }]),
            tags: Some(vec![TagFilterInput {
                kind: crate::tag::TagType::Genre,
                ids: None,
                names: Some(vec!["Drama".to_owned()]),
            }]),
            collections: Some(vec![CollectionFilterInput {
                tag_numbers: Some(vec!["187346".to_owned()]),
                ..CollectionFilterInput::default()
            }]),
            watched: Some(false),
            in_progress: Some(true),
            all_episodes_watched: Some(false),
            released_from: Some(DateTime::from_unix_seconds(1_762_732_800)),
            audience_rating_from: Some(7.0),
            any_episode_started: Some(false),
            last_viewed_from: Some(DateTime::from_unix_seconds(1_700_000_000)),
            last_viewed_to: None,
            user_rating: Some(UserRatingFilterInput {
                any_of: Some(vec![8.0]),
                unrated: Some(true),
                below: Some(9.5),
                ..UserRatingFilterInput::default()
            }),
            favorite: Some(true),
        };
        let filter = ItemFilter::try_from(input).unwrap();
        assert_eq!(filter.kind, Some(ItemKind::Show));
        assert_eq!(filter.year_from, Some(2000));
        assert_eq!(filter.year_to, Some(2010));
        assert_eq!(filter.content_rating.as_deref(), Some("fr/16"));
        assert_eq!(filter.studio_contains.as_deref(), Some("HBO"));
        assert_eq!(filter.title_word_prefix.as_deref(), Some("the"));
        assert_eq!(filter.title_completion.as_deref(), Some("first sl"));
        assert!(filter
            .frame
            .is_some_and(|range| range.contains(Some(3840), Some(1600))));
        assert_eq!(filter.depth, ItemDepth::All);
        assert!(filter.needs_media());
        assert_eq!(filter.tags.len(), 1);
        assert_eq!(
            filter.collections,
            vec![CollectionCondition {
                any_of: vec![CollectionRef::TagNumber(187_346)]
            }]
        );
        assert_eq!(filter.watched, Some(false));
        assert_eq!(filter.in_progress, Some(true));
        assert_eq!(filter.episodes_watched, Some(false));
        assert_eq!(filter.released_from, Some(1_762_732_800));
        assert_eq!(filter.audience_rating_from, Some(7.0));
        assert_eq!(filter.episodes_started, Some(false));
        assert_eq!(filter.last_viewed_from, Some(1_700_000_000));
        assert_eq!(filter.ids, vec![RatingKey::new(46_616)]);
        assert_eq!(filter.favorite, Some(true));
        assert_eq!(
            filter.user_rating,
            Some(RatingCondition {
                any_of: vec![8.0],
                unrated: Some(true),
                below: Some(9.5),
                ..RatingCondition::default()
            })
        );
        assert_eq!(
            filter.tracks,
            vec![TrackCondition {
                field: DomainTrackField::AudioLayout,
                any_of: vec!["5.1".to_owned()],
                negated: true,
            }]
        );
    }

    #[test]
    fn an_empty_list_of_items_or_values_is_refused() {
        // Either would otherwise read as "no constraint" and keep the whole library.
        let no_items = ItemFilterInput {
            ids: Some(Vec::new()),
            ..ItemFilterInput::default()
        };
        assert!(matches!(
            ItemFilter::try_from(no_items),
            Err(FilterError::ItemCount)
        ));
        let no_values = ItemFilterInput {
            tracks: Some(vec![TrackFilterInput {
                field: TrackField::VideoCodec,
                any_of: Vec::new(),
                negated: false,
            }]),
            ..ItemFilterInput::default()
        };
        assert!(matches!(
            ItemFilter::try_from(no_values),
            Err(FilterError::TrackValueCount)
        ));
        let a_tag = ItemFilterInput {
            ids: Some(vec![async_graphql::ID(crate::GlobalId::encode(
                crate::tag::TAG_TYPE,
                38,
            ))]),
            ..ItemFilterInput::default()
        };
        assert!(matches!(
            ItemFilter::try_from(a_tag),
            Err(FilterError::NotAnItem)
        ));
    }

    #[test]
    fn item_kinds_round_trip() {
        for kind in [
            ItemKind::Movie,
            ItemKind::Show,
            ItemKind::Season,
            ItemKind::Episode,
        ] {
            assert_eq!(ItemType::from(kind).held(), Some(kind));
        }
    }

    #[test]
    fn a_music_kind_keeps_nothing() {
        for kind in [ItemType::Artist, ItemType::Album, ItemType::Track] {
            let input = ItemFilterInput {
                kind: Some(kind),
                ..ItemFilterInput::default()
            };
            let filter = ItemFilter::try_from(input).unwrap();
            assert!(filter.kind_not_held, "{kind:?}");
            assert_eq!(filter.kind, None);
        }
        assert!(
            !ItemFilter::try_from(ItemFilterInput::default())
                .unwrap()
                .kind_not_held
        );
    }
}
