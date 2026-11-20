//! Subtitles kept in a file of their own beside the media: where their content is, and the
//! offset each viewer shifts a subtitle by.
//!
//! A sidecar's location is stored as the scanner wrote it, a URL: `file:///mnt/films/a.fr.srt`
//! for a file next to the media, `blob://` or `media://…` for a subtitle the reference
//! downloaded into its own storage, whose content was not imported.

use crate::metadata::{MediaStream, StreamKind};

/// Where a sidecar subtitle's content lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sidecar {
    /// A file beside the media, at this path, written as the scanning machine writes paths.
    File(String),
    /// Downloaded into the reference's own storage, which this server does not hold.
    Downloaded,
}

impl Sidecar {
    /// The sidecar a track's stored location names. None for a track inside the media.
    pub fn of(stream: &MediaStream) -> Option<Self> {
        let stored = stream
            .external_path
            .as_deref()
            .filter(|url| !url.is_empty())?;
        Some(
            stored
                .strip_prefix("file://")
                .map_or(Self::Downloaded, |path| Self::File(native_path(path))),
        )
    }

    /// The path of a file sidecar.
    pub fn path(&self) -> Option<&str> {
        match self {
            Self::File(path) => Some(path),
            Self::Downloaded => None,
        }
    }
}

/// The path a `file://` URL's path names, written the way its machine writes paths:
/// `/C:/Films/a%20b.srt` is `C:\Films\a b.srt`, `/mnt/a%20b.srt` is `/mnt/a b.srt`.
///
/// The scanner escapes only some characters (spaces, `%`) and writes the rest as they are, so
/// only well-formed escapes are decoded and anything else is kept.
fn native_path(url_path: &str) -> String {
    let decoded = percent_decode(url_path);
    let bytes = decoded.as_bytes();
    let drive =
        bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':';
    if drive {
        decoded[1..].replace('/', "\\")
    } else {
        decoded
    }
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let escaped = (bytes[index] == b'%')
            .then(|| bytes.get(index + 1..index + 3))
            .flatten()
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        if let Some(byte) = escaped {
            decoded.push(byte);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded)
        .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned())
}

/// A sidecar file the engine is asked to open, with what it must lie within.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarSource {
    /// As stored, decoded into a path.
    pub path: String,
    /// The media file the sidecar belongs to: the sidecar must lie in its directory or below.
    pub media_path: String,
    /// The roots of the media's library.
    pub roots: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum SidecarError {
    /// Not on this machine, or not where a sidecar of its media may be.
    #[error("the sidecar file is not on this machine")]
    Missing,
    #[error("the sidecar file is too large to be a subtitle")]
    TooLarge,
    #[error("the sidecar file could not be read or removed")]
    Io(#[source] std::io::Error),
}

/// Why an offset is not kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum OffsetRefusal {
    /// The reference accepts an offset on any track but publishes it on subtitles only.
    #[error("only a subtitle is shifted")]
    NotASubtitle,
}

/// Whether a track takes an offset.
///
/// # Errors
///
/// [`OffsetRefusal::NotASubtitle`] for a video or audio track.
pub fn check_offset_target(stream: &MediaStream) -> Result<(), OffsetRefusal> {
    if stream.kind == StreamKind::Subtitle {
        Ok(())
    } else {
        Err(OffsetRefusal::NotASubtitle)
    }
}

/// Text in the encoding a subtitle file was written in, as UTF-8.
///
/// Subtitle files carry no declaration of their encoding. Valid UTF-8 is taken as such, a
/// byte-order mark included, and anything else as Windows-1252, the encoding of most
/// non-UTF-8 subtitles in Western languages — what the reference's own conversion produced on
/// a Latin-1 file.
pub fn subtitle_text(bytes: Vec<u8>) -> String {
    match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => error
            .as_bytes()
            .iter()
            .map(|&byte| windows_1252(byte))
            .collect(),
    }
}

/// The character a Windows-1252 byte encodes. The five bytes it leaves undefined map to the
/// C1 controls of the same value, as browsers decode them.
fn windows_1252(byte: u8) -> char {
    const HIGH: [char; 32] = [
        '\u{20AC}', '\u{81}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}',
        '\u{2021}', '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{8D}',
        '\u{017D}', '\u{8F}', '\u{90}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}',
        '\u{2013}', '\u{2014}', '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}',
        '\u{9D}', '\u{017E}', '\u{0178}',
    ];
    match byte {
        0x80..=0x9F => HIGH[usize::from(byte - 0x80)],
        _ => char::from(byte),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::ids::{PartId, StreamId};
    use crate::metadata::TrackTraits;

    fn stored(url: Option<&str>) -> MediaStream {
        MediaStream {
            id: StreamId::new(1),
            file_id: PartId::new(1),
            kind: StreamKind::Subtitle,
            position: 0,
            codec: Some("srt".to_owned()),
            language: None,
            channels: None,
            bitrate: None,
            is_default: false,
            is_forced: false,
            external_path: url.map(str::to_owned),
            title: None,
            stream_identifier: None,
            profile: None,
            bit_depth: None,
            traits: TrackTraits::default(),
            video: None,
            audio: None,
            subtitle: None,
            required_bandwidths: Vec::new(),
            stated: crate::metadata::StatedTrack::default(),
        }
    }

    #[test]
    fn reads_a_file_url_into_the_path_its_machine_writes() {
        // Both forms are the importer's, from the reference database.
        assert_eq!(
            Sidecar::of(&stored(Some(
                "file:///mnt/acd/Plex/TVShows/The%20Walking%20Dead/S04E03%20(Isolement).en.srt"
            ))),
            Some(Sidecar::File(
                "/mnt/acd/Plex/TVShows/The Walking Dead/S04E03 (Isolement).en.srt".to_owned()
            ))
        );
        assert_eq!(
            Sidecar::of(&stored(Some(
                "file:///C:/Users/viewer/Documents/TEST/Soul.de.srt"
            ))),
            Some(Sidecar::File(
                r"C:\Users\viewer\Documents\TEST\Soul.de.srt".to_owned()
            ))
        );
    }

    #[test]
    fn keeps_what_the_scanner_left_unescaped() {
        assert_eq!(
            native_path("/mnt/D%C3%A9j%C3%A0 100%.srt"),
            "/mnt/Déjà 100%.srt"
        );
        assert_eq!(native_path("/mnt/Après.srt"), "/mnt/Après.srt");
        assert_eq!(native_path("/mnt/50%zz.srt"), "/mnt/50%zz.srt");
    }

    #[test]
    fn a_downloaded_subtitle_has_no_file() {
        assert_eq!(
            Sidecar::of(&stored(Some("blob://"))),
            Some(Sidecar::Downloaded)
        );
        assert_eq!(
            Sidecar::of(&stored(Some(
                "media://b/ee.bundle/Contents/Subtitles/fr/a.srt"
            ))),
            Some(Sidecar::Downloaded)
        );
    }

    #[test]
    fn a_track_inside_the_media_is_no_sidecar() {
        assert_eq!(Sidecar::of(&stored(None)), None);
        assert_eq!(Sidecar::of(&stored(Some(""))), None);
    }

    #[test]
    fn utf8_stays_as_it_is_and_anything_else_is_read_as_windows_1252() {
        let utf8 = "\u{FEFF}1\r\nDu début\r\n".as_bytes().to_vec();
        assert_eq!(subtitle_text(utf8.clone()).as_bytes(), utf8.as_slice());
        assert_eq!(
            subtitle_text(b"Du d\xe9but \x80 \x92".to_vec()),
            "Du début € \u{2019}"
        );
    }

    #[test]
    fn only_a_subtitle_takes_an_offset() {
        assert_eq!(check_offset_target(&stored(None)), Ok(()));
        let mut audio = stored(None);
        audio.kind = StreamKind::Audio;
        assert_eq!(
            check_offset_target(&audio),
            Err(OffsetRefusal::NotASubtitle)
        );
    }
}
