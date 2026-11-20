//! Chapters: the named sections of a film or an episode a player lists and jumps between.
//!
//! They belong to the item, one set whatever its versions, as on the reference, which reads them
//! from the file it analyzed or from its metadata agent. Offsets are in milliseconds from the
//! start of the media.

use crate::{ChapterId, MediaId, RatingKey};

/// Where an item's chapters came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChapterSource {
    /// Read from the file.
    Media,
    /// Supplied by the reference's metadata agent, in whole seconds.
    Agent,
    /// The agent's, over the file's.
    Mixed,
}

impl ChapterSource {
    /// The name the reference stores and publishes.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Media => "media",
            Self::Agent => "agent",
            Self::Mixed => "mixed",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "media" => Some(Self::Media),
            "agent" => Some(Self::Agent),
            "mixed" => Some(Self::Mixed),
            _ => None,
        }
    }
}

/// One chapter of one item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chapter {
    pub id: ChapterId,
    pub item: RatingKey,
    /// From 1, in the media's order.
    pub index: u32,
    pub start_ms: i64,
    pub end_ms: i64,
    /// `None` when the file names none, or only numbers them.
    pub title: Option<String>,
    /// The version whose frame the reference drew as the chapter's image, when it drew one.
    pub image_version: Option<MediaId>,
}
