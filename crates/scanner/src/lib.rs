//! Reading media files: what they hold, and what is derived from their pictures and sound.
//!
//! The server opens content nobody vouches for, so nothing here parses a file's bytes or
//! decodes anything: ffprobe and ffmpeg-plex do, as children of the server, which the patched
//! ffmpeg makes enough — it states the MP4 layout, the MPEG-TS packet length, H.264's sequence
//! header and Matroska's declared languages, and computes Chromaprint fingerprints. What is
//! read here is their output, and the files they write into a directory the caller owns
//! (frames, subtitles, the BIF packed from its frames). Each read handles one file, keeps no
//! state and reaches no network: walking libraries, deciding what to redo and storing results
//! are the scan's.

pub mod report;
pub mod transcoder;

#[cfg(feature = "engine")]
mod engine;

#[cfg(feature = "engine")]
pub use engine::*;
