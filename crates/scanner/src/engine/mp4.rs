//! The two layout facts of an MP4 that ffprobe does not report.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Boxes walked at most, so that a crafted file of empty boxes cannot keep the scan busy.
const MAX_BOXES: usize = 10_000;

/// Boxes whose payload is boxes, on the way down to the chunk offsets.
const CONTAINERS: [&[u8; 4]; 5] = [b"moov", b"trak", b"mdia", b"minf", b"stbl"];

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Layout {
    /// `moov` comes before `mdat`.
    pub optimized_for_streaming: bool,
    /// A track's chunk offsets are 64-bit (`co64`).
    pub has_64bit_offsets: bool,
}

pub(crate) fn layout(path: &Path) -> std::io::Result<Layout> {
    let mut file = File::open(path)?;
    let end = file.metadata()?.len();
    let mut walk = Walk {
        file: &mut file,
        boxes: 0,
        layout: Layout::default(),
        seen_mdat: false,
    };
    walk.boxes_in(0, end, true)?;
    Ok(walk.layout)
}

struct Walk<'a> {
    file: &'a mut File,
    boxes: usize,
    layout: Layout,
    seen_mdat: bool,
}

impl Walk<'_> {
    fn boxes_in(&mut self, mut position: u64, end: u64, top: bool) -> std::io::Result<()> {
        while position + 8 <= end && self.boxes < MAX_BOXES {
            self.boxes += 1;
            self.file.seek(SeekFrom::Start(position))?;
            let mut header = [0; 8];
            self.file.read_exact(&mut header)?;
            let kind = [header[4], header[5], header[6], header[7]];
            let (size, header_length) =
                match u32::from_be_bytes([header[0], header[1], header[2], header[3]]) {
                    0 => (end - position, 8),
                    1 => {
                        let mut large = [0; 8];
                        self.file.read_exact(&mut large)?;
                        (u64::from_be_bytes(large), 16)
                    }
                    size => (u64::from(size), 8),
                };
            if size < header_length || position + size > end {
                break;
            }
            match &kind {
                b"mdat" if top => self.seen_mdat = true,
                b"moov" if top && !self.seen_mdat => self.layout.optimized_for_streaming = true,
                b"co64" => self.layout.has_64bit_offsets = true,
                _ => {}
            }
            if CONTAINERS.contains(&&kind) {
                self.boxes_in(position + header_length, position + size, false)?;
            }
            position += size;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use std::io::Write;

    fn mp4_box(kind: [u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut bytes = u32::try_from(payload.len() + 8)
            .unwrap()
            .to_be_bytes()
            .to_vec();
        bytes.extend_from_slice(&kind);
        bytes.extend_from_slice(payload);
        bytes
    }

    fn layout_of(name: &str, bytes: &[u8]) -> Layout {
        let path =
            std::env::temp_dir().join(format!("klapz-scanner-mp4-{}-{name}", std::process::id()));
        File::create(&path).unwrap().write_all(bytes).unwrap();
        let found = layout(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        found
    }

    fn moov(chunk_offsets: [u8; 4]) -> Vec<u8> {
        let stbl = mp4_box(*b"stbl", &mp4_box(chunk_offsets, &[0; 8]));
        let minf = mp4_box(*b"minf", &stbl);
        let mdia = mp4_box(*b"mdia", &minf);
        mp4_box(*b"moov", &mp4_box(*b"trak", &mdia))
    }

    #[test]
    fn index_first_is_optimized_for_streaming() {
        let mut file = mp4_box(*b"ftyp", b"isom");
        file.extend(moov(*b"stco"));
        file.extend(mp4_box(*b"mdat", &[0; 16]));
        assert_eq!(
            layout_of("first", &file),
            Layout {
                optimized_for_streaming: true,
                has_64bit_offsets: false
            }
        );
    }

    #[test]
    fn index_last_with_large_offsets() {
        let mut file = mp4_box(*b"ftyp", b"isom");
        file.extend(mp4_box(*b"mdat", &[0; 16]));
        file.extend(moov(*b"co64"));
        assert_eq!(
            layout_of("last", &file),
            Layout {
                optimized_for_streaming: false,
                has_64bit_offsets: true
            }
        );
    }

    #[test]
    fn stops_at_a_box_claiming_more_than_the_file() {
        let mut file = mp4_box(*b"ftyp", b"isom");
        file.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);
        file.extend_from_slice(b"moov");
        assert_eq!(layout_of("truncated", &file), Layout::default());
    }
}
