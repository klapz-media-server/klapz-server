use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::UNIX_EPOCH;

use crate::report::FileIdentity;
use crate::ScanError;

/// `OpenSubtitles` hashes this much at each end of the file.
const CHUNK: u64 = 64 * 1024;

/// Size, modification time and `OpenSubtitles` hash: what tells whether a file is one already
/// known, without reading it whole.
///
/// # Errors
///
/// [`ScanError::File`] when the file cannot be opened or read.
pub fn identify(path: &Path) -> Result<FileIdentity, ScanError> {
    let fail = |source| ScanError::File {
        path: path.to_owned(),
        source,
    };
    let mut file = File::open(path).map_err(fail)?;
    let metadata = file.metadata().map_err(fail)?;
    let modified_at = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|elapsed| i64::try_from(elapsed.as_secs()).ok())
        .unwrap_or(0);
    let size = metadata.len();
    let hash = open_subtitles_hash(&mut file, size).map_err(fail)?;
    Ok(FileIdentity {
        size,
        modified_at,
        open_subtitles_hash: format!("{hash:016x}"),
    })
}

/// The size plus every little-endian 64-bit word of the first and last 64 KiB, wrapping. A
/// file shorter than 128 KiB has its bytes counted twice where the two ends overlap, as the
/// reference implementation does.
fn open_subtitles_hash(file: &mut File, size: u64) -> std::io::Result<u64> {
    let mut hash = size;
    for start in [0, size.saturating_sub(CHUNK)] {
        file.seek(SeekFrom::Start(start))?;
        let mut chunk = Vec::new();
        file.by_ref().take(CHUNK).read_to_end(&mut chunk)?;
        hash = chunk.chunks(8).fold(hash, |hash, word| {
            let mut bytes = [0; 8];
            bytes[..word.len()].copy_from_slice(word);
            hash.wrapping_add(u64::from_le_bytes(bytes))
        });
    }
    Ok(hash)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use std::io::Write;

    fn hash_of(name: &str, bytes: &[u8]) -> u64 {
        let path =
            std::env::temp_dir().join(format!("klapz-scanner-hash-{}-{name}", std::process::id()));
        File::create(&path).unwrap().write_all(bytes).unwrap();
        let mut file = File::open(&path).unwrap();
        let hash = open_subtitles_hash(&mut file, bytes.len() as u64).unwrap();
        std::fs::remove_file(&path).unwrap();
        hash
    }

    #[test]
    fn hashes_a_file_of_zeros_as_its_size() {
        assert_eq!(hash_of("zeros", &vec![0; 200_000]), 200_000);
    }

    #[test]
    fn counts_the_words_at_both_ends() {
        let mut bytes = vec![0; 200_000];
        bytes[0] = 1;
        bytes[199_999 - 7] = 2;
        // The low byte of the last 64 KiB's last word.
        assert_eq!(hash_of("ends", &bytes), 200_000 + 1 + 2);
    }
}
