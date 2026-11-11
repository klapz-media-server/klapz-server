//! What the server reads from the MP4 fragments the process writes: where each starts and ends,
//! and, for audio, the samples of a time window rewritten as a fragment of their own.

use std::ops::Range;

/// A box: its type, where it starts in what was walked, and where its content lies.
struct Mp4Box {
    kind: [u8; 4],
    start: usize,
    content: Range<usize>,
}

/// The boxes of `data`. A box running past the end stops the walk: a fragment is often read
/// from its head only.
fn boxes(data: &[u8]) -> impl Iterator<Item = Mp4Box> + '_ {
    let mut at = 0;
    std::iter::from_fn(move || {
        let rest = data.get(at..)?;
        let size = usize::try_from(u32::from_be_bytes(rest.get(..4)?.try_into().ok()?)).ok()?;
        let kind: [u8; 4] = rest.get(4..8)?.try_into().ok()?;
        let (header, size) = match size {
            1 => (
                16,
                usize::try_from(u64::from_be_bytes(rest.get(8..16)?.try_into().ok()?)).ok()?,
            ),
            0 => (8, rest.len()),
            size => (8, size),
        };
        if size < header || rest.len() < header {
            return None;
        }
        let start = at;
        let end = (at + size).min(data.len());
        at += size;
        Some(Mp4Box {
            kind,
            start,
            content: start + header..end,
        })
    })
}

fn child(data: &[u8], kind: [u8; 4]) -> Option<&[u8]> {
    boxes(data)
        .find(|found| found.kind == kind)
        .and_then(|found| data.get(found.content))
}

/// The timescale of the track an initialization segment describes, from its `mdhd`.
pub(crate) fn timescale(init: &[u8]) -> Option<u32> {
    let media = child(child(child(init, *b"moov")?, *b"trak")?, *b"mdia")?;
    let header = child(media, *b"mdhd")?;
    // Version 1 holds 64-bit dates before it.
    let at = if header.first()? == &1 { 20 } else { 12 };
    Some(u32::from_be_bytes(header.get(at..at + 4)?.try_into().ok()?))
}

fn word(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

/// The base decode time of a track fragment, from its `tfdt`.
fn decode_time(track: &[u8]) -> Option<i64> {
    let decode = child(track, *b"tfdt")?;
    if decode.first()? == &1 {
        i64::try_from(u64::from_be_bytes(decode.get(4..12)?.try_into().ok()?)).ok()
    } else {
        Some(i64::from(word(decode, 4)?))
    }
}

/// When a fragment's first sample is shown, in its track's timescale: its `tfdt` decode time
/// plus its composition offset from the `trun`. Not the decode time alone, which a copy takes
/// from the source's container and an encode from the encoder: on the test film the two differ
/// by up to half a second for the same key frame, where their presentation times agree.
pub(crate) fn first_presentation_time(fragment: &[u8]) -> Option<i64> {
    let track = child(child(fragment, *b"moof")?, *b"traf")?;
    let decode = decode_time(track)?;
    let Some(run) = child(track, *b"trun") else {
        return Some(decode);
    };
    let flags = word(run, 0)? & 0x00ff_ffff;
    if flags & TRUN_OFFSETS == 0 || word(run, 4)? == 0 {
        return Some(decode);
    }
    // Version and flags, sample count, then the optional fields before the first sample's.
    let optional = [
        TRUN_DATA_OFFSET,
        TRUN_FIRST_FLAGS,
        TRUN_DURATIONS,
        TRUN_SIZES,
        TRUN_FLAGS,
    ];
    let at = 8 + 4 * optional.iter().filter(|bit| flags & **bit != 0).count();
    Some(decode + composition_offset(run.first()? == &1, word(run, at)?))
}

const TFHD_BASE_DATA_OFFSET: u32 = 0x1;
const TFHD_DESCRIPTION: u32 = 0x2;
const TFHD_DURATION: u32 = 0x8;
const TFHD_SIZE: u32 = 0x10;
const TFHD_BASE_IS_MOOF: u32 = 0x2_0000;
const TRUN_DATA_OFFSET: u32 = 0x1;
const TRUN_FIRST_FLAGS: u32 = 0x4;
const TRUN_DURATIONS: u32 = 0x100;
const TRUN_SIZES: u32 = 0x200;
const TRUN_FLAGS: u32 = 0x400;
const TRUN_OFFSETS: u32 = 0x800;

fn composition_offset(signed: bool, raw: u32) -> i64 {
    if signed {
        i64::from(i32::from_be_bytes(raw.to_be_bytes()))
    } else {
        i64::from(raw)
    }
}

/// A sample of a fragment: when it is decoded and shown, for how long, its entry in the
/// `trun` and its bytes.
struct Sample<'a> {
    decode: i64,
    shown: i64,
    duration: i64,
    entry: &'a [u8],
    data: &'a [u8],
}

/// A fragment of one track, read whole: what rewriting part of it needs.
struct Fragment<'a> {
    sequence: &'a [u8],
    header: &'a [u8],
    run_version: u8,
    run_flags: u32,
    first_flags: Option<&'a [u8]>,
    samples: Vec<Sample<'a>>,
}

/// Reads a fragment as the process writes them: one `moof` holding one track whose header
/// counts from the `moof`, with sample durations and sizes stated. `None` for anything else.
fn read_fragment(data: &[u8]) -> Option<Fragment<'_>> {
    let moof = boxes(data).find(|found| found.kind == *b"moof")?;
    let fragment = data.get(moof.content.clone())?;
    let sequence = child(fragment, *b"mfhd")?;
    let mut tracks = boxes(fragment).filter(|found| found.kind == *b"traf");
    let track = fragment.get(tracks.next()?.content)?;
    if tracks.next().is_some() {
        return None;
    }
    let mut header = None;
    let mut run = None;
    for found in boxes(track) {
        let content = track.get(found.content)?;
        match &found.kind {
            b"tfhd" => header = Some(content),
            b"trun" if run.is_none() => run = Some(content),
            b"tfdt" => {}
            // Sample groups, encryption, a second run: not what this reads.
            _ => return None,
        }
    }
    let (header, run) = (header?, run?);
    let header_flags = word(header, 0)? & 0x00ff_ffff;
    if header_flags & TFHD_BASE_DATA_OFFSET != 0 || header_flags & TFHD_BASE_IS_MOOF == 0 {
        return None;
    }
    // Track id, then the optional defaults in their order.
    let mut at = 8;
    if header_flags & TFHD_DESCRIPTION != 0 {
        at += 4;
    }
    let default_duration = (header_flags & TFHD_DURATION != 0)
        .then(|| word(header, at))
        .flatten();
    if header_flags & TFHD_DURATION != 0 {
        at += 4;
    }
    let default_size = (header_flags & TFHD_SIZE != 0)
        .then(|| word(header, at))
        .flatten();

    let run_version = *run.first()?;
    let run_flags = word(run, 0)? & 0x00ff_ffff;
    if run_flags & TRUN_DATA_OFFSET == 0 {
        return None;
    }
    let count = usize::try_from(word(run, 4)?).ok()?;
    let data_offset = i64::from(i32::from_be_bytes(word(run, 8)?.to_be_bytes()));
    let mut at = 12;
    let first_flags = if run_flags & TRUN_FIRST_FLAGS == 0 {
        None
    } else {
        at += 4;
        Some(run.get(at - 4..at)?)
    };
    let entry_size = 4 * [TRUN_DURATIONS, TRUN_SIZES, TRUN_FLAGS, TRUN_OFFSETS]
        .iter()
        .filter(|bit| run_flags & **bit != 0)
        .count();
    let mut decode = decode_time(track)?;
    let mut position = usize::try_from(i64::try_from(moof.start).ok()? + data_offset).ok()?;
    let mut samples = Vec::with_capacity(count);
    for _ in 0..count {
        let entry = run.get(at..at + entry_size)?;
        at += entry_size;
        let mut field = 0;
        let mut next = |present: bool| {
            present.then(|| {
                let value = word(entry, field);
                field += 4;
                value
            })
        };
        let duration = next(run_flags & TRUN_DURATIONS != 0).map_or(default_duration, |v| v)?;
        let size = next(run_flags & TRUN_SIZES != 0).map_or(default_size, |v| v)?;
        let _flags = next(run_flags & TRUN_FLAGS != 0);
        let offset = next(run_flags & TRUN_OFFSETS != 0)
            .flatten()
            .map_or(0, |raw| composition_offset(run_version == 1, raw));
        let size = usize::try_from(size).ok()?;
        samples.push(Sample {
            decode,
            shown: decode + offset,
            duration: i64::from(duration),
            entry,
            data: data.get(position..position + size)?,
        });
        decode += i64::from(duration);
        position += size;
    }
    Some(Fragment {
        sequence,
        header,
        run_version,
        run_flags,
        first_flags,
        samples,
    })
}

/// When a whole fragment's last sample stops being shown, in its track's timescale.
pub(crate) fn presentation_end(fragment: &[u8]) -> Option<i64> {
    read_fragment(fragment)?
        .samples
        .iter()
        .map(|sample| sample.shown + sample.duration)
        .max()
}

fn mp4_box(kind: [u8; 4], parts: &[&[u8]]) -> Vec<u8> {
    let size = 8 + parts.iter().map(|part| part.len()).sum::<usize>();
    let mut out = Vec::with_capacity(size);
    out.extend_from_slice(&u32::try_from(size).unwrap_or(u32::MAX).to_be_bytes());
    out.extend_from_slice(&kind);
    for part in parts {
        out.extend_from_slice(part);
    }
    out
}

/// The samples of a whole fragment shown at any moment of `[from, to)`, as a fragment of their
/// own; `to` `None` runs to the end. Empty when none is, `None` for a fragment
/// [`read_fragment`] cannot read.
///
/// A sample straddling a bound is kept on both sides of it, so that two neighbouring windows cut
/// from different processes — whose audio frames fall at different places — meet without a
/// gap: they overlap by less than a frame instead, which a player overwrites.
pub(crate) fn cut(fragment: &[u8], from: i64, to: Option<i64>) -> Option<Vec<u8>> {
    let read = read_fragment(fragment)?;
    let kept: Vec<(usize, &Sample)> = read
        .samples
        .iter()
        .enumerate()
        .filter(|(_, sample)| {
            sample.shown + sample.duration > from && to.is_none_or(|to| sample.shown < to)
        })
        .collect();
    let Some(&(first_index, first)) = kept.first() else {
        return Some(Vec::new());
    };
    // The first sample's own flags only stay if that sample does.
    let first_flags = read.first_flags.filter(|_| first_index == 0);
    let run_flags = if first_flags.is_some() {
        read.run_flags
    } else {
        read.run_flags & !TRUN_FIRST_FLAGS
    };
    let count = u32::try_from(kept.len()).ok()?;
    let decode = u64::try_from(first.decode).ok()?;
    let entries: Vec<u8> = kept
        .iter()
        .flat_map(|(_, sample)| sample.entry.iter().copied())
        .collect();
    let media: Vec<u8> = kept
        .iter()
        .flat_map(|(_, sample)| sample.data.iter().copied())
        .collect();

    let header = mp4_box(*b"tfhd", &[read.header]);
    let decode_box = mp4_box(
        *b"tfdt",
        &[&0x0100_0000_u32.to_be_bytes(), &decode.to_be_bytes()],
    );
    let moof = |data_offset: u32| {
        let version_flags = (u32::from(read.run_version) << 24) | run_flags;
        let run = mp4_box(
            *b"trun",
            &[
                &version_flags.to_be_bytes(),
                &count.to_be_bytes(),
                &data_offset.to_be_bytes(),
                first_flags.unwrap_or_default(),
                &entries,
            ],
        );
        let track = mp4_box(*b"traf", &[&header, &decode_box, &run]);
        mp4_box(*b"moof", &[&mp4_box(*b"mfhd", &[read.sequence]), &track])
    };
    // The data offset counts from the `moof` to the samples, past the `mdat` header; its own
    // value does not change the `moof`'s size.
    let data_offset = u32::try_from(moof(0).len() + 8).ok()?;
    let mut out = moof(data_offset);
    out.extend(mp4_box(*b"mdat", &[&media]));
    Some(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// Written by the transcoder copying a test episode from 30 seconds on, one piece per group
    /// of pictures: the initialization segment, and the head of the piece starting at 30 s.
    const INIT: &[u8] = include_bytes!("../fixtures/init-video.m4s");
    const PIECE: &[u8] = include_bytes!("../fixtures/fragment-head.m4s");
    /// An audio piece the transcoder wrote converting a film's E-AC3 to AAC at 48 kHz: 184
    /// frames of 1024 samples, the first shown at 15.701 s. Its media data is zeroed, the
    /// boxes around it as written.
    const AUDIO: &[u8] = include_bytes!("../fixtures/fragment-audio.m4s");
    const AUDIO_START: i64 = 753_664;
    const FRAMES: i64 = 184;
    const FRAME: i64 = 1024;

    #[test]
    fn reads_where_a_piece_starts() {
        assert_eq!(timescale(INIT), Some(16_000));
        // Decoded at 30 s, shown a frame later.
        assert_eq!(first_presentation_time(PIECE), Some(30 * 16_000 + 640));
    }

    #[test]
    fn reads_nothing_from_what_is_cut_short_or_not_a_fragment() {
        assert_eq!(first_presentation_time(&PIECE[..100]), None);
        assert_eq!(first_presentation_time(INIT), None);
        assert_eq!(timescale(PIECE), None);
        assert_eq!(timescale(&[]), None);
        assert_eq!(presentation_end(&AUDIO[..AUDIO.len() - 1]), None);
    }

    #[test]
    fn reads_where_a_whole_piece_ends() {
        assert_eq!(first_presentation_time(AUDIO), Some(AUDIO_START));
        assert_eq!(presentation_end(AUDIO), Some(AUDIO_START + FRAMES * FRAME));
    }

    #[test]
    fn a_cut_keeps_the_frames_shown_within_it_and_those_straddling_it() {
        // From inside the third frame to inside the eleventh.
        let from = AUDIO_START + 2 * FRAME + 100;
        let to = AUDIO_START + 10 * FRAME + 1;
        let piece = cut(AUDIO, from, Some(to)).unwrap();
        assert_eq!(
            first_presentation_time(&piece),
            Some(AUDIO_START + 2 * FRAME)
        );
        assert_eq!(presentation_end(&piece), Some(AUDIO_START + 11 * FRAME));
        // The bytes are the same frames: a cut of the cut changes nothing.
        assert_eq!(cut(&piece, from, Some(to)).unwrap(), piece);
    }

    #[test]
    fn a_cut_ending_on_a_frame_leaves_that_frame_to_the_next() {
        let bound = AUDIO_START + 5 * FRAME;
        let before = cut(AUDIO, 0, Some(bound)).unwrap();
        let after = cut(AUDIO, bound, None).unwrap();
        assert_eq!(presentation_end(&before), Some(bound));
        assert_eq!(first_presentation_time(&after), Some(bound));
        assert_eq!(presentation_end(&after), presentation_end(AUDIO));
        // Together, every byte of media once.
        let media = |fragment: &[u8]| child(fragment, *b"mdat").unwrap().len();
        assert_eq!(media(&before) + media(&after), media(AUDIO));
    }

    #[test]
    fn a_cut_outside_the_piece_is_empty() {
        assert_eq!(cut(AUDIO, 0, Some(AUDIO_START)), Some(Vec::new()));
        assert_eq!(
            cut(AUDIO, AUDIO_START + FRAMES * FRAME, None),
            Some(Vec::new())
        );
        assert_eq!(cut(INIT, 0, None), None);
    }
}
