//! Library files on a local disk: the one [`LibraryFiles`] backend so far.

use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use klapz_core::files::{Entry, EntryKind, FileLocation, FilesError, LibraryFiles, RelativePath};
use tokio::io::AsyncReadExt;

/// Roots are directories of this machine, as `section_root.path` holds them.
#[derive(Debug, Default)]
pub struct LocalFiles;

impl LocalFiles {
    /// `file` under `root`. Each name must be a plain one here too: on Windows a name such as
    /// `C:` is a drive, which `join` would jump to.
    fn path(root: &str, file: &RelativePath) -> Result<PathBuf, FilesError> {
        let mut path = PathBuf::from(root);
        for name in file.components() {
            let mut parts = Path::new(name).components();
            match (parts.next(), parts.next()) {
                (Some(Component::Normal(_)), None) => path.push(name),
                _ => return Err(FilesError::Refused),
            }
        }
        Ok(path)
    }
}

fn io_error(error: std::io::Error) -> FilesError {
    match error.kind() {
        std::io::ErrorKind::NotFound => FilesError::NotFound,
        _ => FilesError::Io(Box::new(error)),
    }
}

fn seconds(metadata: &std::fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|elapsed| i64::try_from(elapsed.as_secs()).ok())
        .unwrap_or(0)
}

#[async_trait::async_trait]
impl LibraryFiles for LocalFiles {
    async fn list(&self, root: &str, directory: &RelativePath) -> Result<Vec<Entry>, FilesError> {
        // An absent root is an unmounted disk or a share that is down, never an empty library.
        if !tokio::fs::metadata(root)
            .await
            .is_ok_and(|metadata| metadata.is_dir())
        {
            return Err(FilesError::Unavailable);
        }
        let mut reading = tokio::fs::read_dir(Self::path(root, directory)?)
            .await
            .map_err(io_error)?;
        let mut entries = Vec::new();
        while let Some(entry) = reading.next_entry().await.map_err(io_error)? {
            // Not followed: a link could lead out of the library.
            let Ok(metadata) = tokio::fs::symlink_metadata(entry.path()).await else {
                continue;
            };
            let kind = if metadata.is_file() {
                EntryKind::File
            } else if metadata.is_dir() {
                EntryKind::Directory
            } else {
                continue;
            };
            let Ok(name) = entry.file_name().into_string() else {
                tracing::warn!(path = %entry.path().display(), "skipping a name that is not Unicode");
                continue;
            };
            entries.push(Entry {
                name,
                kind,
                size: if kind == EntryKind::File {
                    metadata.len()
                } else {
                    0
                },
                modified_at: seconds(&metadata),
            });
        }
        Ok(entries)
    }

    async fn read(
        &self,
        root: &str,
        file: &RelativePath,
        limit: u64,
    ) -> Result<Vec<u8>, FilesError> {
        let path = Self::path(root, file)?;
        let metadata = tokio::fs::symlink_metadata(&path).await.map_err(io_error)?;
        if !metadata.is_file() {
            return Err(FilesError::Refused);
        }
        if metadata.len() > limit {
            return Err(FilesError::TooLarge(limit));
        }
        let mut bytes = Vec::new();
        tokio::fs::File::open(&path)
            .await
            .map_err(io_error)?
            .take(limit)
            .read_to_end(&mut bytes)
            .await
            .map_err(io_error)?;
        Ok(bytes)
    }

    fn stored_path(&self, root: &str, file: &RelativePath) -> String {
        let mut path = PathBuf::from(root);
        path.extend(file.components());
        path.to_string_lossy().into_owned()
    }

    fn relative_path(&self, root: &str, stored: &str) -> Option<RelativePath> {
        let inner = Path::new(stored).strip_prefix(root).ok()?;
        let mut path = RelativePath::root();
        for component in inner.components() {
            let Component::Normal(name) = component else {
                return None;
            };
            path = path.join(name.to_str()?).ok()?;
        }
        Some(path)
    }

    async fn locate(&self, root: &str, file: &RelativePath) -> Result<FileLocation, FilesError> {
        Ok(FileLocation::Local(Self::path(root, file)?))
    }

    /// Canonical, as the scan and the media engine compare paths with it, and absolute: a
    /// relative path would name a directory below wherever the server was started.
    async fn resolve_root(&self, path: &str) -> Result<String, FilesError> {
        if path.is_empty() || !Path::new(path).is_absolute() {
            return Err(FilesError::Refused);
        }
        let canonical = tokio::fs::canonicalize(path).await.map_err(io_error)?;
        if !tokio::fs::metadata(&canonical)
            .await
            .map_err(io_error)?
            .is_dir()
        {
            return Err(FilesError::Refused);
        }
        Ok(displayed(&canonical))
    }
}

/// A canonical path as a person writes it. Windows canonicalizes `C:\Films` to the verbatim
/// `\\?\C:\Films`, and a mapped drive to its share, `\\?\UNC\nas\films`, which every API would
/// then publish as the files' location.
fn displayed(path: &std::path::Path) -> String {
    let text = path.to_string_lossy();
    if let Some(share) = text.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{share}");
    }
    match text.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => rest.to_owned(),
        _ => text.into_owned(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("klapz-files-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(path.join("Films/Soul (2020)")).unwrap();
            std::fs::write(path.join("Films/Soul (2020)/Soul.mkv"), b"12345").unwrap();
            Self(path)
        }

        fn root(&self) -> String {
            self.0.to_string_lossy().into_owned()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn lists_files_and_directories() {
        let scratch = Scratch::new("list");
        let local = LocalFiles;
        let films = RelativePath::parse("Films").unwrap();
        let entries = local.list(&scratch.root(), &films).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, EntryKind::Directory);
        let soul = films.join("Soul (2020)").unwrap();
        let entries = local.list(&scratch.root(), &soul).await.unwrap();
        assert_eq!((entries[0].name.as_str(), entries[0].size), ("Soul.mkv", 5));
    }

    #[tokio::test]
    async fn a_missing_root_is_unavailable_and_a_missing_directory_not_found() {
        let scratch = Scratch::new("missing");
        let files = LocalFiles;
        let gone = format!("{}-gone", scratch.root());
        assert!(matches!(
            files.list(&gone, &RelativePath::root()).await,
            Err(FilesError::Unavailable)
        ));
        let absent = RelativePath::parse("Series").unwrap();
        assert!(matches!(
            files.list(&scratch.root(), &absent).await,
            Err(FilesError::NotFound)
        ));
    }

    #[tokio::test]
    async fn stored_paths_round_trip() {
        let scratch = Scratch::new("paths");
        let files = LocalFiles;
        let file = RelativePath::parse("Films/Soul (2020)/Soul.mkv").unwrap();
        let stored = files.stored_path(&scratch.root(), &file);
        assert!(Path::new(&stored).is_file());
        assert_eq!(files.relative_path(&scratch.root(), &stored), Some(file));
        assert_eq!(
            files.relative_path(&scratch.root(), "/elsewhere/Soul.mkv"),
            None
        );
    }

    #[tokio::test]
    async fn reads_a_small_file_and_refuses_a_large_one() {
        let scratch = Scratch::new("read");
        let files = LocalFiles;
        let file = RelativePath::parse("Films/Soul (2020)/Soul.mkv").unwrap();
        assert_eq!(
            files.read(&scratch.root(), &file, 10).await.unwrap(),
            b"12345"
        );
        assert!(matches!(
            files.read(&scratch.root(), &file, 4).await,
            Err(FilesError::TooLarge(4))
        ));
    }

    #[cfg(windows)]
    #[test]
    fn a_drive_is_not_a_name() {
        let file = RelativePath::parse("C:").unwrap();
        assert!(matches!(
            LocalFiles::path("D:\\media", &file),
            Err(FilesError::Refused)
        ));
    }

    #[test]
    fn drops_the_verbatim_prefix_of_a_drive_or_a_share() {
        assert_eq!(
            displayed(std::path::Path::new(r"\\?\C:\Films")),
            r"C:\Films"
        );
        assert_eq!(
            displayed(std::path::Path::new(r"\\?\UNC\nas\films")),
            r"\\nas\films"
        );
        assert_eq!(
            displayed(std::path::Path::new(r"\\?\GLOBALROOT\Device\Films")),
            r"\\?\GLOBALROOT\Device\Films"
        );
        assert_eq!(displayed(std::path::Path::new("/srv/films")), "/srv/films");
    }

    #[tokio::test]
    async fn a_root_is_an_absolute_directory_in_its_canonical_form() {
        let scratch = Scratch::new("roots");
        let local = LocalFiles;
        let films = scratch.0.join("Films");
        let spelled = films.join("Soul (2020)").join("..");
        assert_eq!(
            local
                .resolve_root(&spelled.to_string_lossy())
                .await
                .unwrap(),
            displayed(&std::fs::canonicalize(&films).unwrap())
        );
        let file = films.join("Soul (2020)").join("Soul.mkv");
        assert!(matches!(
            local.resolve_root(&file.to_string_lossy()).await,
            Err(FilesError::Refused)
        ));
        assert!(matches!(
            local.resolve_root("Films").await,
            Err(FilesError::Refused)
        ));
        assert!(matches!(
            local
                .resolve_root(&scratch.0.join("gone").to_string_lossy())
                .await,
            Err(FilesError::NotFound)
        ));
    }
}
