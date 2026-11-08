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
