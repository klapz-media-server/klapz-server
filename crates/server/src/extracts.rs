//! What the analysis makes out of a file's content — its subtitle tracks, its seeking
//! thumbnails — kept on disk by the file's content hash, so that every copy of a file shares
//! them and none is made twice:
//!
//! ```text
//! {KLAPZ_EXTRACTS_DIR}/{hash[..2]}/{hash}/index-sd.bif
//! {KLAPZ_EXTRACTS_DIR}/{hash[..2]}/{hash}/subtitles/stream-{track}.{extension}
//! ```
//!
//! Each is made in a working directory beside where it goes, and moved there once whole: what
//! is found in place is complete, and a run stopped halfway — dropped, or the server killed —
//! leaves a working directory that the next start removes.

use std::future::Future;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use klapz_core::files::FileLocation;
use klapz_core::scan::{AnalyzeError, ContentHash, Extraction, MediaExtractor};
use klapz_scanner::Tools;

use crate::analyzer::{analyze_error, TimedOut};

/// Past this, extracting subtitles is dropped and its process killed: it reads the whole file,
/// as measuring its packets does.
const SUBTITLES_TIMEOUT: Duration = Duration::from_secs(600);

/// Past this, making thumbnails is dropped and its process killed: it decodes every key frame,
/// which a long film over a slow share takes a while to.
const THUMBNAILS_TIMEOUT: Duration = Duration::from_secs(1800);

/// The reference's `GenerateBIFFrameInterval`, in seconds.
const THUMBNAIL_INTERVAL_S: u32 = 2;

/// Where the thumbnails go in a content's directory: the name the reference gives its one
/// quality of index.
const PREVIEW_INDEX: &str = "index-sd.bif";

/// Where the subtitle tracks go in a content's directory.
const SUBTITLES: &str = "subtitles";

/// What a working directory's name ends with.
const WORKING: &str = ".partial";

#[derive(Debug)]
pub struct Extracts {
    root: PathBuf,
    tools: Tools,
    /// Tells apart the working directories of one process.
    runs: AtomicU64,
}

impl Extracts {
    /// The ffmpeg the server streams with, keeping what it makes below `root`.
    pub fn from_env(root: PathBuf) -> Self {
        Self {
            root,
            tools: Tools::from_env(),
            runs: AtomicU64::new(0),
        }
    }

    /// Removes the working directories a run that never ended left, two levels below the
    /// root, where they are made. What cannot be removed is left for the next start.
    pub async fn remove_leftovers(&self) {
        let root = self.root.clone();
        let removed = tokio::task::spawn_blocking(move || {
            let mut removed = 0_usize;
            for prefix in read_dirs(&root) {
                for content in read_dirs(&prefix) {
                    for working in read_dirs(&content) {
                        let is_working = working
                            .file_name()
                            .is_some_and(|name| name.to_string_lossy().ends_with(WORKING));
                        if is_working && std::fs::remove_dir_all(&working).is_ok() {
                            removed += 1;
                        }
                    }
                }
            }
            removed
        })
        .await
        .unwrap_or(0);
        if removed > 0 {
            tracing::info!(removed, "removed what unfinished extractions left");
        }
    }

    fn content_directory(&self, hash: &ContentHash) -> PathBuf {
        let hash = hash.as_str();
        self.root.join(&hash[..2]).join(hash)
    }

    /// Runs `make` into a working directory of its own unless `name` is kept for this content
    /// already, then moves `from` — what it made, below the working directory — to `name`.
    async fn keep<F, Fut>(
        &self,
        hash: &ContentHash,
        name: &str,
        from: &str,
        timeout: Duration,
        make: F,
    ) -> Result<Extraction, AnalyzeError>
    where
        F: FnOnce(PathBuf) -> Fut,
        Fut: Future<Output = Result<(), klapz_scanner::ScanError>>,
    {
        let directory = self.content_directory(hash);
        let target = directory.join(name);
        if tokio::fs::try_exists(&target).await.unwrap_or(false) {
            return Ok(Extraction::AlreadyKept);
        }
        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(storage)?;
        let run = self.runs.fetch_add(1, Ordering::Relaxed);
        let working = directory.join(format!(".{name}-{}-{run}{WORKING}", std::process::id()));
        tokio::fs::create_dir(&working).await.map_err(storage)?;
        // Removed however the run ends, dropped included.
        let _cleanup = RemoveOnDrop(working.clone());
        tokio::time::timeout(timeout, make(working.clone()))
            .await
            .map_err(|_| AnalyzeError::Failed(Box::new(TimedOut(timeout))))?
            .map_err(analyze_error)?;
        let result = if from.is_empty() {
            working
        } else {
            working.join(from)
        };
        match tokio::fs::rename(&result, &target).await {
            Ok(()) => Ok(Extraction::Written),
            // A copy of the file, read meanwhile, put its own there first.
            Err(_) if tokio::fs::try_exists(&target).await.unwrap_or(false) => {
                Ok(Extraction::AlreadyKept)
            }
            Err(error) => Err(storage(error)),
        }
    }
}

#[async_trait::async_trait]
impl MediaExtractor for Extracts {
    async fn extract_subtitles(
        &self,
        file: &FileLocation,
        hash: &ContentHash,
    ) -> Result<Extraction, AnalyzeError> {
        let FileLocation::Local(path) = file;
        // The working directory itself becomes `subtitles`.
        self.keep(hash, SUBTITLES, "", SUBTITLES_TIMEOUT, |out| async move {
            klapz_scanner::extract_subtitles(&self.tools, path, &out)
                .await
                .map(drop)
        })
        .await
    }

    async fn generate_preview_thumbnails(
        &self,
        file: &FileLocation,
        hash: &ContentHash,
    ) -> Result<Extraction, AnalyzeError> {
        let FileLocation::Local(path) = file;
        self.keep(
            hash,
            PREVIEW_INDEX,
            PREVIEW_INDEX,
            THUMBNAILS_TIMEOUT,
            |out| async move {
                klapz_scanner::preview_index(&self.tools, path, THUMBNAIL_INTERVAL_S, &out)
                    .await
                    .map(drop)
            },
        )
        .await
    }
}

/// The server's own disk failed, not the file: the file's task alone fails.
fn storage(error: std::io::Error) -> AnalyzeError {
    AnalyzeError::Failed(Box::new(error))
}

fn read_dirs(directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect()
}

struct RemoveOnDrop(PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        // Moved into place already, or never written to: either way, nothing is lost.
        if let Err(error) = std::fs::remove_dir_all(&self.0) {
            if error.kind() != ErrorKind::NotFound {
                tracing::warn!(path = %self.0.display(), %error, "cannot remove a working directory");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    const HASH: &str = "0123456789abcdef0123456789abcdef01234567";

    fn extracts(name: &str) -> (Extracts, PathBuf) {
        let root =
            std::env::temp_dir().join(format!("klapz-extracts-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let extracts = Extracts {
            root: root.clone(),
            tools: Tools {
                ffmpeg: "klapz-test-no-such-ffmpeg".into(),
                ffprobe: None,
            },
            runs: AtomicU64::new(0),
        };
        (extracts, root)
    }

    fn hash() -> ContentHash {
        ContentHash::parse(HASH).unwrap()
    }

    #[tokio::test]
    async fn what_is_kept_already_is_not_made_again() {
        let (extracts, root) = extracts("kept");
        let content = root.join("01").join(HASH);
        std::fs::create_dir_all(content.join(SUBTITLES)).unwrap();
        std::fs::write(content.join(PREVIEW_INDEX), b"BIF").unwrap();
        let file = FileLocation::Local(root.join("missing.mkv"));

        assert_eq!(
            extracts.extract_subtitles(&file, &hash()).await.unwrap(),
            Extraction::AlreadyKept
        );
        assert_eq!(
            extracts
                .generate_preview_thumbnails(&file, &hash())
                .await
                .unwrap(),
            Extraction::AlreadyKept,
            "no tool ran: none can here"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[tokio::test]
    async fn a_run_that_fails_leaves_nothing_in_place() {
        let (extracts, root) = extracts("failed");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("film.mkv");
        std::fs::write(&path, b"not media").unwrap();
        let file = FileLocation::Local(path);
        let made = extracts.extract_subtitles(&file, &hash()).await;
        assert!(
            matches!(made, Err(AnalyzeError::Unavailable(_))),
            "{made:?}"
        );
        let content = root.join("01").join(HASH);
        assert_eq!(
            std::fs::read_dir(&content).unwrap().count(),
            0,
            "neither the result nor its working directory"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[tokio::test]
    async fn a_start_removes_the_working_directories_left_behind() {
        let (extracts, root) = extracts("leftovers");
        let content = root.join("01").join(HASH);
        let left = content.join(format!(".subtitles-1-0{WORKING}"));
        std::fs::create_dir_all(left.join("nested")).unwrap();
        std::fs::create_dir_all(content.join(SUBTITLES)).unwrap();
        extracts.remove_leftovers().await;
        assert!(!left.exists());
        assert!(content.join(SUBTITLES).exists(), "what is kept stays");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
