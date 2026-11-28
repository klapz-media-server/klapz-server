//! The series scanner's file-name patterns, in the reference's order
//! (`docs/plex/scanner/discovery.md` §5.2, numbered as there). Each is matched by hand over the
//! lower-cased name's bytes with the regex's own leftmost, greedy-then-shorter semantics, so a
//! pattern reads beside its source line.

use crate::edit::CalendarDate;

use super::clean;

/// Where a file name puts an episode. `season` is absent for the number-only patterns, whose
/// season is the folder's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Numbers {
    pub season: Option<i32>,
    pub episode: i32,
    pub last: Option<i32>,
}

/// A range longer than this is read as its first episode. The reference has such a guard at an
/// unknown length and accepted 87 (`S01E13-E99`); one file standing for thousands of episodes
/// is a mistake in its name.
const LONGEST_RANGE: i32 = 100;

fn range(first: i32, second: Option<i32>) -> Option<i32> {
    // An inverted range is one episode, as the reference warns and assumes.
    second.filter(|second| *second > first && *second - first < LONGEST_RANGE)
}

fn at(text: &[u8], index: usize) -> Option<u8> {
    text.get(index).copied()
}

fn digits(text: &[u8], from: usize) -> usize {
    text.get(from..).map_or(0, |rest| {
        rest.iter().take_while(|c| c.is_ascii_digit()).count()
    })
}

fn number(text: &[u8], from: usize, length: usize) -> Option<i32> {
    std::str::from_utf8(text.get(from..from + length)?)
        .ok()?
        .parse()
        .ok()
}

/// The lengths `[0-9]{min,max}` may take at `from`, longest first.
fn lengths(text: &[u8], from: usize, min: usize, max: usize) -> impl Iterator<Item = usize> {
    (min..=digits(text, from).min(max)).rev()
}

fn is_separator(c: u8) -> bool {
    matches!(c, b'-' | b'.' | b'_' | b' ')
}

fn is_word(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || !c.is_ascii()
}

/// `[^0-9c-uw-z%]`: what may follow an episode number without making it part of a word.
fn ends_number(c: u8) -> bool {
    !(c.is_ascii_digit() || (b'c'..=b'u').contains(&c) || (b'w'..=b'z').contains(&c) || c == b'%')
}

/// `(?:-([0-9]{min,max}))?` after the first number at `from`, then `tail`.
fn then_range(
    text: &[u8],
    from: usize,
    (min, max): (usize, usize),
    first: i32,
    tail: impl Fn(usize) -> bool,
) -> Option<Numbers> {
    let numbers = |second| Numbers {
        season: None,
        episode: first,
        last: range(first, second),
    };
    if at(text, from) == Some(b'-') {
        for length in lengths(text, from + 1, min, max) {
            if tail(from + 1 + length) {
                return Some(numbers(number(text, from + 1, length)));
            }
        }
    }
    tail(from).then(|| numbers(None))
}

/// A number of `min` to `max` digits at `from`, its range, then `tail`.
fn numbered(
    text: &[u8],
    from: usize,
    first: (usize, usize),
    second: (usize, usize),
    tail: impl Fn(usize) -> bool + Copy,
) -> Option<Numbers> {
    lengths(text, from, first.0, first.1).find_map(|length| {
        let episode = number(text, from, length)?;
        then_range(text, from + length, second, episode, tail)
    })
}

/// Patterns 1 to 4, which name the season, over the cleaned name with its extension: a
/// second episode needs a separator after it, and the extension's dot is one
/// (`S01E02-E03.mkv` holds 2 and 3). Returns where the show's part of the name ends.
pub(super) fn season_and_episode(text: &[u8]) -> Option<(usize, Numbers)> {
    standard(text)
        .or_else(|| season_dash(text))
        .or_else(|| cross(text))
        .or_else(|| special(text))
}

/// Patterns 1 to 3, with which the movie scanner recognizes an episode.
pub(super) fn names_season(text: &[u8]) -> bool {
    standard(text).is_some() || season_dash(text).is_some() || cross(text).is_some()
}

/// 1: `s(\d+)[._ ]*ep?(\d+)([- ._]?s\d+)?([- ]?e+p?(\d+)[- ._])?`.
fn standard(text: &[u8]) -> Option<(usize, Numbers)> {
    (0..text.len()).find_map(|start| {
        let season_length = digits(text, start + 1);
        if text[start] != b's' || season_length == 0 {
            return None;
        }
        let season = number(text, start + 1, season_length)?;
        let mut index = start + 1 + season_length;
        while matches!(at(text, index), Some(b'.' | b'_' | b' ')) {
            index += 1;
        }
        if at(text, index) != Some(b'e') {
            return None;
        }
        index += 1;
        if at(text, index) == Some(b'p') && digits(text, index + 1) > 0 {
            index += 1;
        }
        let episode_length = digits(text, index);
        let episode = number(text, index, episode_length)?;
        index += episode_length;

        // A second season (`S01E11 S01E12`) is read past and not kept, as the reference does.
        for skip in [1, 0] {
            if skip == 1 && !at(text, index).is_some_and(is_separator) {
                continue;
            }
            let season_at = index + skip;
            let length = digits(text, season_at + 1);
            if at(text, season_at) == Some(b's') && length > 0 {
                index = season_at + 1 + length;
                break;
            }
        }
        let second = [1, 0].into_iter().find_map(|skip| {
            if skip == 1 && !matches!(at(text, index), Some(b'-' | b' ')) {
                return None;
            }
            let mut cursor = index + skip;
            let es = text
                .get(cursor..)?
                .iter()
                .take_while(|c| **c == b'e')
                .count();
            if es == 0 {
                return None;
            }
            cursor += es;
            if at(text, cursor) == Some(b'p') {
                cursor += 1;
            }
            let length = digits(text, cursor);
            let value = number(text, cursor, length)?;
            at(text, cursor + length)
                .is_some_and(is_separator)
                .then_some(value)
        });
        Some((
            start,
            Numbers {
                season: Some(season),
                episode,
                last: range(episode, second),
            },
        ))
    })
}

/// 2: `s(\d{2})[-._ ]+(\d+)`, `S03-03`.
fn season_dash(text: &[u8]) -> Option<(usize, Numbers)> {
    (0..text.len()).find_map(|start| {
        if text[start] != b's' || digits(text, start + 1) < 2 {
            return None;
        }
        let season = number(text, start + 1, 2)?;
        let mut index = start + 3;
        let separators = text
            .get(index..)?
            .iter()
            .take_while(|c| is_separator(**c))
            .count();
        if separators == 0 {
            return None;
        }
        index += separators;
        let episode = number(text, index, digits(text, index))?;
        Some((
            start,
            Numbers {
                season: Some(season),
                episode,
                last: None,
            },
        ))
    })
}

/// 3: `([^0-9]|^)(19[3-9]\d|20[0-5]\d|\d{1,2})x(\d+)((-\d+)?x(\d+))?`, `3x03`, `3x03-3x04`,
/// `3x03x04`.
fn cross(text: &[u8]) -> Option<(usize, Numbers)> {
    (0..=text.len()).find_map(|show_end| {
        let mut starts = Vec::with_capacity(2);
        if at(text, show_end).is_some_and(|c| !c.is_ascii_digit()) {
            starts.push(show_end + 1);
        }
        if show_end == 0 {
            starts.push(0);
        }
        starts.into_iter().find_map(|start| {
            let year_like = matches!(
                text.get(start..start + 3),
                Some([b'1', b'9', b'3'..=b'9'] | [b'2', b'0', b'0'..=b'5'])
            ) && digits(text, start) >= 4;
            let season_lengths = if year_like {
                &[4, 2, 1][..]
            } else {
                &[2, 1][..]
            };
            season_lengths.iter().find_map(|&length| {
                if digits(text, start) < length || at(text, start + length) != Some(b'x') {
                    return None;
                }
                let season = number(text, start, length)?;
                let episode_at = start + length + 1;
                let episode_length = digits(text, episode_at);
                let episode = number(text, episode_at, episode_length)?;
                let mut tail = episode_at + episode_length;
                if at(text, tail) == Some(b'-') && digits(text, tail + 1) > 0 {
                    let skipped = tail + 1 + digits(text, tail + 1);
                    if at(text, skipped) == Some(b'x') && digits(text, skipped + 1) > 0 {
                        tail = skipped;
                    }
                }
                let second = (at(text, tail) == Some(b'x'))
                    .then(|| number(text, tail + 1, digits(text, tail + 1)))
                    .flatten();
                Some((
                    show_end,
                    Numbers {
                        season: Some(season),
                        episode,
                        last: range(episode, second),
                    },
                ))
            })
        })
    })
}

/// 4: `(^|[-._ ])+sp(\d{2,3})([-._ ]|v\d+|$)+`, a special: `SP01` is S00E01.
fn special(text: &[u8]) -> Option<(usize, Numbers)> {
    (0..text.len()).find_map(|start| {
        let opens = start == 0 || is_separator(text[start - 1]);
        if !opens || text.get(start..start + 2) != Some(b"sp") {
            return None;
        }
        lengths(text, start + 2, 2, 3).find_map(|length| {
            let after = start + 2 + length;
            let closes = match at(text, after) {
                None => true,
                Some(b'v') => digits(text, after + 1) > 0,
                Some(c) => is_separator(c),
            };
            if !closes {
                return None;
            }
            let episode = number(text, start + 2, length)?;
            Some((
                start,
                Numbers {
                    season: Some(0),
                    episode,
                    last: None,
                },
            ))
        })
    })
}

/// `[^0-9a-z()[\]]`, between the parts of a date.
fn is_date_separator(c: u8) -> bool {
    !(c.is_ascii_digit() || c.is_ascii_lowercase() || matches!(c, b'(' | b')' | b'[' | b']'))
}

fn date_separators(text: &[u8], from: usize) -> usize {
    text.get(from..).map_or(0, |rest| {
        rest.iter().take_while(|c| is_date_separator(**c)).count()
    })
}

fn calendar(year: i32, month: i32, day: i32) -> Option<CalendarDate> {
    CalendarDate::parse(&format!("{year:04}-{month:02}-{day:02}"))
}

/// Patterns 7 to 9, an air date, over the lower-cased stem.
pub(super) fn air_date(text: &[u8]) -> Option<CalendarDate> {
    year_first(text)
        .or_else(|| month_first(text))
        .or_else(|| loose(text))
}

/// Three numbers of `lengths` digits with date separators between, where `start` begins.
fn three_numbers(text: &[u8], start: usize, lengths: [usize; 3]) -> Option<([i32; 3], usize)> {
    let mut index = start;
    let mut values = [0; 3];
    for (position, length) in lengths.into_iter().enumerate() {
        if position > 0 {
            let separators = date_separators(text, index);
            if separators == 0 {
                return None;
            }
            index += separators;
        }
        if digits(text, index) < length {
            return None;
        }
        values[position] = number(text, index, length)?;
        index += length;
    }
    Some((values, index))
}

/// 7: `(\d{4})SEP+(\d{2})SEP+(\d{2})([^0-9]|$)`, `2009-02-10`.
fn year_first(text: &[u8]) -> Option<CalendarDate> {
    (0..text.len()).find_map(|start| {
        let ([year, month, day], end) = three_numbers(text, start, [4, 2, 2])?;
        if at(text, end).is_some_and(|c| c.is_ascii_digit()) {
            return None;
        }
        calendar(year, month, day)
    })
}

/// 8: `(\d{2})SEP+(\d{2})SEP+(\d{4})([^0-9a-z]|$)`, `02-10-2009`, the month first.
fn month_first(text: &[u8]) -> Option<CalendarDate> {
    (0..text.len()).find_map(|start| {
        let ([month, day, year], end) = three_numbers(text, start, [2, 2, 4])?;
        if at(text, end).is_some_and(|c| c.is_ascii_digit() || c.is_ascii_lowercase()) {
            return None;
        }
        calendar(year, month, day)
    })
}

/// 9: `[.\s-]([12]\d{3})[.\s-](0?[1-9]|1[012])[.\s-]([12][0-9]|3[01]|0?[1-9])([.\s-]|$)`.
fn loose(text: &[u8]) -> Option<CalendarDate> {
    let delimiter = |index: usize| matches!(at(text, index), Some(b'.' | b'-' | b' ' | b'\t'));
    let digit = |index: usize, range: std::ops::RangeInclusive<u8>| {
        at(text, index).is_some_and(|c| range.contains(&c))
    };
    let month_lengths = |index: usize| {
        let mut lengths = Vec::with_capacity(2);
        if digit(index, b'0'..=b'0') && digit(index + 1, b'1'..=b'9') {
            lengths.push(2);
        }
        if digit(index, b'1'..=b'9') {
            lengths.push(1);
        }
        if digit(index, b'1'..=b'1') && digit(index + 1, b'0'..=b'2') {
            lengths.push(2);
        }
        lengths
    };
    let day_lengths = |index: usize| {
        let mut lengths = Vec::with_capacity(2);
        if (digit(index, b'1'..=b'2') && digit(index + 1, b'0'..=b'9'))
            || (digit(index, b'3'..=b'3') && digit(index + 1, b'0'..=b'1'))
            || (digit(index, b'0'..=b'0') && digit(index + 1, b'1'..=b'9'))
        {
            lengths.push(2);
        }
        if digit(index, b'1'..=b'9') {
            lengths.push(1);
        }
        lengths
    };
    (0..text.len()).find_map(|start| {
        let year_at = start + 1;
        if !delimiter(start)
            || !digit(year_at, b'1'..=b'2')
            || digits(text, year_at) < 4
            || !delimiter(year_at + 4)
        {
            return None;
        }
        let year = number(text, year_at, 4)?;
        let month_at = year_at + 5;
        month_lengths(month_at)
            .into_iter()
            .find_map(|month_length| {
                let day_at = month_at + month_length + 1;
                if !delimiter(month_at + month_length) {
                    return None;
                }
                let month = number(text, month_at, month_length)?;
                day_lengths(day_at).into_iter().find_map(|day_length| {
                    let end = day_at + day_length;
                    if end != text.len() && !delimiter(end) {
                        return None;
                    }
                    calendar(year, month, number(text, day_at, day_length)?)
                })
            })
    })
}

/// Patterns 10 to 18, an episode number alone, over the cleaned stem.
pub(super) fn episode_alone(text: &[u8]) -> Option<Numbers> {
    let end = |index: usize| index == text.len();
    let then_ends = |index: usize| at(text, index).is_some_and(ends_number);
    let then_not_hex = |index: usize| {
        at(text, index).is_some_and(|c| !c.is_ascii_digit() && !(b'a'..=b'f').contains(&c))
    };
    let separator_at = |index: usize| is_separator(text[index]);

    // 10: `(\d{1,4})(-\d{1,4})?[-._ ]*of[-._ ]*\d{1,4}`, `15 of 26`.
    let of_total = |index: usize| {
        let mut cursor = index;
        while at(text, cursor).is_some_and(is_separator) {
            cursor += 1;
        }
        if text.get(cursor..cursor + 2) != Some(b"of") {
            return false;
        }
        cursor += 2;
        while at(text, cursor).is_some_and(is_separator) {
            cursor += 1;
        }
        digits(text, cursor) > 0
    };
    (0..text.len())
        .find_map(|start| numbered(text, start, (1, 4), (1, 4), of_total))
        // 11: `^(\d{2,4})(-\d{2,4})?[^0-9a-f]`, `05 - Title`.
        .or_else(|| numbered(text, 0, (2, 4), (2, 4), then_not_hex))
        // 12: `^(\d)(-\d{1,2})?[^0-9a-f]`.
        .or_else(|| numbered(text, 0, (1, 1), (1, 2), then_not_hex))
        // 13: `(\b|_)e[a-z]*[-._ ]*(\d{2,4})(-\d{2,4})?([^0-9c-uw-z%]|$)`, `ep 14`.
        .or_else(|| {
            (0..text.len()).find_map(|start| {
                let opens = start == 0 || !is_word(text[start - 1]) || text[start - 1] == b'_';
                if text[start] != b'e' || !opens {
                    return None;
                }
                let mut cursor = start + 1;
                while at(text, cursor).is_some_and(|c| c.is_ascii_lowercase()) {
                    cursor += 1;
                }
                while at(text, cursor).is_some_and(is_separator) {
                    cursor += 1;
                }
                numbered(text, cursor, (2, 4), (2, 4), |index| {
                    end(index) || then_ends(index)
                })
            })
        })
        // 14: `[-._ ](\d{2,4})(-\d{2,4})?(v\d+)?$`, `Show - 13v2`.
        .or_else(|| {
            let versioned_end = |index: usize| {
                end(index)
                    || (at(text, index) == Some(b'v')
                        && digits(text, index + 1) > 0
                        && end(index + 1 + digits(text, index + 1)))
            };
            (0..text.len())
                .filter(|start| separator_at(*start))
                .find_map(|start| numbered(text, start + 1, (2, 4), (2, 4), versioned_end))
        })
        // 15: `[-._ ](\d{2,4})(-\d{2,4})?[^0-9c-uw-z%]+`, `Show - 12 [1080p]`.
        .or_else(|| {
            (0..text.len())
                .filter(|start| separator_at(*start))
                .find_map(|start| numbered(text, start + 1, (2, 4), (2, 4), then_ends))
        })
        // 16: the last `[-._ ](?<!\d\d-)(\d{2,4})(-\d{2,4})?[^0-9c-uw-z%]+`.
        .or_else(|| {
            (0..text.len())
                .rev()
                .filter(|start| separator_at(*start))
                .filter(|start| {
                    !(text[*start] == b'-'
                        && *start >= 2
                        && text[start - 2].is_ascii_digit()
                        && text[start - 1].is_ascii_digit())
                })
                .find_map(|start| numbered(text, start + 1, (2, 4), (2, 4), then_ends))
        })
        // 17: `[^0-9x](\d{2,4})(-\d{2,4})?$`, `Show707`.
        .or_else(|| {
            (0..text.len())
                .filter(|start| !text[*start].is_ascii_digit() && text[*start] != b'x')
                .find_map(|start| numbered(text, start + 1, (2, 4), (2, 4), end))
        })
        // 18: `^(\d{1,4})(-\d{1,4})?$`, `01`.
        .or_else(|| numbered(text, 0, (1, 4), (1, 4), end))
}

/// The stem as the number patterns read it: lower-cased, its year masked, resolutions and
/// codecs taken out, and the show's name replaced by `series `, so that neither a year nor a
/// number in the show's name is taken for an episode.
pub(super) fn pattern_text(stem: &str, show_title: &str, current_year: i32) -> String {
    let mut text = stem.to_lowercase();
    if let Some((found, _)) = clean::year_in(&text, current_year) {
        let mut masked = String::with_capacity(found.len());
        masked.extend(found.chars().next());
        masked.push_str("xxxx");
        masked.extend(found.chars().last());
        text = text.replace(&found, &masked);
    }
    let mut bytes = text.into_bytes();
    for matcher in [resolution as Matcher, frame_size, codec] {
        bytes = remove_matches(&bytes, matcher);
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let words: Vec<String> = show_title
        .split_whitespace()
        .map(str::to_lowercase)
        .collect();
    replace_show(&text, &words)
}

type Matcher = fn(&[u8], usize) -> Option<usize>;

fn remove_matches(text: &[u8], matcher: Matcher) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    let mut index = 0;
    while index < text.len() {
        match matcher(text, index) {
            Some(length) if length > 0 => index += length,
            _ => {
                out.push(text[index]);
                index += 1;
            }
        }
    }
    out
}

const LINES: [&[u8]; 4] = [b"480", b"720", b"1080", b"2160"];

/// `(?<=[^[0-9])` — a resolution counts after anything but a digit or a bracket, so that
/// `[1080p]` is left to stop pattern 15.
fn resolution_may_start(text: &[u8], index: usize) -> bool {
    index > 0 && !text[index - 1].is_ascii_digit() && text[index - 1] != b'['
}

fn line_at(text: &[u8], index: usize) -> Option<usize> {
    LINES
        .iter()
        .find(|line| text.get(index..).is_some_and(|rest| rest.starts_with(line)))
        .map(|line| line.len())
}

/// `(480|720|1080|2160)[pi]`.
fn resolution(text: &[u8], index: usize) -> Option<usize> {
    if !resolution_may_start(text, index) {
        return None;
    }
    let length = line_at(text, index)?;
    matches!(at(text, index + length), Some(b'p' | b'i')).then_some(length + 1)
}

/// `[0-9]{3,4}x(480|720|1080|2160)[pi]?`.
fn frame_size(text: &[u8], index: usize) -> Option<usize> {
    if !resolution_may_start(text, index) {
        return None;
    }
    let width = digits(text, index);
    if !(3..=4).contains(&width) || at(text, index + width) != Some(b'x') {
        return None;
    }
    let length = width + 1 + line_at(text, index + width + 1)?;
    Some(length + usize::from(matches!(at(text, index + length), Some(b'p' | b'i'))))
}

/// `[hx]\.?26[4,5](?=[^0-9])`.
fn codec(text: &[u8], index: usize) -> Option<usize> {
    if !matches!(at(text, index), Some(b'h' | b'x')) {
        return None;
    }
    let dot = usize::from(at(text, index + 1) == Some(b'.'));
    let length = 1 + dot + 3;
    let matched = text.get(index + 1 + dot..index + 3 + dot) == Some(b"26")
        && matches!(at(text, index + 3 + dot), Some(b'4' | b',' | b'5'))
        && at(text, index + length).is_some_and(|c| !c.is_ascii_digit());
    matched.then_some(length)
}

/// Replaces the first run of the show's words, with `[-._ ()+]+` between them, by `series `.
fn replace_show(text: &str, words: &[String]) -> String {
    let is_between = |c: char| matches!(c, '-' | '.' | '_' | ' ' | '(' | ')' | '+');
    let matches_at = |start: usize| {
        let mut cursor = start;
        for (position, word) in words.iter().enumerate() {
            if position > 0 {
                let between = text[cursor..]
                    .chars()
                    .take_while(|c| is_between(*c))
                    .count();
                if between == 0 {
                    return None;
                }
                cursor += between;
            }
            if !text[cursor..].starts_with(word.as_str()) {
                return None;
            }
            cursor += word.len();
        }
        Some(cursor)
    };
    if words.is_empty() {
        return text.to_owned();
    }
    text.char_indices()
        .find_map(|(start, _)| matches_at(start).map(|end| (start, end)))
        .map_or_else(
            || text.to_owned(),
            |(start, end)| format!("{}series {}", &text[..start], &text[end..]),
        )
}

/// An anime opening or ending (`NCOP1`, `Creditless ED`), an extra of its show. The
/// reference's pattern also takes a bare `Opening` or `Ending`, which titles episodes; only the
/// forms that cannot be a title's word count here, and only for a file no season-and-episode
/// pattern reads.
pub(super) fn is_anime_extra(stem: &str) -> bool {
    let tokens: Vec<&str> = stem
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .collect();
    let short = |token: &str| {
        let token = token.strip_prefix("NC").unwrap_or(token);
        let rest = ["OPED", "OP", "ED"]
            .iter()
            .find_map(|kind| token.strip_prefix(kind));
        rest.is_some_and(|rest| {
            let rest = rest.trim_start_matches(|c: char| c.is_ascii_digit());
            let rest = rest.strip_prefix('v').map_or(rest, |version| {
                version.trim_start_matches(|c: char| c.is_ascii_digit())
            });
            rest.len() <= 1 && rest.chars().all(|c| c.is_ascii_alphabetic())
        })
    };
    let long = |token: &str| {
        ["OPED", "OP", "ED", "Opening", "Ending"]
            .iter()
            .any(|kind| {
                token
                    .strip_prefix(kind)
                    .is_some_and(|rest| rest.len() <= 2 && rest.chars().all(|c| c.is_ascii_digit()))
            })
    };
    tokens.iter().enumerate().any(|(position, token)| {
        short(token)
            || (matches!(*token, "Creditless" | "Clean")
                && tokens.get(position + 1).is_some_and(|next| long(next)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbers(season: Option<i32>, episode: i32, last: Option<i32>) -> Numbers {
        Numbers {
            season,
            episode,
            last,
        }
    }

    fn with_season(name: &str) -> Option<Numbers> {
        season_and_episode(format!("{}.", name.to_lowercase()).as_bytes()).map(|(_, found)| found)
    }

    #[test]
    fn reads_the_measured_multi_episode_files() {
        let measured = [
            ("S01E02-E03", 2, Some(3)),
            ("S01E04-E05-E06", 4, Some(5)),
            ("1x07x08", 7, Some(8)),
            ("S01E11 S01E12", 11, Some(12)),
            ("S01E09-10", 9, None),
            ("S01E01E02E03", 1, None),
            ("S01E13-E99", 13, Some(99)),
            ("S01E20-E15", 20, None),
        ];
        for (name, episode, last) in measured {
            assert_eq!(
                with_season(name),
                Some(numbers(Some(1), episode, last)),
                "{name}"
            );
        }
    }

    #[test]
    fn reads_seasons_from_the_measured_names() {
        assert_eq!(
            with_season("Daily Probe S2021E10"),
            Some(numbers(Some(2021), 10, None))
        );
        assert_eq!(with_season("04x02"), Some(numbers(Some(4), 2, None)));
        assert_eq!(with_season("show s03-03"), Some(numbers(Some(3), 3, None)));
        assert_eq!(with_season("Show SP01"), Some(numbers(Some(0), 1, None)));
        assert_eq!(
            with_season("Show.Name.1x04.720p.HDTV"),
            Some(numbers(Some(1), 4, None))
        );
    }

    #[test]
    fn finds_where_the_show_ends() {
        let text = b"zzroot.episode.s07e03.";
        assert_eq!(season_and_episode(text).map(|(end, _)| end), Some(15));
    }

    #[test]
    fn reads_the_measured_episode_numbers() {
        let alone = |name: &str| episode_alone(name.to_lowercase().as_bytes()).map(|n| n.episode);
        assert_eq!(alone("05 - Fifth"), Some(5));
        assert_eq!(alone("series 107"), Some(107));
        assert_eq!(alone("series .302"), Some(302));
        assert_eq!(alone("series  - 105"), Some(105));
        assert_eq!(alone("series  - 12 [1080p]"), Some(12));
        assert_eq!(alone("series  - 13v2"), Some(13));
        assert_eq!(alone("series  - ep 14"), Some(14));
        assert_eq!(alone("series  - 15 of 26"), Some(15));
        assert_eq!(alone("Show707"), Some(707));
        assert_eq!(alone("01"), Some(1));
        assert_eq!(alone("Episode 6"), None);
        assert_eq!(alone("Behind it"), None);
    }

    #[test]
    fn reads_air_dates_year_or_month_first() {
        let date = |name: &str| air_date(name.to_lowercase().as_bytes()).map(|d| d.to_string());
        assert_eq!(
            date("Daily.Probe.2021-03-04").as_deref(),
            Some("2021-03-04")
        );
        assert_eq!(
            date("Daily Probe 05.03.2021").as_deref(),
            Some("2021-05-03")
        );
        assert_eq!(date("Show 2021 3 4 x").as_deref(), Some("2021-03-04"));
        assert_eq!(date("Show 2021-13-40"), None);
    }

    #[test]
    fn masks_the_year_resolution_codec_and_show() {
        assert_eq!(
            pattern_text("zzShow.Probe.1x04.720p.HDTV.x264-GRP", "ZzShow Probe", 2026),
            "series .1x04..hdtv.-grp"
        );
        assert_eq!(
            pattern_text("Show (2010) - 12 [1080p]", "Show", 2026),
            "series  (xxxx) - 12 [1080p]"
        );
    }

    #[test]
    fn knows_an_anime_opening_from_an_episode_title() {
        assert!(is_anime_extra("zzAnime Probe - NCOP1"));
        assert!(is_anime_extra("Show - Creditless Ending 2"));
        assert!(is_anime_extra("Show ED2v2"));
        assert!(!is_anime_extra("Show - The Ending"));
        assert!(!is_anime_extra("Show - Ed Wood"));
    }
}
