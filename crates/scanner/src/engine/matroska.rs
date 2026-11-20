//! The languages a Matroska file declares for its tracks, which ffprobe-plex does not report as
//! declared.
//!
//! A track that declares `und` has no language, and ffmpeg-plex writes `eng` for it, as for a
//! track without a `Language` element, which is English by the format's default. The reference
//! Jellyfin's prober leaves the first without a language, so the scanner reads the elements.

use std::fs::File;
use std::io::{BufReader, Read, Seek};
use std::path::Path;

/// Elements walked at most, so that a crafted file of empty elements cannot keep the scan busy.
const MAX_ELEMENTS: usize = 10_000;

const EBML: u32 = 0x1A45_DFA3;
const SEGMENT: u32 = 0x1853_8067;
const TRACKS: u32 = 0x1654_AE6B;
const TRACK_ENTRY: u32 = 0xAE;
const LANGUAGE: u32 = 0x0022_B59C;
const LANGUAGE_BCP47: u32 = 0x0022_B59D;
const CLUSTER: u32 = 0x1F43_B675;

/// Element sizes past this are not a track's header: a string or a list of tracks this long
/// is a broken file.
const MAX_HEADER_ELEMENT: u64 = 1 << 20;

/// What one `TrackEntry` declares, in the order the file lists them, which is the order ffmpeg
/// numbers their streams in.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct DeclaredTrack {
    pub language: Option<String>,
    pub language_bcp47: Option<String>,
}

impl DeclaredTrack {
    /// The language the entry states: its BCP 47 tag over its ISO 639-2 code, English when it
    /// writes neither, as the format's default; `und` is none.
    pub fn language(&self) -> Option<&str> {
        let declared = self
            .language_bcp47
            .as_deref()
            .or(self.language.as_deref())
            .unwrap_or("eng");
        (!declared.is_empty() && declared != "und").then_some(declared)
    }
}

/// The track entries of a Matroska or `WebM` file; `None` when its header cannot be walked to its
/// `Tracks` before the first cluster.
pub(crate) fn declared_tracks(path: &Path) -> Option<Vec<DeclaredTrack>> {
    let file = File::open(path).ok()?;
    let end = file.metadata().ok()?.len();
    let mut walk = Walk {
        reader: BufReader::new(file),
        elements: 0,
    };
    let (id, size) = walk.header()?;
    if id != EBML {
        return None;
    }
    walk.skip(size?)?;
    let (id, size) = walk.header()?;
    if id != SEGMENT {
        return None;
    }
    let segment_end = size.map_or(end, |size| walk.position().saturating_add(size).min(end));
    while walk.position() < segment_end {
        let (id, size) = walk.header()?;
        match id {
            TRACKS => return walk.tracks(size.filter(|size| *size <= MAX_HEADER_ELEMENT)?),
            CLUSTER => return None,
            _ => walk.skip(size?)?,
        }
    }
    None
}

struct Walk {
    reader: BufReader<File>,
    elements: usize,
}

impl Walk {
    fn position(&mut self) -> u64 {
        self.reader.stream_position().unwrap_or(u64::MAX)
    }

    fn byte(&mut self) -> Option<u8> {
        let mut byte = [0];
        self.reader.read_exact(&mut byte).ok()?;
        Some(byte[0])
    }

    /// An element's id and size; a size of all ones, which streams write for a size they did
    /// not know, is `None`.
    fn header(&mut self) -> Option<(u32, Option<u64>)> {
        self.elements += 1;
        if self.elements > MAX_ELEMENTS {
            return None;
        }
        let first = self.byte()?;
        let id_length = first.leading_zeros() + 1;
        if id_length > 4 {
            return None;
        }
        let mut id = u32::from(first);
        for _ in 1..id_length {
            id = (id << 8) | u32::from(self.byte()?);
        }
        let first = self.byte()?;
        let size_length = first.leading_zeros() + 1;
        if size_length > 8 {
            return None;
        }
        let mut size = u64::from(first) & (0xFF >> size_length);
        let mut unknown = size == (0xFF >> size_length);
        for _ in 1..size_length {
            let byte = self.byte()?;
            unknown &= byte == 0xFF;
            size = (size << 8) | u64::from(byte);
        }
        Some((id, (!unknown).then_some(size)))
    }

    fn skip(&mut self, size: u64) -> Option<()> {
        self.reader.seek_relative(i64::try_from(size).ok()?).ok()
    }

    fn string(&mut self, size: u64) -> Option<String> {
        let mut bytes = vec![0; usize::try_from(size.min(MAX_HEADER_ELEMENT)).ok()?];
        self.reader.read_exact(&mut bytes).ok()?;
        let text = String::from_utf8_lossy(&bytes);
        Some(text.trim_end_matches('\0').to_owned())
    }

    fn tracks(&mut self, size: u64) -> Option<Vec<DeclaredTrack>> {
        let end = self.position().checked_add(size)?;
        let mut tracks = Vec::new();
        while self.position() < end {
            let (id, size) = self.header()?;
            let size = size.filter(|size| *size <= MAX_HEADER_ELEMENT)?;
            if id == TRACK_ENTRY {
                tracks.push(self.entry(size)?);
            } else {
                self.skip(size)?;
            }
        }
        // Reading back where the element said it ends: past it, a size lied.
        (self.position() == end).then_some(tracks)
    }

    fn entry(&mut self, size: u64) -> Option<DeclaredTrack> {
        let end = self.position().checked_add(size)?;
        let mut track = DeclaredTrack::default();
        while self.position() < end {
            let (id, size) = self.header()?;
            let size = size.filter(|size| *size <= MAX_HEADER_ELEMENT)?;
            match id {
                LANGUAGE => track.language = Some(self.string(size)?),
                LANGUAGE_BCP47 => track.language_bcp47 = Some(self.string(size)?),
                _ => self.skip(size)?,
            }
        }
        Some(track)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn element(id: &[u8], payload: &[u8]) -> Vec<u8> {
        let mut bytes = id.to_vec();
        // An eight-byte size, as some muxers write every size.
        bytes.push(0x01);
        bytes.extend_from_slice(&(payload.len() as u64).to_be_bytes()[1..]);
        bytes.extend_from_slice(payload);
        bytes
    }

    fn declared(name: &str, bytes: &[u8]) -> Option<Vec<DeclaredTrack>> {
        let path = std::env::temp_dir().join(format!(
            "open-plex-scanner-matroska-{}-{name}",
            std::process::id()
        ));
        std::fs::write(&path, bytes).unwrap();
        let found = declared_tracks(&path);
        std::fs::remove_file(&path).unwrap();
        found
    }

    fn matroska(entries: &[Vec<u8>]) -> Vec<u8> {
        let tracks: Vec<u8> = entries
            .iter()
            .flat_map(|entry| element(&[0xAE], entry))
            .collect();
        let mut segment = element(&[0x15, 0x49, 0xA9, 0x66], b"info");
        segment.extend(element(&[0x16, 0x54, 0xAE, 0x6B], &tracks));
        segment.extend(element(&[0x1F, 0x43, 0xB6, 0x75], b"cluster"));
        let mut bytes = element(&[0x1A, 0x45, 0xDF, 0xA3], b"header");
        // The segment with a size of unknown length, as a live recording writes it.
        bytes.extend_from_slice(&[0x18, 0x53, 0x80, 0x67, 0xFF]);
        bytes.extend(segment);
        bytes
    }

    #[test]
    fn tells_a_declared_language_from_none() {
        let english = [element(&[0xD7], &[1]), element(&[0x22, 0xB5, 0x9C], b"eng")].concat();
        let silent = element(&[0xD7], &[2]);
        let bcp47 = [
            element(&[0x22, 0xB5, 0x9C], b"fre"),
            element(&[0x22, 0xB5, 0x9D], b"fr-CA"),
        ]
        .concat();
        let undetermined = element(&[0x22, 0xB5, 0x9C], b"und\0");
        let tracks = declared(
            "languages",
            &matroska(&[english, silent, bcp47, undetermined]),
        )
        .unwrap();
        let languages: Vec<Option<&str>> = tracks.iter().map(DeclaredTrack::language).collect();
        assert_eq!(languages, [Some("eng"), Some("eng"), Some("fr-CA"), None]);
    }

    #[test]
    fn a_file_that_is_not_matroska_declares_nothing() {
        assert_eq!(declared("mp4", b"\0\0\0\x18ftypisom"), None);
    }
}
