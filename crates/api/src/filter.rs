//! Listing arguments: what to keep, and in what order.

use async_graphql::{Enum, InputObject};
use open_plex_core::metadata::{ItemDepth, ItemFilter, ItemKind, ItemSort, TagCondition};
use open_plex_core::page::Direction;

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
/// Short on purpose. Both variants are backed by a non-null column, which is what lets a
/// cursor locate a position unambiguously; ordering by a nullable one — year, rating —
/// additionally needs the cursor to encode absence and the null position to be pinned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Enum)]
pub enum ItemOrder {
    /// By sort title, falling back to the title. Case-insensitive.
    #[default]
    Title,
    /// By when the item was added to the library.
    AddedAt,
}

impl From<ItemOrder> for ItemSort {
    fn from(order: ItemOrder) -> Self {
        match order {
            ItemOrder::Title => Self::Title,
            ItemOrder::AddedAt => Self::AddedAt,
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
    /// Keep only items with a version at least this wide, in pixels — 3840 for 4K.
    ///
    /// The one filter that reaches past the item table, so the one that can slow a listing.
    pub min_width: Option<i32>,
    /// Keep only items carrying the named tags. Every condition must hold.
    pub tags: Option<Vec<TagFilterInput>>,
}

impl TryFrom<ItemFilterInput> for ItemFilter {
    type Error = TagFilterError;

    fn try_from(input: ItemFilterInput) -> Result<Self, Self::Error> {
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
            content_rating: input.content_rating,
            studio_contains: input.studio_contains,
            // Blank input is no constraint: an empty search box must not become a filter
            // that matches everything.
            title_word_prefix: input
                .title_word_prefix
                .filter(|text| !text.trim().is_empty()),
            min_width: input.min_width,
            tags,
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
            min_width: Some(3840),
            depth: Some(ItemDepthInput::All),
            tags: Some(vec![TagFilterInput {
                kind: crate::tag::TagType::Genre,
                ids: None,
                names: Some(vec!["Drama".to_owned()]),
            }]),
        };
        let filter = ItemFilter::try_from(input).unwrap();
        assert_eq!(filter.kind, Some(ItemKind::Show));
        assert_eq!(filter.year_from, Some(2000));
        assert_eq!(filter.year_to, Some(2010));
        assert_eq!(filter.content_rating.as_deref(), Some("fr/16"));
        assert_eq!(filter.studio_contains.as_deref(), Some("HBO"));
        assert_eq!(filter.title_word_prefix.as_deref(), Some("the"));
        assert_eq!(filter.min_width, Some(3840));
        assert_eq!(filter.depth, ItemDepth::All);
        assert!(filter.needs_media());
        assert_eq!(filter.tags.len(), 1);
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
