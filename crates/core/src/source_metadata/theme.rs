//! A series' theme music, found where the reference finds it: a `theme.mp3` beside the
//! series' episodes (its `LocalMedia` agent), else the theme Plex's service keeps for the
//! series' TVDB id, `https://tvthemes.plexapp.com/{tvdb}.mp3`.
//!
//! Either is copied into the server's store, as the reference copies a local asset into its
//! metadata bundle: the theme is then served from there, by the item's id, and no route ever
//! serves a library file by its path.

use std::sync::Arc;

use super::artwork::ArtworkError;
use super::SourceRecord;
use crate::files::{LibraryFiles, RelativePath};
use crate::metadata::ItemKind;
use crate::RatingKey;

/// Plex's theme service, which names a theme by the series' TVDB id.
pub const THEME_SERVICE: &str = "https://tvthemes.plexapp.com";

/// A local theme's extensions. The reference reads `mp3` alone; the other formats browsers
/// play are taken too, the repo owner's decision.
const THEME_EXTENSIONS: [&str; 8] = ["mp3", "m4a", "aac", "flac", "ogg", "oga", "opus", "wav"];

/// The most a theme is read up to: a song of a few minutes weighs a few megabytes, even as
/// FLAC; a file past this is not a theme, and is not read into memory.
pub const MAX_THEME_BYTES: u64 = 48 * 1024 * 1024;

/// Where a theme came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeSource {
    /// A `theme.*` file beside the series' episodes.
    Local,
    /// Plex's theme service.
    Provider,
}

impl ThemeSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Provider => "provider",
        }
    }

    pub fn parse(stored: &str) -> Option<Self> {
        [Self::Local, Self::Provider]
            .into_iter()
            .find(|source| source.as_str() == stored)
    }
}

/// A theme kept in the server's store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredTheme {
    /// Where the store keeps it, a name it gave out: `themes/ab/abcd….mp3`.
    pub file: String,
    pub content_type: String,
    pub size_bytes: u64,
}

/// Keeps themes: fetched from the theme service once, or copied from a library.
#[async_trait::async_trait]
pub trait ThemeStore: Send + Sync + std::fmt::Debug {
    /// The theme at `url`, from the store when it is there, else downloaded; `None` when the
    /// service holds none for it.
    async fn fetch(&self, url: &str) -> Result<Option<StoredTheme>, ArtworkError>;

    /// `bytes`, a theme read from a library file of this extension, kept under a name of their
    /// content's.
    async fn keep(&self, bytes: Vec<u8>, extension: &str) -> Result<StoredTheme, ArtworkError>;
}

/// What finding themes needs: where they are kept, and how a library's files are read.
#[derive(Debug, Clone)]
pub struct Themes {
    pub store: Arc<dyn ThemeStore>,
    pub files: Arc<dyn LibraryFiles>,
}

/// Where a series' episodes lie: its library's roots and the paths its parts record.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SeriesFiles {
    pub roots: Vec<String>,
    pub parts: Vec<String>,
}

/// A theme an item is given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemTheme {
    pub item: RatingKey,
    pub source: ThemeSource,
    /// What it was copied from: the service's address, or the path the library file records.
    pub origin: String,
    pub stored: StoredTheme,
}

/// A theme an item has, as clients are told of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShownTheme {
    pub item: RatingKey,
    pub source: ThemeSource,
    pub file: String,
    pub content_type: String,
    pub size_bytes: u64,
    /// When the item took it.
    pub chosen_at: i64,
}

/// The extension of a name that is a local theme, by `LocalMedia`'s `theme-?[0-9]?` on its
/// stem: `theme.mp3`, `Theme-2.mp3`, `theme1.flac`.
pub fn theme_extension(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    let (stem, extension) = lower.rsplit_once('.')?;
    let rest = stem.strip_prefix("theme")?;
    let rest = rest.strip_prefix('-').unwrap_or(rest);
    let numbered = rest.is_empty() || (rest.len() == 1 && rest.as_bytes()[0].is_ascii_digit());
    if !numbered {
        return None;
    }
    THEME_EXTENSIONS
        .iter()
        .find(|known| **known == extension)
        .copied()
}

/// The first local theme among `names`, in the reference's order: by stem, so `theme.mp3`
/// comes before `theme-1.mp3`.
pub fn first_theme<'a>(names: &[&'a str]) -> Option<&'a str> {
    names
        .iter()
        .copied()
        .filter(|name| theme_extension(name).is_some())
        .min_by_key(|name| {
            let lower = name.to_ascii_lowercase();
            let stem = lower
                .rsplit_once('.')
                .map_or(lower.as_str(), |(stem, _)| stem);
            (stem.to_owned(), lower.clone())
        })
}

/// The directories a series' local theme is looked for in, by root: each episode's own, the
/// one above it and the one above that, as the reference's `FindUniqueSubdirs` gathers them —
/// a library's root aside, whose files are no one series'. Shallowest first, so the series'
/// own folder wins over a season's.
pub fn theme_directories(
    files: &dyn LibraryFiles,
    series: &SeriesFiles,
) -> Vec<(String, RelativePath)> {
    let mut directories: Vec<(String, RelativePath)> = Vec::new();
    for part in &series.parts {
        let Some((root, path)) = series
            .roots
            .iter()
            .find_map(|root| Some((root, files.relative_path(root, part)?)))
        else {
            continue;
        };
        let mut directory = path.parent();
        for _ in 0..3 {
            if directory.is_root() {
                break;
            }
            let entry = (root.clone(), directory.clone());
            if !directories.contains(&entry) {
                directories.push(entry);
            }
            directory = directory.parent();
        }
    }
    directories.sort_by(|(a_root, a), (b_root, b)| {
        (a.components().count(), a_root, a).cmp(&(b.components().count(), b_root, b))
    });
    directories
}

/// The address of the theme Plex's service keeps for a series record, by its TVDB id.
pub fn provider_theme_url(record: &SourceRecord) -> Option<String> {
    if record.kind != ItemKind::Show {
        return None;
    }
    let tvdb = if record.agent == "tvdb" {
        Some(record.provider_id.as_str())
    } else {
        record
            .external_ids
            .iter()
            .find(|id| id.source == "tvdb")
            .map(|id| id.value.as_str())
    }?;
    let numeric = !tvdb.is_empty() && tvdb.bytes().all(|byte| byte.is_ascii_digit());
    numeric.then(|| format!("{THEME_SERVICE}/{tvdb}.mp3"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::source_metadata::ExternalId;

    #[test]
    fn a_theme_is_named_as_local_media_names_it() {
        for name in [
            "theme.mp3",
            "Theme.MP3",
            "theme-1.mp3",
            "theme2.flac",
            "theme.m4a",
        ] {
            assert!(theme_extension(name).is_some(), "{name}");
        }
        for name in [
            "theme",
            "themes.mp3",
            "theme-12.mp3",
            "theme-.mp3x",
            "my theme.mp3",
            "theme.mkv",
            "theme.jpg",
        ] {
            assert!(theme_extension(name).is_none(), "{name}");
        }
        assert_eq!(theme_extension("Theme.OGG"), Some("ogg"));
    }

    #[test]
    fn the_first_theme_is_the_unnumbered_one() {
        let names = ["theme-1.mp3", "poster.jpg", "Theme.mp3", "S01E01.mkv"];
        assert_eq!(first_theme(&names), Some("Theme.mp3"));
        assert_eq!(first_theme(&["poster.jpg"]), None);
    }

    #[test]
    fn the_service_is_asked_by_the_series_tvdb_id() {
        let mut record = SourceRecord {
            kind: ItemKind::Show,
            agent: "tvdb".to_owned(),
            provider_id: "71663".to_owned(),
            position: None,
            original_language: None,
            original_title: None,
            translations: Vec::new(),
            year: None,
            released_on: None,
            duration_ms: None,
            studio: None,
            rating: None,
            certifications: Vec::new(),
            external_ids: Vec::new(),
            images: Vec::new(),
            genres: Vec::new(),
            countries: Vec::new(),
            credits: Vec::new(),
            ratings: Vec::new(),
            children: Vec::new(),
        };
        assert_eq!(
            provider_theme_url(&record).as_deref(),
            Some("https://tvthemes.plexapp.com/71663.mp3")
        );
        record.agent = "tmdb".to_owned();
        record.provider_id = "456".to_owned();
        assert_eq!(provider_theme_url(&record), None);
        record.external_ids.push(ExternalId {
            source: "tvdb".to_owned(),
            value: "71663".to_owned(),
        });
        assert!(provider_theme_url(&record).is_some());
        record.kind = ItemKind::Movie;
        assert_eq!(provider_theme_url(&record), None);
    }
}
