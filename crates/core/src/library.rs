//! Library sections, the port through which storage supplies them, and the rules for creating,
//! changing and deleting one.

use crate::files::{FilesError, LibraryFiles};
use crate::ids::SectionId;
use crate::language::LanguageTag;
use crate::lifecycle::Lifecycle;
use crate::page::{Page, Window};
use crate::scan::{ScanControl, ScanRequest};

#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    #[error("no library section with id {0}")]
    SectionNotFound(SectionId),
    /// Storage failed.
    ///
    /// Boxed rather than stringified so the cause chain survives for the server's logs:
    /// flattening it into a `String` is the `Other(String)` smell in disguise. Boxed rather
    /// than typed because `core` must not know which storage engine is in use.
    ///
    /// Bindings must not relay this to a client — see the Plex adapter's `ApiError`.
    #[error("library storage failed")]
    Storage(#[source] Box<dyn std::error::Error + Send + Sync>),
}

/// The kind of media a section holds.
///
/// Plex stores this as an integer and exposes it as a string. Neither representation appears
/// here: the storage layer maps its column, and the binding maps its wire value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionKind {
    Movie,
    Show,
    Artist,
    Photo,
}

/// A filesystem root a section scans.
///
/// Carries its id because the API exposes it, and clients key on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionRoot {
    pub id: i64,
    pub section_id: SectionId,
    pub path: String,
    /// When the root was added, in epoch seconds. Carried because the Plex adapter orders
    /// locations by it.
    pub created_at: i64,
}

/// A library, as the domain sees it.
///
/// Times are seconds since the Unix epoch. Keeping one unambiguous representation means
/// neither Plex's `datetime` columns nor its epoch wire format leaks into the domain.
#[derive(Debug, Clone)]
pub struct LibrarySection {
    pub id: SectionId,
    pub kind: SectionKind,
    pub title: String,
    pub title_sort: Option<String>,
    /// What its metadata is matched and shown in.
    pub language: LanguageTag,
    pub uuid: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub scanned_at: Option<i64>,
    /// A monotonic change counter, not a timestamp: it drives client-side cache
    /// invalidation and is unrelated to `updated_at`.
    pub content_changed_at: i64,
    /// Whether the section is excluded from normal browsing.
    pub hidden: bool,
}

/// A library to create, with the directories it scans.
#[derive(Debug, Clone)]
pub struct NewLibrary {
    pub kind: SectionKind,
    pub title: String,
    pub language: LanguageTag,
    /// As the files backend stores them: see [`checked_roots`].
    pub roots: Vec<String>,
}

/// Changes to a library. An absent field is left as it is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LibraryEdit {
    /// A new title also clears the sort title, which was the old one's.
    pub title: Option<String>,
    pub language: Option<LanguageTag>,
    /// Replaces every root: one kept keeps its id, the others go.
    pub roots: Option<Vec<String>>,
}

/// Storage port for library data.
///
/// This trait exists to invert the dependency: `core` declares what it needs, and `db`
/// implements it, so the domain never depends on storage. It also lets `core` and the
/// bindings be tested without a database.
#[async_trait::async_trait]
pub trait LibraryRepository: Send + Sync + std::fmt::Debug {
    /// One page of sections ordered by id, without their roots.
    ///
    /// Takes a window rather than returning everything so the limit reaches SQL: this is the
    /// same port shape the metadata collections will need, where "everything" is hundreds of
    /// thousands of rows. `only` narrows it to these sections, the ones a caller may see.
    async fn sections(
        &self,
        only: Option<&[SectionId]>,
        window: Window,
    ) -> Result<Page<LibrarySection>, LibraryError>;

    async fn section(&self, id: SectionId) -> Result<LibrarySection, LibraryError>;

    /// Roots for several sections in one call.
    ///
    /// Batched rather than per-section because the API resolves `roots` through a
    /// `DataLoader`: a query over fifty libraries must cost one root query, not fifty.
    /// Sections with no root are simply absent from the result.
    async fn roots_for(&self, sections: &[SectionId]) -> Result<Vec<SectionRoot>, LibraryError>;

    /// Creates a library and its roots. `now` is its creation time, in epoch seconds.
    async fn create(&self, library: NewLibrary, now: i64) -> Result<LibrarySection, LibraryError>;

    /// Applies an edit. The library as it now is, or [`LibraryError::SectionNotFound`].
    async fn edit(
        &self,
        id: SectionId,
        edit: &LibraryEdit,
        now: i64,
    ) -> Result<LibrarySection, LibraryError>;

    /// Deletes a library with its roots, its items and everything that refers to them, and
    /// the permissions given on it. Watch state and ratings are kept, keyed by the work as
    /// they are for every copy of it. Whether there was a library.
    async fn delete(&self, id: SectionId) -> Result<bool, LibraryError>;
}

/// Lets an `Arc<dyn LibraryRepository>` be used wherever a repository is expected, so the
/// bindings can hold a trait object instead of being generic over the storage type.
#[async_trait::async_trait]
impl<T: LibraryRepository + ?Sized> LibraryRepository for std::sync::Arc<T> {
    async fn sections(
        &self,
        only: Option<&[SectionId]>,
        window: Window,
    ) -> Result<Page<LibrarySection>, LibraryError> {
        (**self).sections(only, window).await
    }

    async fn section(&self, id: SectionId) -> Result<LibrarySection, LibraryError> {
        (**self).section(id).await
    }

    async fn roots_for(&self, sections: &[SectionId]) -> Result<Vec<SectionRoot>, LibraryError> {
        (**self).roots_for(sections).await
    }

    async fn create(&self, library: NewLibrary, now: i64) -> Result<LibrarySection, LibraryError> {
        (**self).create(library, now).await
    }

    async fn edit(
        &self,
        id: SectionId,
        edit: &LibraryEdit,
        now: i64,
    ) -> Result<LibrarySection, LibraryError> {
        (**self).edit(id, edit, now).await
    }

    async fn delete(&self, id: SectionId) -> Result<bool, LibraryError> {
        (**self).delete(id).await
    }
}

/// How many directories one library scans at most: a list a client sends needs a ceiling.
pub const MAX_ROOTS: usize = 32;

/// How long a library's title may be, in characters.
const MAX_TITLE: usize = 255;

/// A title as it is stored: trimmed, not empty, no longer than 255 characters, and holding no
/// control character, which a listing or a log line would misread.
#[must_use]
pub fn normalized_title(title: &str) -> Option<String> {
    let title = title.trim();
    let valid = !title.is_empty()
        && title.chars().count() <= MAX_TITLE
        && !title.chars().any(char::is_control);
    valid.then(|| title.to_owned())
}

/// Why a list of roots was refused, with the position of the one at fault.
#[derive(Debug, thiserror::Error)]
pub enum RootError {
    #[error("a library needs a directory to scan")]
    NoRoot,
    #[error("a library scans at most {MAX_ROOTS} directories")]
    TooMany,
    /// Not an absolute path to a directory the files backend reaches.
    #[error("root {0} is not a directory this server reaches")]
    NotADirectory(usize),
    /// Lies inside another of the list, or holds one: its files would be found twice.
    #[error("root {0} lies inside another root, or holds one")]
    Overlapping(usize),
    #[error("library files could not be read")]
    Files(#[source] FilesError),
}

/// The roots a library is given, each as the files backend stores it. A repeat is kept once.
///
/// These paths come from a client, the one place a path does: an owner naming the directories
/// of the server's own disks. Each is resolved by the backend, links and all, before anything
/// compares it, so `..`, a link or another spelling cannot pass for another directory.
///
/// # Errors
///
/// [`RootError`].
pub async fn checked_roots(
    files: &dyn LibraryFiles,
    paths: &[String],
) -> Result<Vec<String>, RootError> {
    if paths.is_empty() {
        return Err(RootError::NoRoot);
    }
    if paths.len() > MAX_ROOTS {
        return Err(RootError::TooMany);
    }
    let mut roots: Vec<(usize, String)> = Vec::with_capacity(paths.len());
    for (index, path) in paths.iter().enumerate() {
        let root = match files.resolve_root(path.trim()).await {
            Ok(root) => root,
            Err(FilesError::NotFound | FilesError::Refused | FilesError::Unavailable) => {
                return Err(RootError::NotADirectory(index))
            }
            Err(error) => return Err(RootError::Files(error)),
        };
        if roots.iter().any(|(_, kept)| *kept == root) {
            continue;
        }
        let overlaps = roots.iter().any(|(_, kept)| {
            files.relative_path(kept, &root).is_some() || files.relative_path(&root, kept).is_some()
        });
        if overlaps {
            return Err(RootError::Overlapping(index));
        }
        roots.push((index, root));
    }
    Ok(roots.into_iter().map(|(_, root)| root).collect())
}

/// Starts a scan of a library just given new roots, as the reference scans a library when its
/// folders change. A scan already running, or a kind that is not scanned, is no failure.
async fn scan_after_change(scans: &dyn ScanControl, section: &LibrarySection) {
    if !matches!(section.kind, SectionKind::Movie | SectionKind::Show) {
        return;
    }
    let request = ScanRequest {
        section: section.id,
        below: None,
        force: false,
    };
    if let Err(error) = scans.start(request).await {
        tracing::info!(section = section.id.get(), %error, "no scan after a library change");
    }
}

/// Creates a library, then scans it.
///
/// # Errors
///
/// The storage's failure.
pub async fn create_library(
    library: &dyn LibraryRepository,
    scans: &dyn ScanControl,
    new: NewLibrary,
    now: i64,
) -> Result<LibrarySection, LibraryError> {
    let created = library.create(new, now).await?;
    scan_after_change(scans, &created).await;
    Ok(created)
}

/// What an edit did.
#[derive(Debug, Clone)]
pub enum Edited {
    Edited(LibrarySection),
    NotFound,
    /// Roots are not changed under a running scan, which walks the ones it started with.
    Scanning,
}

/// Edits a library, and scans it again when its roots changed. The files being read stop
/// being read: the scan that follows removes those no longer below a root, and its files are
/// read again once it is over.
///
/// # Errors
///
/// The storage's failure.
pub async fn edit_library(
    library: &dyn LibraryRepository,
    scans: &dyn ScanControl,
    id: SectionId,
    edit: &LibraryEdit,
    now: i64,
) -> Result<Edited, LibraryError> {
    if edit.roots.is_some() {
        if scans.is_running(id) {
            return Ok(Edited::Scanning);
        }
        scans.forget(id);
    }
    let edited = match library.edit(id, edit, now).await {
        Ok(edited) => edited,
        Err(LibraryError::SectionNotFound(_)) => return Ok(Edited::NotFound),
        Err(error) => return Err(error),
    };
    if edit.roots.is_some() {
        scan_after_change(scans, &edited).await;
    }
    Ok(Edited::Edited(edited))
}

/// What a deletion did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deletion {
    Deleted,
    NotFound,
    /// A running scan would write into the library as it goes.
    Scanning,
}

/// Deletes a library, ending first every playback of its items, stream and session together,
/// and the reading of its files, whose queue goes with them.
///
/// # Errors
///
/// The storage's failure.
pub async fn delete_library(
    library: &dyn LibraryRepository,
    lifecycle: &Lifecycle<'_>,
    scans: &dyn ScanControl,
    id: SectionId,
) -> Result<Deletion, LibraryError> {
    if scans.is_running(id) {
        return Ok(Deletion::Scanning);
    }
    scans.forget(id);
    let playing = lifecycle
        .sessions
        .keys_in_library(id)
        .await
        .map_err(|error| LibraryError::Storage(Box::new(error)))?;
    for key in playing {
        lifecycle
            .end(&key)
            .await
            .map_err(|error| LibraryError::Storage(Box::new(error)))?;
    }
    Ok(if library.delete(id).await? {
        Deletion::Deleted
    } else {
        Deletion::NotFound
    })
}

#[cfg(test)]
mod tests {
    // A panicking test is a failing test, which is the intended reporting mechanism.
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;
    use crate::page::Window;
    use crate::testing::InMemoryLibrary;

    fn catalog() -> InMemoryLibrary {
        InMemoryLibrary::new(vec![LibrarySection {
            id: SectionId::new(4),
            kind: SectionKind::Movie,
            title: "VF - Films".to_owned(),
            title_sort: None,
            language: LanguageTag::parse("fr-FR").expect("a tag"),
            uuid: "2cd48489-54c0-41e9-ae62-f3dcb0f3c249".to_owned(),
            created_at: 1_477_835_115,
            updated_at: 1_792_916_300,
            scanned_at: Some(1_792_916_245),
            content_changed_at: 8_457_612_157_739_110_435,
            hidden: false,
        }])
    }

    #[tokio::test]
    async fn finds_a_section_by_id() {
        let section = catalog().section(SectionId::new(4)).await.expect("seeded");
        assert_eq!(section.title, "VF - Films");
    }

    #[tokio::test]
    async fn reports_the_requested_id_when_absent() {
        let error = catalog()
            .section(SectionId::new(99))
            .await
            .expect_err("absent");
        assert!(matches!(error, LibraryError::SectionNotFound(id) if id.get() == 99));
    }

    #[tokio::test]
    async fn pages_sections_through_the_window() {
        let window = Window::new(None, None, Some(1), None).expect("valid window");
        let page = catalog().sections(None, window).await.expect("page");
        assert_eq!(page.items.len(), 1);
        assert!(!page.has_next);
        assert!(!page.has_previous);
    }

    #[test]
    fn a_title_is_trimmed_and_never_empty() {
        assert_eq!(normalized_title("  Films  ").as_deref(), Some("Films"));
        assert_eq!(normalized_title("   "), None);
        assert_eq!(
            normalized_title(
                "Films
VF"
            ),
            None
        );
        assert_eq!(
            normalized_title(&"é".repeat(255)).map(|t| t.chars().count()),
            Some(255)
        );
        assert_eq!(normalized_title(&"é".repeat(256)), None);
    }

    #[tokio::test]
    async fn roots_are_resolved_kept_once_and_never_nested() {
        let files = crate::testing::KnownDirectories::new(&[
            "/media/films",
            "/media/films/vf",
            "/media/series",
        ]);
        let paths = |paths: &[&str]| {
            paths
                .iter()
                .map(|path| (*path).to_owned())
                .collect::<Vec<_>>()
        };
        let kept = checked_roots(
            &files,
            &paths(&["/media/films/", " /media/films", "/media/series"]),
        )
        .await
        .unwrap();
        assert_eq!(kept, ["/media/films", "/media/series"]);
        assert!(matches!(
            checked_roots(&files, &paths(&["/media/films", "/media/films/vf"])).await,
            Err(RootError::Overlapping(1))
        ));
        assert!(matches!(
            checked_roots(&files, &paths(&["/media/films/vf", "/media/films"])).await,
            Err(RootError::Overlapping(1))
        ));
        assert!(matches!(
            checked_roots(&files, &paths(&["/media/series", "/etc"])).await,
            Err(RootError::NotADirectory(1))
        ));
        assert!(matches!(
            checked_roots(&files, &[]).await,
            Err(RootError::NoRoot)
        ));
        let many = vec!["/media/films".to_owned(); MAX_ROOTS + 1];
        assert!(matches!(
            checked_roots(&files, &many).await,
            Err(RootError::TooMany)
        ));
    }

    #[tokio::test]
    async fn a_library_being_scanned_is_neither_deleted_nor_given_roots() {
        let library = catalog();
        let scans = crate::testing::RecordedScans::default();
        scans.running.lock().unwrap().push(SectionId::new(4));
        let playback = crate::testing::InMemoryPlayback::default();
        let streams = crate::testing::StubStreams::default();
        let locks = crate::lifecycle::SessionLocks::default();
        let lifecycle = Lifecycle {
            sessions: &playback,
            streams: &streams,
            locks: &locks,
        };
        let id = SectionId::new(4);
        let roots = LibraryEdit {
            roots: Some(vec!["/media".to_owned()]),
            ..LibraryEdit::default()
        };
        assert!(matches!(
            edit_library(&library, &scans, id, &roots, 1).await.unwrap(),
            Edited::Scanning
        ));
        assert_eq!(
            delete_library(&library, &lifecycle, &scans, id)
                .await
                .unwrap(),
            Deletion::Scanning
        );
        let title = LibraryEdit {
            title: Some("Films".to_owned()),
            ..LibraryEdit::default()
        };
        assert!(matches!(
            edit_library(&library, &scans, id, &title, 1).await.unwrap(),
            Edited::Edited(section) if section.title == "Films"
        ));
        assert!(
            scans.forgotten.lock().unwrap().is_empty(),
            "a refused change and a new title stop no read"
        );

        // Reading its files does not hold a deletion back: it stops.
        scans.running.lock().unwrap().clear();
        assert_eq!(
            delete_library(&library, &lifecycle, &scans, id)
                .await
                .unwrap(),
            Deletion::Deleted
        );
        assert_eq!(*scans.forgotten.lock().unwrap(), [id]);
        assert_eq!(
            delete_library(&library, &lifecycle, &scans, id)
                .await
                .unwrap(),
            Deletion::NotFound
        );
    }

    #[tokio::test]
    async fn new_roots_start_a_scan_and_a_new_title_does_not() {
        let library = catalog();
        let scans = crate::testing::RecordedScans::default();
        let id = SectionId::new(4);
        let title = LibraryEdit {
            title: Some("Films".to_owned()),
            ..LibraryEdit::default()
        };
        edit_library(&library, &scans, id, &title, 1).await.unwrap();
        assert!(scans.started.lock().unwrap().is_empty());
        let roots = LibraryEdit {
            roots: Some(vec!["/media".to_owned()]),
            ..LibraryEdit::default()
        };
        edit_library(&library, &scans, id, &roots, 2).await.unwrap();
        assert_eq!(scans.started.lock().unwrap().len(), 1);
        assert_eq!(
            *scans.forgotten.lock().unwrap(),
            [id],
            "the files below its old roots are not read on"
        );
        let kept = library.roots_for(&[id]).await.unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].path, "/media");
    }
}
