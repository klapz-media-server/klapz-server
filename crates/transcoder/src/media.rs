//! Turning a stored path into a file the server may open.

use std::path::{Path, PathBuf};

use crate::TranscodeError;

/// The real location of `path`, provided it lies within one of the library `roots`.
///
/// Stored paths are not client input, but this is the one place a path becomes a disk access,
/// so it holds whatever put it in the database to the same rule. Canonicalizing first is what
/// makes the comparison sound: it resolves `..`, separators and symlinks, and `starts_with`
/// compares whole components, so `/media/movies2` does not pass for `/media/movies`.
///
/// # Errors
///
/// [`TranscodeError::MissingFile`] when it does not exist, [`TranscodeError::OutsideLibrary`]
/// when it is not under any root.
pub async fn resolve_within(path: &str, roots: &[String]) -> Result<PathBuf, TranscodeError> {
    let file = tokio::fs::canonicalize(Path::new(path))
        .await
        .map_err(|_| TranscodeError::MissingFile)?;
    for root in roots {
        // A root that does not exist here cannot hold the file.
        if let Ok(root) = tokio::fs::canonicalize(Path::new(root)).await {
            if file.starts_with(&root) {
                return Ok(file);
            }
        }
    }
    Err(TranscodeError::OutsideLibrary)
}

/// The most a sidecar subtitle is read up to. A film's subtitles weigh some hundred
/// kilobytes; a file past this is not one, and is not read into memory.
pub const MAX_SIDECAR_BYTES: u64 = 16 * 1024 * 1024;

/// The real location of a sidecar subtitle of `media`: a file within one of the library
/// `roots`, in the media's directory or below it — where the scanner looks for sidecars — and
/// not the media itself, which a deletion must never reach.
///
/// # Errors
///
/// [`TranscodeError::MissingFile`] when it does not exist or is not a file,
/// [`TranscodeError::OutsideLibrary`] when it is not where a sidecar of the media may be.
pub async fn resolve_sidecar(
    path: &str,
    media: &str,
    roots: &[String],
) -> Result<PathBuf, TranscodeError> {
    let file = resolve_within(path, roots).await?;
    let is_file = tokio::fs::metadata(&file)
        .await
        .is_ok_and(|metadata| metadata.is_file());
    if !is_file {
        return Err(TranscodeError::MissingFile);
    }
    let media = Path::new(media);
    let directory = match media.parent() {
        Some(directory) => tokio::fs::canonicalize(directory)
            .await
            .map_err(|_| TranscodeError::OutsideLibrary)?,
        None => return Err(TranscodeError::OutsideLibrary),
    };
    let is_the_media = tokio::fs::canonicalize(media)
        .await
        .is_ok_and(|media| media == file);
    if file.starts_with(&directory) && !is_the_media {
        Ok(file)
    } else {
        Err(TranscodeError::OutsideLibrary)
    }
}

/// A sidecar's content, which [`resolve_sidecar`] found.
///
/// # Errors
///
/// [`TranscodeError::SidecarTooLarge`] past [`MAX_SIDECAR_BYTES`], [`TranscodeError::Io`]
/// when it cannot be read.
pub async fn read_sidecar(file: &Path) -> Result<Vec<u8>, TranscodeError> {
    use tokio::io::AsyncReadExt as _;

    let opened = tokio::fs::File::open(file)
        .await
        .map_err(TranscodeError::Io)?;
    let mut content = Vec::new();
    // One byte past the limit tells a file at the limit from a larger one.
    opened
        .take(MAX_SIDECAR_BYTES + 1)
        .read_to_end(&mut content)
        .await
        .map_err(TranscodeError::Io)?;
    if content.len() as u64 > MAX_SIDECAR_BYTES {
        return Err(TranscodeError::SidecarTooLarge);
    }
    Ok(content)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "open-plex-media-test-{name}-{}",
                std::process::id()
            ));
            std::fs::create_dir_all(path.join("movies")).unwrap();
            std::fs::create_dir_all(path.join("movies2")).unwrap();
            std::fs::write(path.join("movies").join("film.mkv"), b"x").unwrap();
            std::fs::write(path.join("movies2").join("film.mkv"), b"x").unwrap();
            std::fs::write(path.join("secret.txt"), b"x").unwrap();
            Self(path)
        }

        fn text(&self, tail: &str) -> String {
            self.0.join(tail).display().to_string()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn accepts_a_file_inside_a_root() {
        let scratch = Scratch::new("inside");
        let resolved = resolve_within(&scratch.text("movies/film.mkv"), &[scratch.text("movies")])
            .await
            .unwrap();
        assert!(resolved.ends_with("film.mkv"));
    }

    #[tokio::test]
    async fn refuses_a_sibling_sharing_the_roots_prefix() {
        let scratch = Scratch::new("sibling");
        let outcome =
            resolve_within(&scratch.text("movies2/film.mkv"), &[scratch.text("movies")]).await;
        assert!(matches!(outcome, Err(TranscodeError::OutsideLibrary)));
    }

    #[tokio::test]
    async fn refuses_a_path_climbing_out_of_a_root() {
        let scratch = Scratch::new("climb");
        let outcome = resolve_within(
            &scratch.text("movies/../secret.txt"),
            &[scratch.text("movies")],
        )
        .await;
        assert!(matches!(outcome, Err(TranscodeError::OutsideLibrary)));
    }

    #[tokio::test]
    async fn finds_a_sidecar_beside_its_media_or_below() {
        let scratch = Scratch::new("sidecar");
        std::fs::create_dir_all(scratch.0.join("movies").join("Subs")).unwrap();
        std::fs::write(scratch.0.join("movies").join("film.fr.srt"), b"1").unwrap();
        std::fs::write(
            scratch.0.join("movies").join("Subs").join("film.en.srt"),
            b"2",
        )
        .unwrap();
        let roots = [scratch.text("movies")];
        let media = scratch.text("movies/film.mkv");
        let beside = resolve_sidecar(&scratch.text("movies/film.fr.srt"), &media, &roots)
            .await
            .unwrap();
        assert_eq!(read_sidecar(&beside).await.unwrap(), b"1");
        let below = resolve_sidecar(&scratch.text("movies/Subs/film.en.srt"), &media, &roots)
            .await
            .unwrap();
        assert_eq!(read_sidecar(&below).await.unwrap(), b"2");
    }

    #[tokio::test]
    async fn refuses_a_sidecar_that_is_the_media_or_elsewhere() {
        let scratch = Scratch::new("not-sidecar");
        std::fs::create_dir_all(scratch.0.join("movies").join("other")).unwrap();
        std::fs::write(scratch.0.join("movies").join("other").join("b.srt"), b"x").unwrap();
        let roots = [scratch.text("movies")];
        let media = scratch.text("movies/other/../film.mkv");
        // The media itself: a deletion through it would take the film.
        let itself = resolve_sidecar(&scratch.text("movies/film.mkv"), &media, &roots).await;
        assert!(matches!(itself, Err(TranscodeError::OutsideLibrary)));
        // Another film's directory, beside the media's.
        let media = scratch.text("movies/other/a.mkv");
        let elsewhere = resolve_sidecar(&scratch.text("movies/film.mkv"), &media, &roots).await;
        assert!(matches!(elsewhere, Err(TranscodeError::OutsideLibrary)));
        // Outside every library, though beside the media.
        let outside = resolve_sidecar(
            &scratch.text("secret.txt"),
            &scratch.text("film.mkv"),
            &roots,
        )
        .await;
        assert!(matches!(outside, Err(TranscodeError::OutsideLibrary)));
        // A directory is no file.
        let directory = resolve_sidecar(
            &scratch.text("movies/other"),
            &scratch.text("movies/a.mkv"),
            &roots,
        )
        .await;
        assert!(matches!(directory, Err(TranscodeError::MissingFile)));
    }

    #[tokio::test]
    async fn reports_a_file_that_is_gone() {
        let scratch = Scratch::new("gone");
        let outcome = resolve_within(
            &scratch.text("movies/nothing.mkv"),
            &[scratch.text("movies")],
        )
        .await;
        assert!(matches!(outcome, Err(TranscodeError::MissingFile)));
    }
}
