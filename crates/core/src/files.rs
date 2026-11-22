//! Where a library's files live, behind one port.
//!
//! A library root is a string the storage backend understands: a local directory today; a
//! share or a bucket tomorrow. Everything past the root is a [`RelativePath`], so nothing above
//! this port builds a filesystem path, and a backend that is not a filesystem fits without the
//! scan noticing.

use std::fmt;
use std::path::PathBuf;

/// A path below a library root: `/`-separated names, none empty, `.` or `..`, none holding a
/// separator. Built only through [`RelativePath::parse`] and [`RelativePath::join`], so a value
/// of this type cannot climb out of its root.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RelativePath(String);

impl RelativePath {
    /// The root itself.
    pub fn root() -> Self {
        Self::default()
    }

    /// # Errors
    ///
    /// [`FilesError::Refused`] for an empty, `.` or `..` component, or one holding `\` or a
    /// NUL.
    pub fn parse(path: &str) -> Result<Self, FilesError> {
        if path.is_empty() {
            return Ok(Self::root());
        }
        for name in path.split('/') {
            check_name(name)?;
        }
        Ok(Self(path.to_owned()))
    }

    /// This path with `name` below it.
    ///
    /// # Errors
    ///
    /// [`FilesError::Refused`] when `name` is not a single plain name.
    pub fn join(&self, name: &str) -> Result<Self, FilesError> {
        check_name(name)?;
        Ok(if self.0.is_empty() {
            Self(name.to_owned())
        } else {
            Self(format!("{}/{name}", self.0))
        })
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    /// The names from the root down, the file's own last.
    pub fn components(&self) -> impl Iterator<Item = &str> {
        self.0.split('/').filter(|name| !name.is_empty())
    }

    /// The last name, the root having none.
    pub fn name(&self) -> Option<&str> {
        self.components().last()
    }

    /// The directory holding this path, the root for a top-level name.
    #[must_use]
    pub fn parent(&self) -> Self {
        match self.0.rsplit_once('/') {
            Some((parent, _)) => Self(parent.to_owned()),
            None => Self::root(),
        }
    }

    /// Whether `self` is `ancestor` or lies below it.
    pub fn starts_with(&self, ancestor: &Self) -> bool {
        ancestor.is_root()
            || self.0 == ancestor.0
            || (self.0.starts_with(&ancestor.0)
                && self.0.as_bytes().get(ancestor.0.len()) == Some(&b'/'))
    }
}

impl fmt::Display for RelativePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

fn check_name(name: &str) -> Result<(), FilesError> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', '\0']) {
        Err(FilesError::Refused)
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Directory,
}

/// One name in a directory listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub kind: EntryKind,
    /// Bytes; zero for a directory.
    pub size: u64,
    /// Unix seconds.
    pub modified_at: i64,
}

/// A directory an owner may pick as a library root, its path in the form
/// [`LibraryFiles::resolve_root`] gives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Directory {
    pub name: String,
    pub path: String,
}

/// The directories inside one, for an owner looking for a library root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryListing {
    /// The directory listed, resolved as a root is.
    pub path: String,
    /// The directory holding it; `None` at the top of a drive or of the filesystem.
    pub parent: Option<String>,
    /// In no particular order.
    pub directories: Vec<Directory>,
    /// The directory held more than the limit asked for, and the rest were not read.
    pub truncated: bool,
}

/// How many directories one listing holds at most: a directory of a million names must not
/// become a million strings in memory because an owner opened it.
pub const MAX_LISTED_DIRECTORIES: usize = 10_000;

/// What a media tool is handed to read a file. Only local paths exist today; a backend that is
/// not a filesystem will hand out an address the server serves instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileLocation {
    Local(PathBuf),
}

#[derive(Debug, thiserror::Error)]
pub enum FilesError {
    /// The root cannot be reached at all: an unmounted disk, a share that is down. A scan must
    /// then leave the root's items alone rather than take them for deleted.
    #[error("the library root cannot be reached")]
    Unavailable,
    #[error("no such file or directory")]
    NotFound,
    /// The path would leave its root, or names something that is not a plain file or
    /// directory.
    #[error("the path is not one a library may reach")]
    Refused,
    /// It exists, and the server is not allowed to read it.
    #[error("the server is not allowed to read it")]
    Unreadable,
    #[error("the file is larger than the {0} bytes read")]
    TooLarge(u64),
    #[error("reading library files failed")]
    Io(#[source] Box<dyn std::error::Error + Send + Sync>),
}

/// Storage port for library files. Implementations decide which roots they serve; a root none
/// serves answers [`FilesError::Unavailable`].
#[async_trait::async_trait]
pub trait LibraryFiles: Send + Sync + std::fmt::Debug {
    /// The entries of `directory` under `root`, files and directories alike, in no particular
    /// order. Links and special files are left out.
    async fn list(&self, root: &str, directory: &RelativePath) -> Result<Vec<Entry>, FilesError>;

    /// A small file read whole, such as `.plexignore`.
    ///
    /// # Errors
    ///
    /// [`FilesError::TooLarge`] past `limit` bytes.
    async fn read(
        &self,
        root: &str,
        file: &RelativePath,
        limit: u64,
    ) -> Result<Vec<u8>, FilesError>;

    /// The path a part records for `file`, what the API publishes as the file's location.
    fn stored_path(&self, root: &str, file: &RelativePath) -> String;

    /// The file a recorded path names under `root`, the inverse of [`Self::stored_path`]:
    /// `None` when it is not below `root`.
    fn relative_path(&self, root: &str, stored: &str) -> Option<RelativePath>;

    /// Where a media tool reads `file`.
    async fn locate(&self, root: &str, file: &RelativePath) -> Result<FileLocation, FilesError>;

    /// The root `path` names, in the one form this backend stores and compares roots in.
    ///
    /// # Errors
    ///
    /// [`FilesError::NotFound`] when nothing is there, [`FilesError::Refused`] when `path` is
    /// not one a root can be: relative, or not a directory.
    async fn resolve_root(&self, path: &str) -> Result<String, FilesError>;

    /// Where an owner looking for a root starts: the drives of a Windows machine, `/`
    /// elsewhere. In no particular order.
    async fn starting_points(&self) -> Result<Vec<Directory>, FilesError>;

    /// The directories inside `path`, which is resolved exactly as [`Self::resolve_root`]
    /// resolves a root before anything is read. Files and hidden directories are left out, and
    /// at most `limit` directories are read.
    ///
    /// This and [`Self::starting_points`] are how an owner finds a root without typing it:
    /// they name directories and never read a file.
    ///
    /// # Errors
    ///
    /// As [`Self::resolve_root`], and [`FilesError::Unreadable`] when the directory may not be
    /// read.
    async fn list_directories(
        &self,
        path: &str,
        limit: usize,
    ) -> Result<DirectoryListing, FilesError>;
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn a_relative_path_cannot_climb() {
        for path in ["..", "a/../b", "a//b", "a/./b", "a\\b", "/a", "a/"] {
            assert!(RelativePath::parse(path).is_err(), "{path}");
        }
        assert!(RelativePath::root().join("..").is_err());
        assert!(RelativePath::root().join("a/b").is_err());
        let path = RelativePath::parse("Films/Soul (2020)").unwrap();
        assert_eq!(
            path.join("Soul.mkv").unwrap().as_str(),
            "Films/Soul (2020)/Soul.mkv"
        );
    }

    #[test]
    fn walks_up_and_compares_whole_names() {
        let file = RelativePath::parse("Series/Bull/S05/e01.mkv").unwrap();
        assert_eq!(file.parent().as_str(), "Series/Bull/S05");
        assert_eq!(
            RelativePath::parse("a").unwrap().parent(),
            RelativePath::root()
        );
        assert!(file.starts_with(&RelativePath::parse("Series/Bull").unwrap()));
        assert!(!file.starts_with(&RelativePath::parse("Series/Bu").unwrap()));
        assert!(file.starts_with(&RelativePath::root()));
        assert_eq!(file.name(), Some("e01.mkv"));
    }
}
