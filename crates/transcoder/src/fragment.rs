//! What the server reads from the MP4 the process writes: where each fragment starts, and how
//! its track's times become a player's.

/// The boxes of `data`, as their type and their content. A box running past the end stops the
/// walk: a fragment is read from its head only.
fn boxes(mut data: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8])> {
    std::iter::from_fn(move || {
        let size = usize::try_from(u32::from_be_bytes(data.get(..4)?.try_into().ok()?)).ok()?;
        let kind: [u8; 4] = data.get(4..8)?.try_into().ok()?;
        let (header, size) = match size {
            1 => (
                16,
                usize::try_from(u64::from_be_bytes(data.get(8..16)?.try_into().ok()?)).ok()?,
            ),
            0 => (8, data.len()),
            size => (8, size),
        };
        let content = data.get(header..size.min(data.len()))?;
        data = data.get(size..).unwrap_or_default();
        Some((kind, content))
    })
}

fn child(data: &[u8], kind: [u8; 4]) -> Option<&[u8]> {
    boxes(data)
        .find(|(found, _)| *found == kind)
        .map(|(_, content)| content)
}

/// How the sample times of a track become the times a player shows its samples at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TrackTiming {
    pub(crate) timescale: u32,
    /// What the track's edit list takes off every presentation time, in its timescale: the
    /// video's B-frame delay, the audio encoder's priming. Players apply it, and it differs
    /// between the tracks of one file (83 ms against 21 ms on a test film), so the times of
    /// two tracks only compare once it is taken off.
    pub(crate) shift: i64,
}

/// The timescale of a `mvhd` or `mdhd`, after the dates of its version.
fn header_timescale(header: &[u8]) -> Option<u32> {
    let at = if header.first()? == &1 { 20 } else { 12 };
    word(header, at)
}

/// The timing of the track an initialization segment describes, from its `mdhd` and its
/// edit list.
pub(crate) fn track_timing(init: &[u8]) -> Option<TrackTiming> {
    let movie = child(init, *b"moov")?;
    let track = child(movie, *b"trak")?;
    let timescale = header_timescale(child(child(track, *b"mdia")?, *b"mdhd")?)?;
    let shift = child(track, *b"edts")
        .and_then(|edits| child(edits, *b"elst"))
        .map_or(Some(0), |list| {
            edit_shift(list, timescale, header_timescale(child(movie, *b"mvhd")?)?)
        })?;
    Some(TrackTiming { timescale, shift })
}

/// The media time the first edit that shows media starts at, less the empty edits before it,
/// which delay the track: their durations are in the movie's timescale.
fn edit_shift(list: &[u8], timescale: u32, movie_timescale: u32) -> Option<i64> {
    let wide = list.first()? == &1;
    let count = usize::try_from(word(list, 4)?).ok()?;
    let entry = if wide { 20 } else { 12 };
    let mut delay = 0_i64;
    for index in 0..count {
        let at = 8 + index * entry;
        let (duration, media_time) = if wide {
            (
                i64::try_from(u64::from_be_bytes(list.get(at..at + 8)?.try_into().ok()?)).ok()?,
                i64::from_be_bytes(list.get(at + 8..at + 16)?.try_into().ok()?),
            )
        } else {
            (
                i64::from(word(list, at)?),
                i64::from(i32::from_be_bytes(word(list, at + 4)?.to_be_bytes())),
            )
        };
        if media_time >= 0 {
            return Some(media_time - delay);
        }
        if movie_timescale == 0 {
            return None;
        }
        delay += duration * i64::from(timescale) / i64::from(movie_timescale);
    }
    Some(-delay)
}

fn word(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

/// When a fragment's first sample is shown, in its track's timescale: its `tfdt` decode time
/// plus its composition offset from the `trun`. Not the decode time alone, which a copy takes
/// from the source's container and an encode from the encoder: on the test film the two differ
/// by up to half a second for the same key frame, where their presentation times agree.
pub(crate) fn first_presentation_time(fragment: &[u8]) -> Option<i64> {
    let track = child(child(fragment, *b"moof")?, *b"traf")?;
    let decode = child(track, *b"tfdt")?;
    let decode = if decode.first()? == &1 {
        i64::try_from(u64::from_be_bytes(decode.get(4..12)?.try_into().ok()?)).ok()?
    } else {
        i64::from(word(decode, 4)?)
    };
    let Some(run) = child(track, *b"trun") else {
        return Some(decode);
    };
    let flags = word(run, 0)? & 0x00ff_ffff;
    if flags & 0x800 == 0 || word(run, 4)? == 0 {
        return Some(decode);
    }
    // Version and flags, sample count, then the optional fields before the first sample's.
    let optional = [0x1, 0x4, 0x100, 0x200, 0x400];
    let at = 8 + 4 * optional.iter().filter(|bit| flags & **bit != 0).count();
    let offset = word(run, at)?;
    let offset = if run.first()? == &1 {
        i64::from(i32::from_be_bytes(offset.to_be_bytes()))
    } else {
        i64::from(offset)
    };
    Some(decode + offset)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// Written by the transcoder copying a test episode from 30 seconds on, one piece per group
    /// of pictures: the initialization segment, and the head of the piece starting at 30 s.
    const INIT: &[u8] = include_bytes!("../fixtures/init-video.m4s");
    const PIECE: &[u8] = include_bytes!("../fixtures/fragment-head.m4s");

    #[test]
    fn reads_where_a_piece_starts() {
        let timing = track_timing(INIT).unwrap();
        assert_eq!(timing.timescale, 16_000);
        // Decoded at 30 s, written a frame later, which the edit list takes back off.
        assert_eq!(timing.shift, 640);
        assert_eq!(first_presentation_time(PIECE), Some(30 * 16_000 + 640));
    }

    #[test]
    fn an_empty_edit_delays_the_track() {
        // Half a second of nothing, in a movie timescale of 1000, then the media from 1024.
        let mut list = vec![0, 0, 0, 0, 0, 0, 0, 2];
        for (duration, media_time) in [(500_u32, -1_i32), (0, 1024)] {
            list.extend(duration.to_be_bytes());
            list.extend(media_time.to_be_bytes());
            list.extend([0, 1, 0, 0]);
        }
        assert_eq!(edit_shift(&list, 48_000, 1000), Some(1024 - 24_000));
    }

    #[test]
    fn reads_nothing_from_what_is_cut_short_or_not_a_fragment() {
        assert_eq!(first_presentation_time(&PIECE[..100]), None);
        assert_eq!(first_presentation_time(INIT), None);
        assert_eq!(track_timing(PIECE), None);
        assert_eq!(track_timing(&[]), None);
    }
}
