//! How file and directory names become items: the reference's movie and series scanners, as
//! measured in `docs/plex/scanner/discovery.md`. Pure functions over names; nothing here reads a
//! file.

mod clean;
mod episode;
mod ignore;
mod sidecar;
mod stack;

use crate::files::RelativePath;
use crate::language::LanguageTag;
use crate::scan::ShowKey;

pub use ignore::IgnoreRules;

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
    pub language: Option<LanguageTag>,
    pub forced: bool,
    pub hearing_impaired: bool,
}

/// The extras a name's suffix makes (`Film-trailer2`): left to the extras step, which is not
/// written yet, and never an item.
const EXTRA_SUFFIXES: [&str; 8] = [
    "trailer",
    "deleted",
    "behindthescenes",
    "interview",
    "scene",
    "featurette",
    "short",
    "other",
];

/// The stem and the extension, which a name without a dot (or only a leading one) lacks.
fn split_extension(name: &str) -> (&str, &str) {
    match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, extension),
        _ => (name, ""),
    }
}

pub fn classify(name: &str) -> FileClass {
    if name.starts_with('.') {
        return FileClass::Ignored;
    }
    let (stem, extension) = split_extension(name);
    let extension = extension.to_ascii_lowercase();
    if sidecar::SUBTITLE_EXTENSIONS.contains(&extension.as_str()) {
        return FileClass::Subtitle;
    }
    let video = clean::VIDEO_EXTENSIONS.contains(&extension.as_str())
        && extension != "vob"
        && extension != "bup";
    if !video || is_sample(name) || is_extra(stem) {
        return FileClass::Ignored;
    }
    FileClass::Media
}

/// `[-._]sample` or `sample[-._]`. The reference spares a sample of 300 MB or more; a name says
/// nothing of size, so every one is left out.
fn is_sample(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.match_indices("sample").any(|(at, _)| {
        let before = at.checked_sub(1).map(|index| lower.as_bytes()[index]);
        let after = lower.as_bytes().get(at + "sample".len()).copied();
        matches!(before, Some(b'-' | b'.' | b'_')) || matches!(after, Some(b'-' | b'.' | b'_'))
    })
}

/// `-(trailer|…)[-_]?[0-9]*$` on the stem, `trailer` alone, or `movie-trailer…`.
fn is_extra(stem: &str) -> bool {
    let lower = stem.to_ascii_lowercase();
    let rest = lower.trim_end_matches(|c: char| c.is_ascii_digit());
    let rest = rest.strip_suffix(['-', '_']).unwrap_or(rest);
    let head = rest.trim_end_matches(|c: char| c.is_ascii_lowercase());
    let suffixed = head.ends_with('-') && EXTRA_SUFFIXES.contains(&&rest[head.len()..]);
    let alone = lower
        .strip_prefix("trailer")
        .is_some_and(|rest| rest.chars().count() <= 1);
    suffixed || alone || lower.starts_with("movie-trailer")
}

/// Whether the walk leaves the directory `name` unvisited, as the reference does; `at_root`
/// for a directory directly below a library root, where `Extras` may be a show.
pub fn skips_directory(name: &str, at_root: bool) -> bool {
    let lower = name.to_lowercase();
    let everywhere = lower.starts_with('.')
        || lower.contains("_unpack_")
        || lower.contains("_failed_")
        || lower
            .rsplit_once('.')
            .is_some_and(|(_, kind)| matches!(kind, "itlp" | "photoslibrary"))
        || is_synology_index(&lower)
        || matches!(
            lower.as_str(),
            "lost+found"
                | "@recycle"
                | "#recycle"
                | "@recently-snapshot"
                | "#snapshot"
                | "#snapshots"
        );
    if everywhere {
        return true;
    }
    if at_root {
        return matches!(
            lower.as_str(),
            "$recycle.bin"
                | "system volume information"
                | "temporary items"
                | "network trash folder"
        );
    }
    // `video_ts` is on the legacy list, but the reference entered it (it holds nothing kept).
    lower.contains("bonus disc")
        || matches!(
            lower.as_str(),
            "extra" | "extras" | "sample" | "samples" | "!sample" | "!samples" | "bonus" | "bdmv"
        )
        || is_extras_folder(&lower)
}

/// `@eaDir`, or a copy of one (`@eaDir (copy 2)`).
fn is_synology_index(lower: &str) -> bool {
    lower.strip_prefix("@eadir").is_some_and(|rest| {
        rest.is_empty()
            || rest
                .strip_prefix(':')
                .unwrap_or(rest)
                .trim_start()
                .strip_prefix("(copy ")
                .and_then(|copy| copy.strip_suffix(')'))
                .is_some_and(|number| {
                    !number.is_empty() && number.chars().all(|c| c.is_ascii_digit())
                })
    })
}

/// The legacy `^interview.?$`, `^deleted.?(scene.?)?$`, `^behind.?the.?scenes$` and their
/// kin, where `.?` admits a plural or a space.
fn is_extras_folder(lower: &str) -> bool {
    let at_most_one = |rest: &str| rest.chars().count() <= 1;
    // `.?word`: `word`, after one character or none.
    let then = |rest: &'_ str, word: &str| -> Option<usize> {
        if rest.starts_with(word) {
            return Some(word.len());
        }
        let skipped = rest.chars().next()?.len_utf8();
        rest[skipped..]
            .starts_with(word)
            .then_some(skipped + word.len())
    };
    let deleted = |rest: &str| {
        at_most_one(rest) || then(rest, "scene").is_some_and(|length| at_most_one(&rest[length..]))
    };
    let behind = |rest: &str| {
        then(rest, "the").is_some_and(|length| {
            let rest = &rest[length..];
            then(rest, "scenes").is_some_and(|length| length == rest.len())
        })
    };
    [
        "interview",
        "scene",
        "trailer",
        "featurette",
        "short",
        "other",
    ]
    .iter()
    .any(|word| lower.strip_prefix(word).is_some_and(at_most_one))
        || lower.strip_prefix("deleted").is_some_and(deleted)
        || lower.strip_prefix("behind").is_some_and(behind)
}

/// Whether any directory on the way to `directory` is one the walk does not enter.
fn below_skipped(directory: &RelativePath) -> bool {
    directory
        .components()
        .enumerate()
        .any(|(depth, name)| skips_directory(name, depth == 0))
}

/// `files`' media files, in name order.
fn sorted_media(files: &[&str]) -> Vec<usize> {
    let mut sorted: Vec<usize> = (0..files.len())
        .filter(|index| classify(files[*index]) == FileClass::Media)
        .collect();
    sorted.sort_by(|a, b| files[*a].cmp(files[*b]));
    sorted
}

/// Merges `(key, files)` pairs into one entry per key, in order of first appearance.
fn merge_versions<K: PartialEq>(
    units: impl IntoIterator<Item = (K, Vec<usize>)>,
) -> Vec<(K, Vec<Vec<usize>>)> {
    let mut merged: Vec<(K, Vec<Vec<usize>>)> = Vec::new();
    for (key, parts) in units {
        match merged.iter_mut().find(|(existing, _)| *existing == key) {
            Some((_, versions)) => versions.push(parts),
            None => merged.push((key, vec![parts])),
        }
    }
    merged
}

/// `cd[\s]*[0-9]+` at the start of a folder's name: one part of a film split across folders.
fn is_disc_folder(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower
        .strip_prefix("cd")
        .map(str::trim_start)
        .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
}

/// `{edition-…}` or `[edition-…]`, which keeps two copies of a title apart.
fn edition(name: &str) -> Option<String> {
    let lower = name.to_lowercase();
    lower.match_indices("edition-").find_map(|(at, _)| {
        let opens = at
            .checked_sub(1)
            .and_then(|index| lower.as_bytes().get(index))
            .is_some_and(|c| matches!(c, b'{' | b'['));
        let rest = &lower[at + "edition-".len()..];
        let end = rest.find(['}', ']']).filter(|end| *end > 0)?;
        opens.then(|| rest[..end].to_owned())
    })
}

/// The films among the media files `files` of `directory`.
///
/// Copies of one film in one directory — the same cleaned title, year and edition — are one
/// item with several versions: the reference leaves that to its agent's match, which this
/// server does not have (§4.1, §7). A folder named `CD1`, `CD 2`… is one part of a film titled
/// by the folder above it, so the caller merges the parts it finds under that title.
pub fn group_movies(directory: &RelativePath, files: &[&str]) -> Vec<Grouped> {
    if below_skipped(directory) {
        return Vec::new();
    }
    let year = clean::current_year();
    let sorted: Vec<usize> = sorted_media(files)
        .into_iter()
        .filter(|index| !looks_like_episode(files[*index]))
        .collect();

    let folder = directory.name();
    let parent = directory.parent();
    if let Some(parent_name) = parent.name().filter(|_| folder.is_some_and(is_disc_folder)) {
        if sorted.is_empty() {
            return Vec::new();
        }
        let (title, year) = clean::clean_name(parent_name, year);
        return vec![Grouped {
            kind: GroupedKind::Movie { title, year },
            versions: vec![sorted],
        }];
    }

    let units = stack::stacks(files, &sorted, true)
        .into_iter()
        .map(|stack| {
            let name = stack
                .root
                .unwrap_or_else(|| split_extension(files[stack.parts[0]]).0.to_owned());
            let (title, found_year) = clean::clean_name(&name, year);
            ((title, found_year, edition(&name)), stack.parts)
        });
    let mut grouped: Vec<Grouped> = merge_versions(units)
        .into_iter()
        .map(|((title, year, _), versions)| Grouped {
            kind: GroupedKind::Movie { title, year },
            versions,
        })
        .collect();

    // A folder of one film names it, even when the file has a year and the folder none (§4.1).
    if let ([only], Some(folder)) = (grouped.as_mut_slice(), folder) {
        let (title, folder_year) = clean::clean_name(folder, year);
        if title.chars().count() > 1 || folder_year.is_some() {
            only.kind = GroupedKind::Movie {
                title,
                year: folder_year,
            };
        }
    }
    grouped
}

/// The reference's movie scanner leaves a file an episode pattern reads (`SxxEyy`, `Sxx-yy`,
/// `NxNN`) to a series library.
fn looks_like_episode(name: &str) -> bool {
    episode::names_season(name.to_ascii_lowercase().as_bytes())
}

/// What the season folder a file sits in says: the last number of its name (`Season 5 -
/// Name`, `Disc 2`), 0 for `Specials`. A file at its show's root has none.
fn folder_season(directory: &RelativePath) -> Option<i32> {
    if directory.components().count() < 2 {
        return None;
    }
    let name = directory.name()?;
    let through_last = name.trim_end_matches(|c: char| !c.is_ascii_digit());
    let digits = &through_last[through_last
        .trim_end_matches(|c: char| c.is_ascii_digit())
        .len()..];
    if digits.is_empty() {
        // Not measured with numbered files; season 0 keeps them from colliding with season 1.
        return name.eq_ignore_ascii_case("specials").then_some(0);
    }
    digits.parse().ok()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EpisodeKey {
    show: ShowKey,
    season: i32,
    episode: i32,
    last_episode: Option<i32>,
    released_on: Option<i64>,
}

/// The episodes among the media files `files` of `directory`, a path below a series library's
/// root whose first name is the show.
pub fn group_episodes(directory: &RelativePath, files: &[&str]) -> Vec<Grouped> {
    if below_skipped(directory) {
        return Vec::new();
    }
    let year = clean::current_year();
    let sorted = sorted_media(files);
    let show = directory.components().next().map(|folder| {
        let (title, year) = clean::clean_name(folder, year);
        ShowKey { title, year }
    });
    let season = folder_season(directory);

    let units = stack::stacks(files, &sorted, false)
        .into_iter()
        .filter_map(|stack| {
            let name = files[stack.parts[0]];
            let key = match &show {
                Some(show) => in_show(name, show, season, year),
                None => at_root(name, year),
            }?;
            Some((key, stack.parts))
        });
    merge_versions(units)
        .into_iter()
        .map(|(key, versions)| Grouped {
            kind: GroupedKind::Episode {
                show: key.show,
                season: key.season,
                episode: key.episode,
                last_episode: key.last_episode,
                released_on: key.released_on,
            },
            versions,
        })
        .collect()
}

/// A file directly at the root names its show before its episode (`Show.S07E03.mkv`); only
/// the patterns that give a season apply there.
fn at_root(name: &str, current_year: i32) -> Option<EpisodeKey> {
    let (mut show_end, numbers) =
        episode::season_and_episode(name.to_ascii_lowercase().as_bytes())?;
    // The patterns step through bytes, and may stop inside a character before the numbers.
    while !name.is_char_boundary(show_end) {
        show_end -= 1;
    }
    let (title, year) = clean::clean_name(&name[..show_end], current_year);
    if title.is_empty() {
        return None;
    }
    Some(EpisodeKey {
        show: ShowKey { title, year },
        season: numbers.season.unwrap_or(1),
        episode: numbers.episode,
        last_episode: numbers.last,
        released_on: None,
    })
}

fn in_show(
    name: &str,
    show: &ShowKey,
    folder_season: Option<i32>,
    current_year: i32,
) -> Option<EpisodeKey> {
    let (stem, extension) = split_extension(name);
    let text = episode::pattern_text(stem, &show.title, current_year);
    let key = |season: i32, numbers: episode::Numbers| EpisodeKey {
        show: show.clone(),
        season,
        episode: numbers.episode,
        last_episode: numbers.last,
        released_on: None,
    };

    let with_extension = format!("{text}.{}", extension.to_lowercase());
    if let Some((_, numbers)) = episode::season_and_episode(with_extension.as_bytes()) {
        return Some(key(numbers.season.unwrap_or(1), numbers));
    }
    if episode::is_anime_extra(stem) {
        return None;
    }
    if let Some(date) = episode::air_date(stem.to_lowercase().as_bytes()) {
        // The reference numbers a dated episode, and its season, -1.
        return Some(EpisodeKey {
            show: show.clone(),
            season: -1,
            episode: -1,
            last_episode: None,
            released_on: Some(date.midnight()),
        });
    }
    let numbers = episode::episode_alone(text.as_bytes())?;
    Some(key(folder_season.unwrap_or(1), numbers))
}

/// The subtitle files among `siblings` that belong to the media file `media`.
pub fn sidecars(media: &str, siblings: &[&str]) -> Vec<SidecarMatch> {
    sidecar::sidecars(media, siblings)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn directory(path: &str) -> RelativePath {
        RelativePath::parse(path).unwrap()
    }

    fn movie(title: &str, year: Option<i32>, versions: &[&[usize]]) -> Grouped {
        Grouped {
            kind: GroupedKind::Movie {
                title: title.to_owned(),
                year,
            },
            versions: versions.iter().map(|parts| parts.to_vec()).collect(),
        }
    }

    fn movies(path: &str, files: &[&str]) -> Vec<Grouped> {
        group_movies(&directory(path), files)
    }

    /// An episode's show title, season, episode and last episode, with its versions.
    type Row = (String, i32, i32, Option<i32>, Vec<Vec<usize>>);

    fn episodes(path: &str, files: &[&str]) -> Vec<Row> {
        group_episodes(&directory(path), files)
            .into_iter()
            .map(|grouped| match grouped.kind {
                GroupedKind::Episode {
                    show,
                    season,
                    episode,
                    last_episode,
                    ..
                } => (show.title, season, episode, last_episode, grouped.versions),
                GroupedKind::Movie { .. } => unreachable!(),
            })
            .collect()
    }

    fn episode(show: &str, season: i32, number: i32) -> Row {
        (show.to_owned(), season, number, None, vec![vec![0]])
    }

    #[test]
    fn keeps_the_measured_video_files_and_drops_the_rest() {
        for name in [
            "a.mkv", "a.mp4", "a.avi", "a.ts", "a.m2ts", "a.webm", "a.wmv", "A.MKV",
        ] {
            assert_eq!(classify(name), FileClass::Media, "{name}");
        }
        for name in [
            "a.iso",
            "a.rar",
            "VIDEO_TS.IFO",
            "VTS_01_1.VOB",
            ".hidden.mkv",
            ".plexignore",
            "poster.jpg",
            "Inception.2010.sample.mkv",
            "Sample Film (2000)-sample.mkv",
            "foo-trailer2.mkv",
            "Film-trailer.mkv",
            "Film-deleted.mkv",
            "Film-behindthescenes.mkv",
            "Film-scene.mkv",
            "Film-short.mkv",
            "Film-other.mkv",
            "Film-featurette_1.mkv",
            "Film-interview-2.mkv",
            "trailer.mkv",
            "movie-trailer.mkv",
        ] {
            assert_eq!(classify(name), FileClass::Ignored, "{name}");
        }
        for name in ["X.en.srt", "X.vtt", "X.ko.smi", "X.idx", "X.sub"] {
            assert_eq!(classify(name), FileClass::Subtitle, "{name}");
        }
        for name in ["X.ass", "X.ssa", "X.txt"] {
            assert_eq!(classify(name), FileClass::Ignored, "{name}");
        }
    }

    #[test]
    fn leaves_the_measured_directories_unvisited() {
        for name in [
            "@eaDir",
            "@eaDir (copy 2)",
            ".hiddendir",
            "lost+found",
            "x_UNPACK_y",
            "#snapshot",
        ] {
            assert!(skips_directory(name, true), "{name}");
            assert!(skips_directory(name, false), "{name}");
        }
        for name in [
            "Extras",
            "Samples",
            "BDMV",
            "Trailers",
            "Deleted Scenes",
            "Behind The Scenes",
            "Featurettes",
            "Interviews",
            "Scenes",
            "Shorts",
            "Other",
            "Bonus",
            "Film Bonus Disc 2",
        ] {
            assert!(skips_directory(name, false), "{name}");
        }
        assert!(skips_directory("$RECYCLE.BIN", true));
        assert!(!skips_directory("Extras", true));
        for name in [
            "Specials",
            "VIDEO_TS",
            "Season 01",
            "Other People",
            "Shorter Films",
            "CD1",
        ] {
            assert!(!skips_directory(name, false), "{name}");
        }
    }

    #[test]
    fn names_a_single_film_after_its_folder() {
        assert_eq!(
            movies("Folder Name (2012)", &["random.release.name.mkv"]),
            [movie("Folder Name", Some(2012), &[&[0]])]
        );
        assert_eq!(
            movies("Nested (1990)/Sub", &["deeper.file.mkv"]),
            [movie("Sub", None, &[&[0]])]
        );
        assert_eq!(
            movies("Single In Folder", &["Totally Different (1999).mkv"]),
            [movie("Single in Folder", None, &[&[0]])]
        );
        assert_eq!(
            movies("Ign Parent", &["Kept (2000).mkv"]),
            [movie("Ign Parent", None, &[&[0]])]
        );
        assert_eq!(
            movies("KB2", &["Kill Bill Vol 2 (2004).mkv"]),
            [movie("KB2", None, &[&[0]])]
        );
    }

    #[test]
    fn titles_each_film_of_a_shared_folder_by_its_file() {
        assert_eq!(
            movies("Two Films", &["Film B (2002).mkv", "Film A (2001).mkv"]),
            [
                movie("Film A", Some(2001), &[&[1]]),
                movie("Film B", Some(2002), &[&[0]]),
            ]
        );
    }

    #[test]
    fn titles_a_film_at_the_root_by_its_file() {
        assert_eq!(
            movies("", &["zzRoot Movie Like (2001).mkv"]),
            [movie("ZzRoot Movie Like", Some(2001), &[&[0]])]
        );
    }

    #[test]
    fn makes_versions_of_copies_that_clean_alike() {
        assert_eq!(
            movies(
                "",
                &[
                    "Brazil (1985) - 720p.mkv",
                    "Brazil (1985) - 1080p.mkv",
                    "Heat (1995).mkv"
                ]
            ),
            [
                movie("Brazil", Some(1985), &[&[1], &[0]]),
                movie("Heat", Some(1995), &[&[2]]),
            ]
        );
        assert_eq!(
            movies(
                "Versions Film (2011)",
                &[
                    "Versions Film (2011) - 1080p.mkv",
                    "Versions Film (2011) - 4K.mkv"
                ]
            ),
            [movie("Versions Film", Some(2011), &[&[0], &[1]])]
        );
    }

    #[test]
    fn keeps_editions_apart() {
        let grouped = movies(
            "",
            &[
                "Amelie (2001) {edition-Director's Cut}.mkv",
                "Amelie (2001).mkv",
            ],
        );
        assert_eq!(
            grouped,
            [
                movie("Amelie", Some(2001), &[&[0]]),
                movie("Amelie", Some(2001), &[&[1]]),
            ]
        );
    }

    #[test]
    fn stacks_parts_into_one_version() {
        assert_eq!(
            movies("", &["Heat (1995) - pt2.mkv", "Heat (1995) - pt1.mkv"]),
            [movie("Heat", Some(1995), &[&[1, 0]])]
        );
        assert_eq!(
            movies(
                "",
                &["Twin (2001) - 1 of 2.mkv", "Twin (2001) - 2 of 2.mkv"]
            ),
            [movie("Twin", Some(2001), &[&[0, 1]])]
        );
        // Not stacked, as on the reference; one title, so two versions of one film here.
        assert_eq!(
            movies(
                "",
                &["Movie.Name.2014.Part.1.mkv", "Movie.Name.2014.Part.2.mkv"]
            ),
            [movie("Movie Name", Some(2014), &[&[0], &[1]])]
        );
        assert_eq!(
            movies(
                "",
                &["Kill Bill Vol 1 (2003).mkv", "Kill Bill Vol 2 (2004).mkv"]
            )
            .len(),
            2
        );
    }

    #[test]
    fn titles_a_disc_folder_by_the_folder_above() {
        assert_eq!(
            movies("Disc Film (2003)/CD1", &["a.mkv"]),
            [movie("Disc Film", Some(2003), &[&[0]])]
        );
        assert_eq!(
            movies("Disc Film (2003)/CD 2", &["b.mkv"]),
            [movie("Disc Film", Some(2003), &[&[0]])]
        );
    }

    #[test]
    fn leaves_episodes_extras_and_samples_out_of_films() {
        assert!(movies("", &["Show.Name.S01E01.mkv"]).is_empty());
        assert_eq!(
            movies(
                "Film (2000)",
                &[
                    "Film (2000).mkv",
                    "Film-trailer.mkv",
                    "baz-sample.mkv",
                    "Film.en.srt"
                ]
            ),
            [movie("Film", Some(2000), &[&[0]])]
        );
        assert!(movies("Film (2000)/Trailers", &["Teaser.mkv"]).is_empty());
        assert_eq!(
            movies("Film (2000)/Specials", &["Making of.mkv"]),
            [movie("Specials", None, &[&[0]])]
        );
    }

    #[test]
    fn reads_episodes_as_the_reference_did() {
        let measured: [(&str, &str, i32, i32); 13] = [
            (
                "zzShow Probe (2019)/Season 01",
                "zzShow Probe - S01E01 - Pilot.mkv",
                1,
                1,
            ),
            (
                "zzShow Probe (2019)/Season 01",
                "zzShow.Probe.1x04.720p.HDTV.x264.mkv",
                1,
                4,
            ),
            ("zzShow Probe (2019)/Season 01", "05 - Fifth.mkv", 1, 5),
            (
                "zzShow Probe (2019)/Season 01",
                "zzShow Probe 107.mkv",
                1,
                107,
            ),
            ("zzShow Probe (2019)/Season 2", "s02e01.mkv", 2, 1),
            ("zzShow Probe (2019)", "zzShow Probe - S03E01.mkv", 3, 1),
            ("zzShow Probe (2019)/Saison 4", "04x02.mkv", 4, 2),
            (
                "zzShow Probe (2019)/Season 2",
                "Other Name S05E09.mkv",
                5,
                9,
            ),
            ("zzShow Probe (2019)/Season 3", "Weak Probe 1x05.mkv", 1, 5),
            (
                "zzShow Probe (2019)/Season Two",
                "zzShow Probe - 03.mkv",
                1,
                3,
            ),
            ("zzShow Probe (2019)/Season 1/Disc 2", "02.mkv", 2, 2),
            (
                "zzShow Probe (2019)/Specials",
                "zzShow Probe S00E01.mkv",
                0,
                1,
            ),
            (
                "zzShow Probe (2019)/Series 3",
                "zzShow Probe - 04.mkv",
                3,
                4,
            ),
        ];
        for (path, file, season, number) in measured {
            assert_eq!(
                episodes(path, &[file]),
                [episode("ZzShow Probe", season, number)],
                "{path}/{file}"
            );
        }
    }

    #[test]
    fn reads_number_only_episodes_in_the_folder_season() {
        let measured: [(&str, &str, i32, i32); 7] = [
            ("Weak Probe", "Weak.Probe.302.mkv", 1, 302),
            ("Weak Probe/Season 3", "Weak.Probe.304.mkv", 3, 304),
            ("zzAnime Probe", "zzAnime Probe - 105.mkv", 1, 105),
            ("zzAnime Probe", "zzAnime Probe - 12 [1080p].mkv", 1, 12),
            ("zzAnime Probe", "zzAnime Probe - 13v2.mkv", 1, 13),
            ("zzAnime Probe", "zzAnime Probe - ep 14.mkv", 1, 14),
            ("zzAnime Probe", "zzAnime Probe - 15 of 26.mkv", 1, 15),
        ];
        for (path, file, season, number) in measured {
            let show = clean::clean_name(path.split('/').next().unwrap(), 2026).0;
            assert_eq!(
                episodes(path, &[file]),
                [episode(&show, season, number)],
                "{path}/{file}"
            );
        }
        assert_eq!(
            episodes("zzFlat Show", &["zzFlat Show 102.mkv"]),
            [episode("ZzFlat Show", 1, 102)]
        );
        assert_eq!(
            episodes("Daily Probe", &["Daily Probe S2021E10.mkv"]),
            [episode("Daily Probe", 2021, 10)]
        );
    }

    #[test]
    fn dates_an_episode_named_by_its_air_date() {
        let dated = |file: &str| {
            group_episodes(&directory("Daily Probe"), &[file])
                .into_iter()
                .map(|grouped| grouped.kind)
                .collect::<Vec<_>>()
        };
        let show = ShowKey {
            title: "Daily Probe".to_owned(),
            year: None,
        };
        assert_eq!(
            dated("Daily.Probe.2021-03-04.mkv"),
            [GroupedKind::Episode {
                show: show.clone(),
                season: -1,
                episode: -1,
                last_episode: None,
                released_on: Some(1_614_816_000),
            }]
        );
        assert_eq!(
            dated("Daily Probe 05.03.2021.mkv"),
            [GroupedKind::Episode {
                show,
                season: -1,
                episode: -1,
                last_episode: None,
                released_on: Some(1_620_000_000),
            }]
        );
    }

    #[test]
    fn drops_what_no_pattern_reads() {
        assert!(episodes("Show/Season 1", &["Episode 6.mkv"]).is_empty());
        assert!(episodes("Show/Specials", &["Behind it.mkv"]).is_empty());
        assert!(episodes("zzAnime Probe", &["zzAnime Probe - NCOP1.mkv"]).is_empty());
        assert!(episodes("Show/Season 1", &["Show - S01E01-trailer.mkv"]).is_empty());
    }

    #[test]
    fn takes_a_root_files_show_from_its_name() {
        assert_eq!(
            episodes("", &["zzRoot.Episode.S07E03.mkv"]),
            [episode("ZzRoot Episode", 7, 3)]
        );
        assert!(episodes("", &["05 - Fifth.mkv"]).is_empty());
    }

    #[test]
    fn keys_shows_by_cleaned_title_and_year() {
        let show = |path: &str| match &group_episodes(&directory(path), &["S01E01.mkv"])[0].kind {
            GroupedKind::Episode { show, .. } => show.clone(),
            GroupedKind::Movie { .. } => unreachable!(),
        };
        assert_eq!(show("zzDup Show"), show("zzDup.Show"));
        assert_ne!(show("zzYear Show (2010)"), show("zzYear Show (2015)"));
        assert_eq!(show("Hinted Probe {tvdb-12345}").title, "Hinted Probe");
    }

    #[test]
    fn makes_versions_and_stacks_of_one_episode() {
        assert_eq!(
            episodes("Show/Season 1", &["S01E02.mkv", "S01E02 - 720p.mkv"]),
            [("Show".to_owned(), 1, 2, None, vec![vec![1], vec![0]])]
        );
        assert_eq!(
            episodes("Show/Season 1", &["S01E01 cd1.mkv", "S01E01 cd2.mkv"]),
            [("Show".to_owned(), 1, 1, None, vec![vec![0, 1]])]
        );
        assert_eq!(
            episodes(
                "Show/Season 2",
                &["s02e02 - part2.mkv", "s02e02 - part1.mkv"]
            ),
            [("Show".to_owned(), 2, 2, None, vec![vec![1, 0]])]
        );
    }

    #[test]
    fn keeps_a_multi_episode_files_range() {
        assert_eq!(
            episodes("Show/Season 1", &["Show - S01E02-E03.mkv"]),
            [("Show".to_owned(), 1, 2, Some(3), vec![vec![0]])]
        );
    }
}
