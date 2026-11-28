//! `CleanName`: the title and year a file or folder name gives, as the reference reads them
//! (`docs/plex/scanner/discovery.md` §4.3).

use std::time::{SystemTime, UNIX_EPOCH};

use unicode_normalization::UnicodeNormalization;

use crate::edit::CalendarDate;

/// Stands for the year in the token list; a name cannot hold a NUL.
const YEAR_BREAK: &str = "\0";

const SEPARATORS: [char; 7] = [' ', '-', '_', '.', '(', ')', '+'];

/// The legacy scanner's list. As garbage words all count; as files to keep, `vob` and `bup` do
/// not, which the reference dropped.
pub(super) const VIDEO_EXTENSIONS: [&str; 48] = [
    "3g2", "3gp", "asf", "asx", "avc", "avi", "avs", "bivx", "bup", "divx", "dv", "dvr-ms", "evo",
    "fli", "flv", "m2t", "m2ts", "m2v", "m4v", "mkv", "mov", "mp4", "mpeg", "mpg", "mts", "nsv",
    "nuv", "ogm", "ogv", "tp", "pva", "qt", "rm", "rmvb", "sdp", "svq3", "strm", "ts", "ty", "vdr",
    "viv", "vob", "vp3", "wmv", "wtv", "xsp", "xvid", "webm",
];

const FORMATS: [&str; 37] = [
    "ac3",
    "divx",
    "fragment",
    "limited",
    "ogg",
    "ogm",
    "ntsc",
    "pal",
    "ps3avchd",
    "r1",
    "r3",
    "r5",
    "720i",
    "720p",
    "1080i",
    "1080p",
    "remux",
    "x264",
    "xvid",
    "vorbis",
    "aac",
    "dts",
    "fs",
    "ws",
    "1920x1080",
    "1280x720",
    "h264",
    "h",
    "264",
    "prores",
    "uhd",
    "2160p",
    "truehd",
    "atmos",
    "hevc",
    "h265",
    "se",
];

const SOURCES: [&str; 38] = [
    "bdrc",
    "bdrip",
    "bluray",
    "bd",
    "brrip",
    "hdrip",
    "hddvd",
    "hddvdrip",
    "cam",
    "ddc",
    "dvdrip",
    "dvd",
    "r1",
    "r3",
    "r5",
    "retail",
    "dsr",
    "dsrip",
    "hdtv",
    "pdtv",
    "ppv",
    "stv",
    "tvrip",
    "bdscr",
    "dvdscr",
    "dvdscreener",
    "scr",
    "screener",
    "svcd",
    "vcd",
    "tc",
    "telecine",
    "ts",
    "telesync",
    "webrip",
    "web-dl",
    "wp",
    "workprint",
];

const MISC: [&str; 22] = [
    "multi",
    "multisubs",
    "cd1",
    "cd2",
    "1cd",
    "2cd",
    "custom",
    "internal",
    "repack",
    "read.nfo",
    "readnfo",
    "nfofix",
    "proper",
    "rerip",
    "dubbed",
    "subbed",
    "extended",
    "unrated",
    "xxx",
    "nfo",
    "dvxa",
    "web",
];

const SMALL_WORDS: [&str; 21] = [
    "a", "an", "and", "as", "at", "but", "by", "en", "for", "if", "in", "of", "on", "or", "the",
    "to", "v", "v.", "via", "vs", "vs.",
];

fn is_garbage(token: &str) -> bool {
    let token = token.to_lowercase();
    let token = token.as_str();
    MISC.contains(&token)
        || FORMATS.contains(&token)
        || SOURCES.contains(&token)
        || VIDEO_EXTENSIONS.contains(&token)
}

/// A name written backwards (`p0801.yaruLB`) is turned around when it holds more than two long
/// format or source words reversed.
fn is_reversed(name: &str) -> bool {
    let tokens: std::collections::HashSet<String> = tokens(name)
        .map(|token| token.to_lowercase().chars().rev().collect())
        .collect();
    FORMATS
        .iter()
        .chain(SOURCES.iter())
        .filter(|word| word.len() > 3)
        .filter(|word| tokens.contains(**word))
        .count()
        > 2
}

fn tokens(name: &str) -> impl Iterator<Item = &str> {
    name.split(SEPARATORS)
        .map(str::trim)
        .filter(|token| !token.is_empty())
}

pub(super) fn current_year() -> i32 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_secs()).ok());
    now.and_then(CalendarDate::containing)
        .map_or(i32::MAX, CalendarDate::year)
}

/// The first `([.-(\[])YYYY([.-)\]_,+])` of `chars`, by the index of its opening delimiter.
fn find_year(chars: &[char]) -> Option<(usize, i32)> {
    chars.windows(6).enumerate().find_map(|(start, window)| {
        let opens = matches!(window[0], '(' | '[' | '.' | '-');
        let closes = matches!(window[5], '.' | '-' | ')' | ']' | '_' | ',' | '+');
        let digits =
            matches!(window[1], '1' | '2') && window[1..5].iter().all(char::is_ascii_digit);
        if !(opens && closes && digits) {
            return None;
        }
        let year = window[1..5].iter().collect::<String>().parse().ok()?;
        Some((start, year))
    })
}

/// The year a name gives, and its six characters with their delimiters, when the year is
/// plausible: after 1900 and not in the future.
pub(super) fn year_in(name: &str, current_year: i32) -> Option<(String, i32)> {
    let chars: Vec<char> = name.chars().collect();
    // Only the first candidate counts: a name whose first is implausible has no year at all.
    let (start, year) = find_year(&chars)?;
    (1900 < year && year < current_year + 1)
        .then(|| (chars[start..start + 6].iter().collect(), year))
}

/// The title and year of a file's stem or a folder's name.
pub(super) fn clean_name(name: &str, current_year: i32) -> (String, Option<i32>) {
    let mut name: String = name.nfkc().collect();
    if is_reversed(&name) {
        name = name.chars().rev().collect();
    }

    let year = year_in(&name, current_year).map(|(found, year)| {
        name = name.replace(&found, &format!(" {YEAR_BREAK} "));
        year
    });

    let mut chars: Vec<char> = name.chars().collect();
    while let Some(stripped) = strip_one_bracket_pass(&chars) {
        chars = stripped;
    }
    let mut name: String = chars.into_iter().collect();
    if name.to_lowercase().ends_with(".dvdmedia") {
        name.truncate(name.len() - ".dvdmedia".len());
    }
    name.push(' ');
    let mut chars: Vec<char> = name.chars().collect();
    for spec in [
        audio_spec('5', true),
        audio_spec('5', false),
        audio_spec('7', true),
        audio_spec('7', false),
    ] {
        chars = replace_matches(&chars, spec);
    }
    chars = replace_matches(&chars, video_codec);
    let name: String = chars.into_iter().collect();

    let tokens: Vec<&str> = tokens(&name).collect();
    // Every garbage word counts: the reference reads `Internal.Affairs.1990-INTERNAL` as
    // `Affairs`, where the legacy scanner forgave a word's second occurrence.
    let good: Vec<bool> = tokens
        .iter()
        .map(|token| tokens.len() <= 2 || !is_garbage(token))
        .collect();
    let mut kept = Vec::new();
    let mut bad = 0;
    for (position, (token, good)) in tokens.iter().zip(&good).enumerate() {
        if *good && bad < 2 {
            if *token == YEAR_BREAK {
                if position == 0 {
                    continue;
                }
                break;
            }
            kept.push(*token);
        }
        if !good {
            bad += 1;
        }
    }
    if kept.is_empty() {
        // A name that is nothing but its year is titled by it rather than left untitled.
        match tokens.iter().find(|token| **token != YEAR_BREAK) {
            Some(first) => kept.push(first),
            None => return (year.map(|year| year.to_string()).unwrap_or_default(), year),
        }
    }
    (title_case(&kept), year)
}

/// Removes every `[…]` and `{…}` group, left to right, once; `None` when there was none.
fn strip_one_bracket_pass(chars: &[char]) -> Option<Vec<char>> {
    let mut out = Vec::with_capacity(chars.len());
    let mut removed = false;
    let mut index = 0;
    while index < chars.len() {
        let close = match chars[index] {
            '[' => Some(']'),
            '{' => Some('}'),
            _ => None,
        };
        let end = close.and_then(|close| {
            chars[index + 1..]
                .iter()
                .position(|c| *c == close)
                .filter(|length| *length > 0)
                .map(|length| index + 1 + length)
        });
        if let Some(end) = end {
            index = end + 1;
            removed = true;
        } else {
            out.push(chars[index]);
            index += 1;
        }
    }
    removed.then_some(out)
}

/// Replaces every match of `matcher`, left to right and without overlap, by a space.
fn replace_matches(chars: &[char], matcher: impl Fn(&[char], usize) -> Option<usize>) -> Vec<char> {
    let mut out = Vec::with_capacity(chars.len());
    let mut index = 0;
    while index < chars.len() {
        match matcher(chars, index) {
            Some(length) if length > 0 => {
                out.push(' ');
                index += length;
            }
            _ => {
                out.push(chars[index]);
                index += 1;
            }
        }
    }
    out
}

/// `([^0-9])N\.1[ ]*ch(.)` with `channels`, `([^0-9])N\.1([^0-9]?)` without; the 7.1 form
/// without `ch` needs its trailing non-digit.
fn audio_spec(first: char, channels: bool) -> impl Fn(&[char], usize) -> Option<usize> {
    move |chars, start| {
        let at = |index: usize| chars.get(start + index).copied();
        if at(0).is_none_or(|c| c.is_ascii_digit())
            || at(1) != Some(first)
            || at(2) != Some('.')
            || at(3) != Some('1')
        {
            return None;
        }
        let mut length = 4;
        if channels {
            while at(length) == Some(' ') {
                length += 1;
            }
            let ch = at(length).zip(at(length + 1));
            if !ch
                .is_some_and(|(c, h)| c.eq_ignore_ascii_case(&'c') && h.eq_ignore_ascii_case(&'h'))
            {
                return None;
            }
            at(length + 2)?;
            return Some(length + 3);
        }
        match at(length) {
            Some(c) if !c.is_ascii_digit() => Some(length + 1),
            _ if first == '7' => None,
            _ => Some(length),
        }
    }
}

/// `\b[hx]\.26[45]\b`, a native addition.
fn video_codec(chars: &[char], start: usize) -> Option<usize> {
    let word = |index: usize| {
        chars
            .get(index)
            .is_some_and(|c| c.is_alphanumeric() || *c == '_')
    };
    let at = |index: usize| chars.get(start + index).map(char::to_ascii_lowercase);
    let matched = (start == 0 || !word(start - 1))
        && matches!(at(0), Some('h' | 'x'))
        && at(1) == Some('.')
        && at(2) == Some('2')
        && at(3) == Some('6')
        && matches!(at(4), Some('4' | '5'))
        && !word(start + 5);
    matched.then_some(5)
}

const PUNCTUATION: &str = "!\"#$%&'\u{2018}()*+,-./:;?@[\\]_`{|}~";

fn is_punctuation(c: char) -> bool {
    PUNCTUATION.contains(c)
}

fn is_small(word: &str) -> bool {
    SMALL_WORDS.contains(&word.to_lowercase().as_str())
}

/// Upper-cases the first ASCII letter after leading punctuation, as `titlecase`'s `CAPFIRST`.
fn capitalize_first(word: &str) -> String {
    let skip = word.chars().take_while(|c| is_punctuation(*c)).count();
    word.chars()
        .enumerate()
        .map(|(index, c)| {
            if index == skip {
                c.to_ascii_uppercase()
            } else {
                c
            }
        })
        .collect()
}

/// `capitalize()` on the word-character run a small word is, past its leading punctuation.
fn capitalize_small(word: &str) -> String {
    let skip = word.chars().take_while(|c| is_punctuation(*c)).count();
    word.chars()
        .enumerate()
        .map(|(index, c)| match index.cmp(&skip) {
            std::cmp::Ordering::Less => c,
            std::cmp::Ordering::Equal => c.to_ascii_uppercase(),
            std::cmp::Ordering::Greater => c.to_ascii_lowercase(),
        })
        .collect()
}

/// Whether a letter after the first of the word's leading letters is a capital (`ZzShow`, `IV`):
/// `titlecase` leaves such a word alone.
fn has_inner_capital(word: &str) -> bool {
    let mut letters = word
        .chars()
        .skip_while(|c| is_punctuation(*c))
        .take_while(char::is_ascii_alphabetic);
    letters.next().is_some() && letters.any(|c| c.is_ascii_uppercase())
}

fn has_inline_period(word: &str) -> bool {
    word.as_bytes()
        .windows(3)
        .any(|w| w[0].is_ascii_alphabetic() && w[1] == b'.' && w[2].is_ascii_alphabetic())
}

/// Strips leading punctuation and at most one trailing punctuation mark, which is how
/// `titlecase` finds a small word at either end of the title.
fn bare(word: &str) -> &str {
    let word = word.trim_start_matches(is_punctuation);
    word.strip_suffix(is_punctuation).unwrap_or(word)
}

/// The Python `titlecase` the reference ports, then the first letter of the title upper-cased:
/// it writes `zzShow Probe` as `ZzShow Probe`, which `titlecase` alone would leave.
fn title_case(words: &[&str]) -> String {
    let mut out: Vec<String> = words
        .iter()
        .map(|word| {
            if has_inline_period(word) || has_inner_capital(word) {
                (*word).to_owned()
            } else if is_small(word) {
                word.to_lowercase()
            } else {
                capitalize_first(word)
            }
        })
        .collect();
    if out.first().is_some_and(|word| is_small(bare(word))) {
        out[0] = capitalize_small(&out[0]);
    }
    if let Some(last) = out.last_mut() {
        if is_small(bare(last)) {
            *last = capitalize_small(last);
        }
    }
    for index in 1..out.len() {
        if out[index - 1].ends_with([':', '.', ';', '?', '!'])
            && SMALL_WORDS
                .iter()
                .any(|small| out[index].starts_with(small))
        {
            out[index] = capitalize_first(&out[index]);
        }
    }
    if let Some(first) = out.first_mut() {
        *first = capitalize_first(first);
    }
    out.join(" ")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn clean(name: &str) -> (String, Option<i32>) {
        clean_name(name, 2026)
    }

    #[test]
    fn reads_the_titles_and_years_the_reference_wrote() {
        let measured: [(&str, &str, Option<i32>); 26] = [
            (
                "The.Matrix.1999.1080p.BluRay.x264-GRP",
                "The Matrix",
                Some(1999),
            ),
            (
                "Le.Fabuleux.Destin.2001.FRENCH.720p.WEB.H264-XYZ",
                "Le Fabuleux Destin",
                Some(2001),
            ),
            (
                "Star.Wars.Episode.IV.1977.REMASTERED.2160p.UHD.BluRay.x265.HDR",
                "Star Wars Episode IV",
                Some(1977),
            ),
            (
                "Ghost.Movie.2012.DVDRip.XviD-GRP",
                "Ghost Movie",
                Some(2012),
            ),
            ("Movie.2008.720p.WEB-DL.DD5.1.H264", "Movie", Some(2008)),
            (
                "Title.With.Dots.and.Year.In.Middle.1984.Remake.2003",
                "Title With Dots and Year in Middle",
                Some(1984),
            ),
            ("Movie.Title.(2010)", "Movie Title", Some(2010)),
            ("Bracket Movie [2015]", "Bracket Movie", Some(2015)),
            ("Tenet (2020) (2160p)", "Tenet", Some(2020)),
            (
                "Some Movie (2015) - Extended Edition",
                "Some Movie",
                Some(2015),
            ),
            ("[Group] Paprika (2006) [BD 1080p]", "Paprika", Some(2006)),
            (
                "The Lord of the Rings - The Fellowship of the Ring (2001)",
                "The Lord of the Rings the Fellowship of the Ring",
                Some(2001),
            ),
            (
                "Dash-Title - Part Two (2005)",
                "Dash Title Part Two",
                Some(2005),
            ),
            ("Les Misérables (2012)", "Les Misérables", Some(2012)),
            ("Blade Runner 2049 (2017)", "Blade Runner 2049", Some(2017)),
            ("1917 (2019)", "1917", Some(2019)),
            ("the godfather (1972)", "The Godfather", Some(1972)),
            ("UPPER CASE TITLE (1999)", "UPPER CASE TITLE", Some(1999)),
            ("Kill Bill Vol 1 (2003)", "Kill Bill Vol 1", Some(2003)),
            ("Multi One (2001)", "One", Some(2001)),
            ("Internal.Affairs.1990-INTERNAL", "Affairs", Some(1990)),
            ("Movie Title 2010 1080p", "Movie Title 2010", None),
            (
                "2001.A.Space.Odyssey.1968",
                "2001 a Space Odyssey 1968",
                None,
            ),
            ("Future.Film.2030", "Future Film 2030", None),
            ("Old Film (1850)", "Old Film 1850", None),
            ("Ext Mp4 (2001)", "Ext", Some(2001)),
        ];
        for (name, title, year) in measured {
            assert_eq!(clean(name), (title.to_owned(), year), "{name}");
        }
        assert_eq!(clean("no year at all"), ("No Year at All".to_owned(), None));
    }

    #[test]
    fn titles_folders_as_the_reference_did() {
        assert_eq!(
            clean("Single In Folder"),
            ("Single in Folder".to_owned(), None)
        );
        assert_eq!(
            clean("zzShow Probe (2019)"),
            ("ZzShow Probe".to_owned(), Some(2019))
        );
        assert_eq!(clean("zzDup.Show"), ("ZzDup Show".to_owned(), None));
        assert_eq!(clean("KB2"), ("KB2".to_owned(), None));
        assert_eq!(
            clean("Alien (1979) {imdb-tt0078748}"),
            ("Alien".to_owned(), Some(1979))
        );
        assert_eq!(
            clean("Hinted Probe {tvdb-12345}"),
            ("Hinted Probe".to_owned(), None)
        );
    }

    #[test]
    fn turns_a_reversed_name_around() {
        assert_eq!(
            clean("PRG-462x.yaRulB.p0801.9991.xirtaM.ehT"),
            ("The Matrix".to_owned(), Some(1999))
        );
    }

    #[test]
    fn titles_a_name_that_is_only_a_year_by_it() {
        assert_eq!(clean("(2012)"), ("2012".to_owned(), Some(2012)));
    }

    #[test]
    fn removes_brackets_left_to_right_until_none_is_left() {
        let chars: Vec<char> = "a[b]c{d}e[]".chars().collect();
        let once: String = strip_one_bracket_pass(&chars)
            .unwrap()
            .into_iter()
            .collect();
        assert_eq!(once, "ace[]");
    }
}
