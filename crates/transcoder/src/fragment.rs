//! The one fact the server reads from the MP4 fragments the process writes: where each starts.

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
    use super::*;

    /// Written by the transcoder copying a test episode from 30 seconds on, one piece per group
    /// of pictures: the initialization segment, and the head of the piece starting at 30 s.
    const INIT: &[u8] = include_bytes!("../fixtures/init-video.m4s");
    const PIECE: &[u8] = include_bytes!("../fixtures/fragment-head.m4s");

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
    }
}
