//! Library files on a local disk: the one [`LibraryFiles`] backend so far.

use std::path::{Component, Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use klapz_core::files::{
    sort_directories, Directory, DirectoryListing, Entry, EntryKind, FileLocation, FilesError,
    FirstDirectories, LibraryFiles, RelativePath,
};
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
        std::io::ErrorKind::PermissionDenied => FilesError::Unreadable,
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
        let found = within(SCANNING_DEADLINE, tokio::fs::metadata(root)).await;
        if !matches!(found, Ok(Ok(metadata)) if metadata.is_dir()) {
            return Err(FilesError::Unavailable);
        }
        let path = Self::path(root, directory)?;
        let mut reading = within(SCANNING_DEADLINE, tokio::fs::read_dir(path))
            .await?
            .map_err(io_error)?;
        let mut entries = Vec::new();
        while let Some(entry) = within(SCANNING_DEADLINE, reading.next_entry())
            .await?
            .map_err(io_error)?
        {
            // Not followed: a link could lead out of the library.
            let Ok(metadata) =
                within(SCANNING_DEADLINE, tokio::fs::symlink_metadata(entry.path())).await?
            else {
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
        within(SCANNING_DEADLINE, async {
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
        })
        .await?
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

    async fn resolve_root(&self, path: &str) -> Result<String, FilesError> {
        Ok(displayed(&canonical_directory(path).await?))
    }

    async fn starting_points(&self) -> Result<Vec<Directory>, FilesError> {
        if cfg!(windows) {
            Ok(drives().await)
        } else {
            Ok(vec![Directory {
                name: "/".to_owned(),
                path: "/".to_owned(),
            }])
        }
    }

    async fn list_directories(
        &self,
        path: &str,
        limit: usize,
    ) -> Result<DirectoryListing, FilesError> {
        let directory = within(BROWSING_DEADLINE, canonical_directory(path)).await??;
        let mut reading = within(BROWSING_DEADLINE, tokio::fs::read_dir(&directory))
            .await?
            .map_err(io_error)?;
        let mut first = FirstDirectories::new(limit);
        while let Some(entry) = within(BROWSING_DEADLINE, reading.next_entry())
            .await?
            .map_err(io_error)?
        {
            // A mount point whose server is gone answers no one: it is left out rather than
            // hold the whole listing.
            if let Ok(Some(name)) = within(BROWSING_DEADLINE, listed_directory(&entry)).await {
                first.offer(Directory {
                    path: displayed(&directory.join(&name)),
                    name,
                });
            }
        }
        let (directories, truncated) = first.finish();
        Ok(DirectoryListing {
            path: displayed(&directory),
            parent: directory.parent().map(displayed),
            directories,
            truncated,
        })
    }
}

/// How long one step of a listing may take. A share whose server is gone can hold a call to
/// the filesystem for minutes; a disk waking up takes a few seconds.
const BROWSING_DEADLINE: Duration = Duration::from_secs(5);

/// The same for a scan, which no one watches: a disk that takes its time to wake must not
/// pass for an unmounted one, whose items the scan would then leave alone, but a share that
/// stopped answering must not hold the library as being scanned until the server restarts.
const SCANNING_DEADLINE: Duration = Duration::from_secs(30);

/// `step`, given up on after `deadline` as [`FilesError::Unavailable`]. The thread it blocks
/// is left to finish on its own.
async fn within<T>(
    deadline: Duration,
    step: impl std::future::Future<Output = T>,
) -> Result<T, FilesError> {
    tokio::time::timeout(deadline, step)
        .await
        .map_err(|_| FilesError::Unavailable)
}

/// The name of `entry` when an owner may pick it: a visible directory, or a link to one.
async fn listed_directory(entry: &tokio::fs::DirEntry) -> Option<String> {
    // Told by the directory itself, so the files, however many, cost no system call.
    let kind = entry.file_type().await.ok()?;
    if kind.is_file() {
        return None;
    }
    // The entry's own attributes, a link's included: a hidden junction such as
    // `Documents and Settings` leads to a directory that is not hidden.
    let own = entry.metadata().await.ok()?;
    let name = entry.file_name().into_string().ok()?;
    if is_hidden(&name, &own) {
        return None;
    }
    // A link to a directory is followed: a share often appears that way
    // (`/media/nas` → `/mnt/nas`), and entering it resolves it as any root.
    let is_directory = if kind.is_symlink() {
        tokio::fs::metadata(entry.path())
            .await
            .is_ok_and(|target| target.is_dir())
    } else {
        own.is_dir()
    };
    is_directory.then_some(name)
}

/// `path` as a root is stored: canonical, as the scan and the media engine compare paths with
/// it, and absolute, as a relative path would name a directory below wherever the server was
/// started.
async fn canonical_directory(path: &str) -> Result<PathBuf, FilesError> {
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
    Ok(canonical)
}

/// What a file manager would not show: a dot name anywhere, and on Windows what is marked
/// hidden or system, such as `$RECYCLE.BIN` and `System Volume Information`.
#[cfg(windows)]
fn is_hidden(name: &str, metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const HIDDEN: u32 = 0x2;
    const SYSTEM: u32 = 0x4;
    name.starts_with('.') || metadata.file_attributes() & (HIDDEN | SYSTEM) != 0
}

#[cfg(not(windows))]
fn is_hidden(name: &str, _metadata: &std::fs::Metadata) -> bool {
    name.starts_with('.')
}

/// The drive letters that answer. A network drive whose server is gone can take a long time
/// to say so, hence the deadline; a reader with no card in it does not answer at all.
async fn drives() -> Vec<Directory> {
    const DEADLINE: Duration = Duration::from_secs(2);
    let mut probes = tokio::task::JoinSet::new();
    for letter in 'A'..='Z' {
        probes.spawn(async move {
            let path = format!("{letter}:\\");
            let answer = within(DEADLINE, tokio::fs::metadata(&path)).await;
            matches!(answer, Ok(Ok(metadata)) if metadata.is_dir()).then(|| Directory {
                name: format!("{letter}:"),
                path,
            })
        });
    }
    let mut drives = Vec::new();
    while let Some(probe) = probes.join_next().await {
        if let Ok(Some(drive)) = probe {
            drives.push(drive);
        }
    }
    sort_directories(&mut drives);
    drives
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

    use std::sync::Arc;

    use klapz_core::scan::{scan_section, ScanOutcome, ScanRequest, Scanner};

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

    fn names(listing: &DirectoryListing) -> Vec<&str> {
        let mut names: Vec<&str> = listing
            .directories
            .iter()
            .map(|directory| directory.name.as_str())
            .collect();
        names.sort_unstable();
        names
    }

    #[tokio::test]
    async fn lists_the_visible_directories_of_a_directory_and_no_file() {
        let scratch = Scratch::new("browse");
        std::fs::create_dir(scratch.0.join("Series")).unwrap();
        std::fs::create_dir(scratch.0.join(".cache")).unwrap();
        std::fs::write(scratch.0.join("notes.txt"), b"").unwrap();
        let local = LocalFiles;
        let listing = local.list_directories(&scratch.root(), 100).await.unwrap();
        assert_eq!(names(&listing), ["Films", "Series"]);
        assert!(!listing.truncated);
        let canonical = std::fs::canonicalize(&scratch.0).unwrap();
        assert_eq!(listing.path, displayed(&canonical));
        assert_eq!(listing.parent, canonical.parent().map(displayed));
        let films = listing
            .directories
            .iter()
            .find(|directory| directory.name == "Films")
            .unwrap();
        assert_eq!(films.path, displayed(&canonical.join("Films")));
        assert_eq!(
            local.resolve_root(&films.path).await.unwrap(),
            films.path,
            "a listed path is already a root's form"
        );
    }

    #[tokio::test]
    async fn a_listing_is_resolved_as_a_root_is() {
        let scratch = Scratch::new("browse-resolved");
        let local = LocalFiles;
        let spelled = scratch.0.join("Films").join("Soul (2020)").join("..");
        let listing = local
            .list_directories(&spelled.to_string_lossy(), 100)
            .await
            .unwrap();
        assert_eq!(
            listing.path,
            displayed(&std::fs::canonicalize(scratch.0.join("Films")).unwrap())
        );
        assert_eq!(names(&listing), ["Soul (2020)"]);
        let file = scratch.0.join("Films/Soul (2020)/Soul.mkv");
        for refused in [file.to_string_lossy().into_owned(), "Films".to_owned()] {
            assert!(matches!(
                local.list_directories(&refused, 100).await,
                Err(FilesError::Refused)
            ));
        }
        assert!(matches!(
            local
                .list_directories(&scratch.0.join("gone").to_string_lossy(), 100)
                .await,
            Err(FilesError::NotFound)
        ));
    }

    #[tokio::test]
    async fn a_listing_stops_at_its_limit_and_says_so() {
        let scratch = Scratch::new("browse-limit");
        for name in ["Series", "anime", "Music"] {
            std::fs::create_dir(scratch.0.join(name)).unwrap();
        }
        let local = LocalFiles;
        let cut = local.list_directories(&scratch.root(), 2).await.unwrap();
        let first: Vec<&str> = cut.directories.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(
            first,
            ["anime", "Films"],
            "the first by name, whatever the disk's order"
        );
        assert!(cut.truncated);
        let whole = local.list_directories(&scratch.root(), 4).await.unwrap();
        assert_eq!((whole.directories.len(), whole.truncated), (4, false));
    }

    #[tokio::test]
    async fn a_step_that_does_not_answer_is_given_up_as_unavailable() {
        let stuck = within(Duration::from_millis(10), std::future::pending::<()>()).await;
        assert!(matches!(stuck, Err(FilesError::Unavailable)));
        assert!(matches!(
            within(Duration::from_millis(10), async { 1 }).await,
            Ok(1)
        ));
    }

    #[tokio::test]
    async fn browsing_starts_at_the_top_of_each_drive_or_of_the_filesystem() {
        let local = LocalFiles;
        let starts = local.starting_points().await.unwrap();
        let expected = if cfg!(windows) {
            format!("{}\\", std::env::var("SystemDrive").unwrap())
        } else {
            "/".to_owned()
        };
        let start = starts
            .iter()
            .find(|start| start.path.eq_ignore_ascii_case(&expected))
            .unwrap();
        let listing = local.list_directories(&start.path, 10).await.unwrap();
        assert_eq!(listing.parent, None);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn a_directory_marked_hidden_is_not_listed() {
        let scratch = Scratch::new("browse-hidden");
        let hidden = scratch.0.join("Hidden");
        std::fs::create_dir(&hidden).unwrap();
        let marked = std::process::Command::new("attrib")
            .arg("+h")
            .arg(&hidden)
            .status()
            .unwrap();
        assert!(marked.success());
        let listing = LocalFiles
            .list_directories(&scratch.root(), 100)
            .await
            .unwrap();
        assert_eq!(names(&listing), ["Films"]);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn a_hidden_junction_is_not_listed_and_a_denied_directory_is_unreadable() {
        let scratch = Scratch::new("browse-junction");
        let run = |program: &str, args: &[&std::ffi::OsStr]| {
            let status = std::process::Command::new(program)
                .args(args)
                .status()
                .unwrap();
            assert!(status.success(), "{program} {args:?}");
        };
        let target = scratch.0.join("Films");
        let junction = scratch.0.join("Old Films");
        run(
            "cmd",
            &[
                "/c".as_ref(),
                "mklink".as_ref(),
                "/J".as_ref(),
                junction.as_os_str(),
                target.as_os_str(),
            ],
        );
        let local = LocalFiles;
        let listing = local.list_directories(&scratch.root(), 100).await.unwrap();
        assert_eq!(
            names(&listing),
            ["Films", "Old Films"],
            "a junction is followed"
        );
        // Marked on the junction itself, as Windows marks `Documents and Settings`.
        run(
            "attrib",
            &[
                "+h".as_ref(),
                "+s".as_ref(),
                "/L".as_ref(),
                junction.as_os_str(),
            ],
        );
        let listing = local.list_directories(&scratch.root(), 100).await.unwrap();
        assert_eq!(names(&listing), ["Films"]);

        let denied = scratch.0.join("Denied");
        std::fs::create_dir(&denied).unwrap();
        let user = std::env::var("USERNAME").unwrap();
        run(
            "icacls",
            &[
                denied.as_os_str(),
                "/deny".as_ref(),
                format!("{user}:(RX)").as_ref(),
            ],
        );
        let answer = local.list_directories(&denied.to_string_lossy(), 100).await;
        run(
            "icacls",
            &[denied.as_os_str(), "/remove:d".as_ref(), user.as_ref()],
        );
        assert!(matches!(answer, Err(FilesError::Unreadable)), "{answer:?}");
    }

    /// Every way an owner may write the root of `scratch`'s films: on Windows `\`, `/` or
    /// both, a separator at the end, the drive in lower case.
    fn spellings(scratch: &Scratch) -> Vec<String> {
        let typed = scratch.0.join("Films").to_string_lossy().into_owned();
        if !cfg!(windows) {
            return vec![typed.clone(), format!("{typed}/")];
        }
        let mut drive = typed.chars();
        let lower_drive = drive
            .next()
            .map(|letter| letter.to_ascii_lowercase())
            .into_iter()
            .chain(drive)
            .collect();
        vec![
            typed.replace('/', "\\"),
            typed.replace('\\', "/"),
            typed.replace('/', "\\").replacen('\\', "/", 2),
            format!("{}\\", typed.replace('/', "\\")),
            lower_drive,
        ]
    }

    #[tokio::test]
    async fn a_root_resolves_alike_whatever_its_separators() {
        let scratch = Scratch::new("separators");
        let canonical = displayed(&std::fs::canonicalize(scratch.0.join("Films")).unwrap());
        for typed in spellings(&scratch) {
            assert_eq!(
                LocalFiles.resolve_root(&typed).await.unwrap(),
                canonical,
                "{typed}"
            );
        }
    }

    /// A film library over `typed`, resolved as `createLibrary` resolves it, scanned through
    /// this backend and the real storage.
    async fn scanned_library(typed: &str) -> (Scanner, ScanRequest) {
        use klapz_core::library::{LibraryRepository, NewLibrary, SectionKind};

        let connection = klapz_db::connect("sqlite::memory:").await.unwrap();
        let library = Arc::new(klapz_db::SqlLibraryRepository::new(connection.clone()));
        let section = library
            .create(
                NewLibrary {
                    kind: SectionKind::Movie,
                    title: "Films".to_owned(),
                    language: "fr-FR".to_owned(),
                    roots: vec![LocalFiles.resolve_root(typed).await.unwrap()],
                },
                0,
            )
            .await
            .unwrap();
        let scanner = Scanner {
            library,
            files: Arc::new(LocalFiles),
            store: Arc::new(klapz_db::SqlScanRepository::new(connection)),
        };
        let request = ScanRequest {
            section: section.id,
            below: None,
            force: false,
        };
        (scanner, request)
    }

    #[tokio::test]
    async fn a_root_as_typed_is_scanned_and_found_unchanged_the_next_time() {
        let scratch = Scratch::new("scan");
        for typed in spellings(&scratch) {
            let (scanner, request) = scanned_library(&typed).await;
            let first = scan_section(&scanner, &request).await.unwrap();
            assert_eq!(first.added, 1, "{typed}");
            assert_eq!(
                scan_section(&scanner, &request).await.unwrap(),
                ScanOutcome::default(),
                "{typed}: each file is found under its root again"
            );
        }
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn a_directory_the_server_may_not_read_does_not_stop_a_scan() {
        let scratch = Scratch::new("scan-denied");
        let denied = scratch.0.join("Films").join("Denied");
        std::fs::create_dir(&denied).unwrap();
        let user = std::env::var("USERNAME").unwrap();
        let icacls = |args: &[&std::ffi::OsStr]| {
            let status = std::process::Command::new("icacls")
                .arg(&denied)
                .args(args)
                .status()
                .unwrap();
            assert!(status.success(), "icacls {args:?}");
        };
        icacls(&["/deny".as_ref(), format!("{user}:(RX)").as_ref()]);
        let (scanner, request) = scanned_library(&scratch.root()).await;
        let outcome = scan_section(&scanner, &request).await;
        icacls(&["/remove:d".as_ref(), user.as_ref()]);
        assert_eq!(outcome.unwrap().added, 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_link_to_a_directory_is_listed_and_an_unreadable_directory_reported() {
        use std::os::unix::fs::PermissionsExt;

        let scratch = Scratch::new("browse-unix");
        std::os::unix::fs::symlink(scratch.0.join("Films"), scratch.0.join("Linked")).unwrap();
        std::os::unix::fs::symlink(
            scratch.0.join("Films/Soul (2020)/Soul.mkv"),
            scratch.0.join("Soul.mkv"),
        )
        .unwrap();
        let local = LocalFiles;
        let listing = local.list_directories(&scratch.root(), 100).await.unwrap();
        assert_eq!(names(&listing), ["Films", "Linked"]);

        let locked = scratch.0.join("Locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        // Whoever runs as root reads it anyway: there is nothing to see then.
        if std::fs::read_dir(&locked).is_err() {
            assert!(matches!(
                local.list_directories(&locked.to_string_lossy(), 100).await,
                Err(FilesError::Unreadable)
            ));
        }
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}
