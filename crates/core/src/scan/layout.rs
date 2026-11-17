//! How file and directory names become items: the reference's movie and series scanners, as
//! measured in `docs/scanner/discovery.md`. Pure functions over names; nothing here reads a
//! file.

use crate::files::RelativePath;
use crate::scan::ShowKey;

/// What a file found by the walk is to the scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileClass {
    /// A video file the scan groups into items.
    Media,
    /// A subtitle file, attached to the media file it is named after.
    Subtitle,
    /// Anything else: images, samples, metadata files, unknown extensions.
    Ignored,
}

/// One item a directory's files make, and which of them make it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grouped {
    pub kind: GroupedKind,
    /// Each version's files, by index into the `files` given, in part order: one version per
    /// copy of the work, several files in one version when they are stacked (`cd1`, `cd2`).
    pub versions: Vec<Vec<usize>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupedKind {
    Movie {
        title: String,
        year: Option<i32>,
    },
    Episode {
        show: ShowKey,
        season: i32,
        episode: i32,
        /// The last of a multi-episode file's numbers (`S01E01-E03` gives 3).
        last_episode: Option<i32>,
        /// The air date's midnight UTC, for an episode named by its date.
        released_on: Option<i64>,
    },
}

/// A subtitle file attached to a media file, and what its name says about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarMatch {
    /// Index into the `siblings` given.
    pub index: usize,
    pub language: Option<String>,
    pub forced: bool,
    pub hearing_impaired: bool,
}

pub fn classify(_name: &str) -> FileClass {
    FileClass::Ignored
}

/// The films among the media files `files` of `directory`.
pub fn group_movies(_directory: &RelativePath, _files: &[&str]) -> Vec<Grouped> {
    Vec::new()
}

/// The episodes among the media files `files` of `directory`, a path below a series library's
/// root whose first name is the show.
pub fn group_episodes(_directory: &RelativePath, _files: &[&str]) -> Vec<Grouped> {
    Vec::new()
}

/// The subtitle files among `siblings` that belong to the media file `media`.
pub fn sidecars(_media: &str, _siblings: &[&str]) -> Vec<SidecarMatch> {
    Vec::new()
}

/// The rules of a `.plexignore`.
#[derive(Debug, Clone, Default)]
pub struct IgnoreRules {}

impl IgnoreRules {
    pub fn parse(_text: &str) -> Self {
        Self::default()
    }

    /// Whether `path`, relative to the directory holding the `.plexignore`, is ignored.
    pub fn ignores(&self, _path: &RelativePath) -> bool {
        false
    }
}
