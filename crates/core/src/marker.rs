//! Markers: where a film's or an episode's intro, credits or advertisements sit, and the places
//! a viewer bookmarks in it. What a player's "Skip intro" and "Skip credits" read.
//!
//! A marker has one of two owners. An intro, the credits and a commercial belong to the work:
//! every viewer sees them, and only a library's editors change them. The reference finds them by
//! analysis, which is out of scope here — they are imported, or set by hand. A bookmark belongs
//! to one viewer, who alone sees and changes it. The reference keeps the two in separate tables;
//! here the owner says which it is.
//!
//! Offsets are in milliseconds from the start of the media.

use crate::metadata::MetadataItem;
use crate::{MarkerId, RatingKey, UserId};

/// What a marker marks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MarkerKind {
    Intro,
    Credits,
    /// An advertisement break.
    Commercial,
    /// A place a viewer kept.
    Bookmark,
}

impl MarkerKind {
    /// Whether a marker of this kind belongs to one viewer rather than to the work.
    pub const fn is_personal(self) -> bool {
        matches!(self, Self::Bookmark)
    }
}

/// One marker on one item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marker {
    pub id: MarkerId,
    pub item: RatingKey,
    pub kind: MarkerKind,
    /// The viewer a bookmark belongs to; `None` for the work's own markers.
    pub owner: Option<UserId>,
    pub start_ms: i64,
    /// Where it ends. A bookmark may mark a point and have none; the work's markers always
    /// have one, except 51 credits of the reference library that end before they start, kept as
    /// imported.
    pub end_ms: Option<i64>,
    /// The credits that run to the end, as opposed to a mid-film credit sequence. What a player
    /// skips to the next episode from. Credits only.
    pub is_final: bool,
    /// The version of the detection that found it; `None` for a marker set by hand.
    pub version: Option<i32>,
    /// A bookmark's name.
    pub title: Option<String>,
    /// A bookmark's colour, as its client wrote it (`ff0000`).
    pub color: Option<String>,
}

/// Why a marker cannot be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MarkerError {
    #[error("only films and episodes carry markers")]
    NotPlayable,
    #[error("a marker cannot start before the media does")]
    NegativeStart,
    #[error("a marker must end after it starts")]
    EmptySpan,
    #[error("only a bookmark can mark a point: every other marker needs an end")]
    MissingEnd,
    #[error("only credits can be final")]
    FinalNotCredits,
    #[error("only a bookmark has a title or a colour")]
    NotPersonal,
    #[error("a bookmark belongs to a viewer, and there is none")]
    NoViewer,
}

/// A marker to add, checked against the item it is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMarker {
    item: RatingKey,
    kind: MarkerKind,
    owner: Option<UserId>,
    values: MarkerValues,
}

/// What can change about a marker once it exists: everything but its item, kind and owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkerValues {
    pub start_ms: i64,
    pub end_ms: Option<i64>,
    pub is_final: bool,
    pub title: Option<String>,
    pub color: Option<String>,
}

impl NewMarker {
    /// A marker of `kind` on `item`. `viewer` is who is adding it, and owns it when it is a
    /// bookmark; the work's markers have no owner, whoever adds them.
    ///
    /// # Errors
    ///
    /// Refuses a marker on a series or a season, which nothing plays, and values that break a
    /// rule of [`MarkerError`].
    pub fn new(
        item: &MetadataItem,
        kind: MarkerKind,
        viewer: Option<UserId>,
        values: MarkerValues,
    ) -> Result<Self, MarkerError> {
        if !item.kind.is_playable() {
            return Err(MarkerError::NotPlayable);
        }
        let owner = if kind.is_personal() {
            Some(viewer.ok_or(MarkerError::NoViewer)?)
        } else {
            None
        };
        let values = values.checked(kind)?;
        Ok(Self {
            item: item.id,
            kind,
            owner,
            values,
        })
    }

    pub const fn item(&self) -> RatingKey {
        self.item
    }

    pub const fn kind(&self) -> MarkerKind {
        self.kind
    }

    pub const fn owner(&self) -> Option<UserId> {
        self.owner
    }

    pub const fn values(&self) -> &MarkerValues {
        &self.values
    }

    /// Whether `marker` is this one already: the same kind, owner and span on the same item.
    ///
    /// The reference answers an add of a marker already present with that marker rather than a
    /// second copy, leaving its title as it was; measured on bookmarks, by span — a start alone
    /// and the same start with an end are two markers.
    pub fn duplicates(&self, marker: &Marker) -> bool {
        marker.item == self.item
            && marker.kind == self.kind
            && marker.owner == self.owner
            && marker.start_ms == self.values.start_ms
            && marker.end_ms == self.values.end_ms
    }
}

impl MarkerValues {
    /// The values as they will be stored, or why they cannot be.
    ///
    /// An empty title or colour is no title or colour: the reference stores `''` and publishes
    /// nothing for it.
    fn checked(self, kind: MarkerKind) -> Result<Self, MarkerError> {
        let title = self.title.filter(|title| !title.is_empty());
        let color = self.color.filter(|color| !color.is_empty());
        if self.start_ms < 0 {
            return Err(MarkerError::NegativeStart);
        }
        match self.end_ms {
            // Measured on bookmarks: an end equal to the start is refused like one before it.
            Some(end) if end <= self.start_ms => return Err(MarkerError::EmptySpan),
            None if !kind.is_personal() => return Err(MarkerError::MissingEnd),
            _ => {}
        }
        if self.is_final && kind != MarkerKind::Credits {
            return Err(MarkerError::FinalNotCredits);
        }
        if !kind.is_personal() && (title.is_some() || color.is_some()) {
            return Err(MarkerError::NotPersonal);
        }
        Ok(Self {
            start_ms: self.start_ms,
            end_ms: self.end_ms,
            is_final: self.is_final,
            title,
            color,
        })
    }
}

impl Marker {
    /// The values a marker has now, to start an edit from.
    pub fn values(&self) -> MarkerValues {
        MarkerValues {
            start_ms: self.start_ms,
            end_ms: self.end_ms,
            is_final: self.is_final,
            title: self.title.clone(),
            color: self.color.clone(),
        }
    }

    /// This marker with new values, checked as an added one is.
    ///
    /// A marker edited by hand is no longer the detection's, so it loses its version.
    ///
    /// # Errors
    ///
    /// Values that break a rule of [`MarkerError`].
    pub fn edited(&self, values: MarkerValues) -> Result<Self, MarkerError> {
        let values = values.checked(self.kind)?;
        Ok(Self {
            start_ms: values.start_ms,
            end_ms: values.end_ms,
            is_final: values.is_final,
            version: None,
            title: values.title,
            color: values.color,
            ..self.clone()
        })
    }

    /// Whether `viewer` sees this marker: the work's are everyone's, a bookmark its owner's.
    pub fn is_visible_to(&self, viewer: Option<UserId>) -> bool {
        self.owner.is_none() || self.owner == viewer
    }
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;
    use crate::metadata::ItemKind;
    use crate::{SectionId, UserId};

    fn item(kind: ItemKind) -> MetadataItem {
        MetadataItem {
            id: RatingKey::new(7),
            section_id: SectionId::new(27),
            parent_id: None,
            kind,
            guid: "tv.plex.agents.none://7".to_owned(),
            title: Some("Soul".to_owned()),
            title_sort: None,
            original_title: None,
            summary: None,
            tagline: None,
            studio: None,
            content_rating: None,
            content_rating_age: None,
            position: None,
            year: None,
            released_on: None,
            duration_ms: None,
            rating: None,
            audience_rating: None,
            added_at: 0,
            updated_at: 0,
            refreshed_at: None,
            chapter_source: None,
        }
    }

    fn span(start_ms: i64, end_ms: Option<i64>) -> MarkerValues {
        MarkerValues {
            start_ms,
            end_ms,
            is_final: false,
            title: None,
            color: None,
        }
    }

    const VIEWER: UserId = UserId::new(3);

    #[test]
    fn the_works_markers_have_no_owner_and_a_bookmark_its_viewers() {
        let intro = NewMarker::new(
            &item(ItemKind::Episode),
            MarkerKind::Intro,
            Some(VIEWER),
            span(1_000, Some(30_000)),
        )
        .unwrap();
        assert_eq!(intro.owner(), None);

        let bookmark = NewMarker::new(
            &item(ItemKind::Movie),
            MarkerKind::Bookmark,
            Some(VIEWER),
            span(1_000, None),
        )
        .unwrap();
        assert_eq!(bookmark.owner(), Some(VIEWER));
    }

    #[test]
    fn a_bookmark_needs_a_viewer_to_belong_to() {
        assert_eq!(
            NewMarker::new(
                &item(ItemKind::Movie),
                MarkerKind::Bookmark,
                None,
                span(0, None)
            ),
            Err(MarkerError::NoViewer)
        );
    }

    #[test]
    fn nothing_that_does_not_play_carries_a_marker() {
        for kind in [ItemKind::Show, ItemKind::Season] {
            assert_eq!(
                NewMarker::new(&item(kind), MarkerKind::Intro, None, span(0, Some(10))),
                Err(MarkerError::NotPlayable)
            );
        }
    }

    #[test]
    fn a_span_must_start_at_or_after_zero_and_end_after_it_starts() {
        let episode = item(ItemKind::Episode);
        let check = |values| NewMarker::new(&episode, MarkerKind::Intro, None, values);
        assert_eq!(check(span(-5, Some(10))), Err(MarkerError::NegativeStart));
        assert_eq!(check(span(50, Some(10))), Err(MarkerError::EmptySpan));
        assert_eq!(check(span(50, Some(50))), Err(MarkerError::EmptySpan));
        assert_eq!(check(span(50, None)), Err(MarkerError::MissingEnd));
        assert!(check(span(0, Some(1))).is_ok());
    }

    #[test]
    fn only_credits_are_final_and_only_a_bookmark_is_named() {
        let episode = item(ItemKind::Episode);
        let final_intro = MarkerValues {
            is_final: true,
            ..span(0, Some(10))
        };
        assert_eq!(
            NewMarker::new(&episode, MarkerKind::Intro, None, final_intro.clone()),
            Err(MarkerError::FinalNotCredits)
        );
        assert!(NewMarker::new(&episode, MarkerKind::Credits, None, final_intro).is_ok());

        let named = MarkerValues {
            title: Some("Here".to_owned()),
            ..span(0, Some(10))
        };
        assert_eq!(
            NewMarker::new(&episode, MarkerKind::Commercial, None, named.clone()),
            Err(MarkerError::NotPersonal)
        );
        assert!(NewMarker::new(&episode, MarkerKind::Bookmark, Some(VIEWER), named).is_ok());
    }

    #[test]
    fn an_empty_title_is_no_title() {
        let bookmark = NewMarker::new(
            &item(ItemKind::Movie),
            MarkerKind::Bookmark,
            Some(VIEWER),
            MarkerValues {
                title: Some(String::new()),
                color: Some(String::new()),
                ..span(0, None)
            },
        )
        .unwrap();
        assert_eq!(bookmark.values().title, None);
        assert_eq!(bookmark.values().color, None);
    }

    #[test]
    fn an_identical_add_is_the_marker_already_there() {
        let bookmark = NewMarker::new(
            &item(ItemKind::Movie),
            MarkerKind::Bookmark,
            Some(VIEWER),
            span(10_000, None),
        )
        .unwrap();
        let stored = Marker {
            id: MarkerId::new(5),
            item: RatingKey::new(7),
            kind: MarkerKind::Bookmark,
            owner: Some(VIEWER),
            start_ms: 10_000,
            end_ms: None,
            is_final: false,
            version: None,
            title: Some("Kept".to_owned()),
            color: None,
        };
        assert!(bookmark.duplicates(&stored));
        assert!(!bookmark.duplicates(&Marker {
            end_ms: Some(20_000),
            ..stored.clone()
        }));
        assert!(!bookmark.duplicates(&Marker {
            owner: Some(UserId::new(1)),
            ..stored
        }));
    }

    #[test]
    fn an_edited_marker_is_checked_and_no_longer_the_detections() {
        let detected = Marker {
            id: MarkerId::new(1),
            item: RatingKey::new(7),
            kind: MarkerKind::Credits,
            owner: None,
            start_ms: 100,
            end_ms: Some(200),
            is_final: true,
            version: Some(4),
            title: None,
            color: None,
        };
        let moved = detected
            .edited(MarkerValues {
                start_ms: 150,
                ..detected.values()
            })
            .unwrap();
        assert_eq!((moved.start_ms, moved.version), (150, None));
        assert_eq!(
            detected.edited(MarkerValues {
                end_ms: None,
                ..detected.values()
            }),
            Err(MarkerError::MissingEnd)
        );
    }

    #[test]
    fn a_bookmark_is_seen_by_its_owner_alone() {
        let bookmark = Marker {
            id: MarkerId::new(1),
            item: RatingKey::new(7),
            kind: MarkerKind::Bookmark,
            owner: Some(VIEWER),
            start_ms: 0,
            end_ms: None,
            is_final: false,
            version: None,
            title: None,
            color: None,
        };
        assert!(bookmark.is_visible_to(Some(VIEWER)));
        assert!(!bookmark.is_visible_to(Some(UserId::new(1))));
        assert!(!bookmark.is_visible_to(None));
        let intro = Marker {
            kind: MarkerKind::Intro,
            owner: None,
            ..bookmark
        };
        assert!(intro.is_visible_to(None));
    }
}
