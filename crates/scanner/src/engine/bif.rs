//! Preview thumbnails for seeking: Roku's BIF, which Plex clients read.

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::engine::{input, output_directory, produced, run, ScanError, Tools};
use crate::report::{FileRole, Produced};

const MAGIC: [u8; 8] = [0x89, 0x42, 0x49, 0x46, 0x0D, 0x0A, 0x1A, 0x0A];
const HEADER_LENGTH: usize = 64;

/// The name the reference gives its one quality of index.
const NAME: &str = "index-sd.bif";

/// Draws a frame every `interval_s` seconds with the reference's own command — key frames
/// only, fitted inside 320×240, JPEG quality 3 — and packs them into `out/index-sd.bif`.
///
/// # Errors
///
/// [`ScanError::Unreadable`] when ffmpeg cannot decode the video, [`ScanError::File`] when the
/// frames cannot be read back or the index written.
pub fn preview_index(
    tools: &Tools,
    path: &Path,
    interval_s: u32,
    out: &Path,
) -> Result<Produced, ScanError> {
    let directory = output_directory(out)?;
    let frames = directory.join("bif-frames");
    fs::create_dir(&frames).map_err(|source| ScanError::File {
        path: frames.clone(),
        source,
    })?;
    let result = extract_and_pack(tools, path, interval_s, &directory, &frames);
    // The frames are an intermediate; whatever happened, they do not stay behind.
    let _ = fs::remove_dir_all(&frames);
    let count = result?;
    Ok(Produced {
        files: vec![produced(
            &directory,
            NAME.to_owned(),
            FileRole::PreviewIndex {
                frames: count,
                interval_ms: interval_s * 1000,
            },
        )?],
    })
}

fn extract_and_pack(
    tools: &Tools,
    path: &Path,
    interval_s: u32,
    directory: &Path,
    frames: &Path,
) -> Result<u32, ScanError> {
    let filter = format!(
        "[0:V:0] fps=fps={:.6}:round=up,scale=w=320:h=240:force_original_aspect_ratio=decrease [out]",
        1.0 / f64::from(interval_s)
    );
    run(
        "ffmpeg",
        Command::new(&tools.ffmpeg)
            .args([
                "-hide_banner",
                "-nostats",
                "-loglevel",
                "error",
                "-skip_frame:v",
                "nokey",
                "-i",
            ])
            .arg(input(path))
            .args([
                "-threads",
                "0",
                "-q",
                "3",
                "-filter_complex",
                &filter,
                "-map",
                "[out]",
            ])
            .arg(input(&frames.join("img-%06d.jpg"))),
    )?;
    let mut images: Vec<PathBuf> = fs::read_dir(frames)
        .map_err(|source| ScanError::File {
            path: frames.to_owned(),
            source,
        })?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    // ffmpeg numbers them with leading zeros, so the names sort in frame order.
    images.sort();
    let target = directory.join(NAME);
    pack(&images, interval_s, &target).map_err(|source| ScanError::File {
        path: target,
        source,
    })
}

/// Writes the BIF: a 64-byte header, an index of `(timestamp, offset)` pairs closed by
/// `(0xFFFFFFFF, end)`, then the JPEGs back to back. Timestamps are in the header's unit,
/// which a separation of 0 makes one second, as the reference writes it.
fn pack(images: &[PathBuf], interval_s: u32, target: &Path) -> std::io::Result<u32> {
    let too_large =
        || std::io::Error::new(std::io::ErrorKind::InvalidData, "a BIF is limited to 4 GiB");
    let count = u32::try_from(images.len()).map_err(|_| too_large())?;
    let mut sizes = Vec::with_capacity(images.len());
    for image in images {
        sizes.push(fs::metadata(image)?.len());
    }
    let mut offset = (HEADER_LENGTH + (images.len() + 1) * 8) as u64;
    let mut index = Vec::with_capacity((images.len() + 1) * 8);
    for (number, size) in (0..count).zip(&sizes) {
        index.extend_from_slice(&(number * interval_s).to_le_bytes());
        index.extend_from_slice(
            &u32::try_from(offset)
                .map_err(|_| too_large())?
                .to_le_bytes(),
        );
        offset += size;
    }
    index.extend_from_slice(&u32::MAX.to_le_bytes());
    index.extend_from_slice(
        &u32::try_from(offset)
            .map_err(|_| too_large())?
            .to_le_bytes(),
    );

    // `create_new`: a file already there was put by someone else, and is not overwritten.
    let mut file = BufWriter::new(File::options().write(true).create_new(true).open(target)?);
    let mut header = [0; HEADER_LENGTH];
    header[..8].copy_from_slice(&MAGIC);
    header[12..16].copy_from_slice(&count.to_le_bytes());
    file.write_all(&header)?;
    file.write_all(&index)?;
    for image in images {
        file.write_all(&fs::read(image)?)?;
    }
    file.flush()?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn packs_the_reference_layout() {
        let directory =
            std::env::temp_dir().join(format!("open-plex-scanner-bif-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let first = directory.join("img-000001.jpg");
        let second = directory.join("img-000002.jpg");
        fs::write(&first, [1, 2, 3]).unwrap();
        fs::write(&second, [4, 5]).unwrap();
        let target = directory.join(NAME);
        let _ = fs::remove_file(&target);

        assert_eq!(pack(&[first, second], 2, &target).unwrap(), 2);
        let bytes = fs::read(&target).unwrap();
        fs::remove_dir_all(&directory).unwrap();

        let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        assert_eq!(&bytes[..8], &MAGIC);
        assert_eq!(word(8), 0, "version");
        assert_eq!(word(12), 2, "count");
        assert_eq!(word(16), 0, "separation, meaning one second");
        let data = 64 + 3 * 8;
        let start = u32::try_from(data).unwrap();
        assert_eq!((word(64), word(68)), (0, start));
        assert_eq!((word(72), word(76)), (2, start + 3));
        assert_eq!((word(80), word(84)), (u32::MAX, start + 5));
        assert_eq!(&bytes[data..], &[1, 2, 3, 4, 5]);
    }
}
