//! Reading media files: what they hold, and what is derived from their pictures and sound.
//!
//! The scanner is to library files what the transcoder is to playback: it opens content nobody
//! vouches for, so it runs as a process of its own, and decodes nothing itself — ffprobe and
//! ffmpeg do, as children. One run handles one file and prints one JSON [`report::Report`];
//! files it makes (a BIF, frames, subtitles) go into a directory the caller owns. It keeps no
//! state and reaches no network: walking libraries, deciding what to redo and storing results
//! are the server's.

pub mod report;
pub mod transcoder;

#[cfg(feature = "engine")]
mod engine;

#[cfg(feature = "engine")]
pub use engine::*;
