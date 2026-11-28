//! One scan of a library: walk, compare, group, write, and queue what is to be read.
//!
//! The order of `docs/plex/scanner/discovery.md` §2: each directory's media files are compared with
//! the parts storage holds for it, and a directory whose files all match by name, size and
//! modification time is skipped whole. Elsewhere a known path keeps its item (rewritten when
//! the file changed), new files are grouped into items, and whatever was known below the
//! scanned directory and not seen is removed — unless its root could not be reached at all,
//! which is an unmounted disk, not an empty library, or the directory holding it could not be
//! read, which is a permission, not a deletion. Nothing here opens a media file: what is new or
//! changed is queued, and read by [`BackgroundAnalysis`] once the walk is over.
//!
//! A scan is stopped by dropping it: every write is a transaction.
//!
//! [`BackgroundAnalysis`]: crate::scan::BackgroundAnalysis

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::files::{Entry, EntryKind, FilesError, LibraryFiles, RelativePath};
use crate::ids::{PartId, SectionId};
use crate::library::{LibraryRepository, SectionKind};
use crate::now_epoch;
use crate::scan::layout::{self, FileClass, Grouped, GroupedKind, IgnoreRules};
use crate::scan::{
    FoundFile, KnownPart, NewItem, NewMedia, NewPart, NewSidecar, ScanError, ScanRepository,
};

/// A `.plexignore` larger than this is not one.
const MAX_IGNORE_FILE: u64 = 64 * 1024;

/// What a scan needs, assembled once by the server.
#[derive(Debug, Clone)]
pub struct Scanner {
    pub library: Arc<dyn LibraryRepository>,
    pub files: Arc<dyn LibraryFiles>,
    pub store: Arc<dyn ScanRepository>,
}

/// A scan to run: a whole library, or the part of it below one directory.
#[derive(Debug, Clone)]
pub struct ScanRequest {
    pub section: SectionId,
    /// A root of the section and a directory below it.
    pub below: Option<(String, RelativePath)>,
    /// Walk every directory, and read every file again, as if none had been seen.
    pub force: bool,
}

/// What a scan did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanOutcome {
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    /// Roots that could not be reached, whose items were left alone.
    pub unavailable_roots: usize,
    /// Files queued to be read: new, changed, or forced.
    pub queued: usize,
}

impl ScanOutcome {
    fn changed(&self) -> bool {
        self.added + self.updated + self.removed > 0
    }
}

/// Runs one scan to its end.
///
/// # Errors
///
/// [`ScanError::SectionNotFound`], [`ScanError::UnsupportedKind`] for music and photos,
/// [`ScanError::OutsideRoots`] when `below` names no root of the section,
/// [`ScanError::Files`], [`ScanError::Storage`].
pub async fn scan_section(
    scanner: &Scanner,
    request: &ScanRequest,
) -> Result<ScanOutcome, ScanError> {
    let section = scanner
        .library
        .section(request.section)
        .await
        .map_err(|error| match error {
            crate::LibraryError::SectionNotFound(id) => ScanError::SectionNotFound(id),
            crate::LibraryError::Storage(source) => ScanError::Storage(source),
        })?;
    if !matches!(section.kind, SectionKind::Movie | SectionKind::Show) {
        return Err(ScanError::UnsupportedKind);
    }
    let roots: Vec<String> = scanner
        .library
        .roots_for(&[request.section])
        .await
        .map_err(|error| ScanError::Storage(Box::new(error)))?
        .into_iter()
        .map(|root| root.path)
        .collect();
    let targets = match &request.below {
        Some((root, directory)) if roots.contains(root) => vec![(root.clone(), directory.clone())],
        Some((root, directory)) => {
            return Err(ScanError::OutsideRoots(
                scanner.files.stored_path(root, directory),
            ))
        }
        None => roots
            .into_iter()
            .map(|root| (root, RelativePath::root()))
            .collect(),
    };

    let mut run = Run {
        scanner,
        section: request.section,
        kind: section.kind,
        force: request.force,
        now: now_epoch(),
        outcome: ScanOutcome::default(),
    };
    for (root, start) in &targets {
        run.scan_root(root, start).await?;
    }
    if request.below.is_none() {
        run.remove_outside(&targets).await?;
    }
    let outcome = run.outcome;
    scanner
        .store
        .finish(request.section, run.now, outcome.changed())
        .await?;
    Ok(outcome)
}

struct Run<'a> {
    scanner: &'a Scanner,
    section: SectionId,
    kind: SectionKind,
    force: bool,
    now: i64,
    outcome: ScanOutcome,
}

/// A directory waiting to be walked, with the ignore rules in force above it and the
/// directory each set was found in.
struct Pending {
    directory: RelativePath,
    rules: Vec<(RelativePath, Arc<IgnoreRules>)>,
}

impl Run<'_> {
    async fn scan_root(&mut self, root: &str, start: &RelativePath) -> Result<(), ScanError> {
        let files = &self.scanner.files;
        // Reached at all? An unreachable root is left as it is: see the module's comment.
        match files.list(root, &RelativePath::root()).await {
            Ok(_) => {}
            Err(error @ (FilesError::Unavailable | FilesError::Unreadable)) => {
                tracing::warn!(root, %error, "library root not scanned; its items are kept");
                self.outcome.unavailable_roots += 1;
                return Ok(());
            }
            Err(source) => {
                return Err(ScanError::Files {
                    path: root.to_owned(),
                    source,
                })
            }
        }

        let prefix = files.stored_path(root, start);
        let known: HashMap<RelativePath, KnownPart> = self
            .scanner
            .store
            .known_parts(self.section, &prefix)
            .await?
            .into_iter()
            .filter_map(|part| {
                let path = files.relative_path(root, &part.path)?;
                path.starts_with(start).then_some((path, part))
            })
            .collect();
        let mut by_directory: HashMap<RelativePath, Vec<&RelativePath>> = HashMap::new();
        for path in known.keys() {
            by_directory.entry(path.parent()).or_default().push(path);
        }
        let mut seen: HashSet<RelativePath> = HashSet::new();
        let mut again: Vec<PartId> = Vec::new();

        let mut pending = vec![Pending {
            directory: start.clone(),
            rules: Vec::new(),
        }];
        while let Some(Pending {
            directory,
            mut rules,
        }) = pending.pop()
        {
            let entries = match files.list(root, &directory).await {
                Ok(entries) => entries,
                // Gone since its parent was listed, or a directory the walk may not enter:
                // what it held is not seen, and goes.
                Err(FilesError::NotFound | FilesError::Refused) => continue,
                // A directory the server may not read, such as `Recovery` at the top of a
                // Windows drive: the rest of the library is scanned, and what was known below
                // it stays.
                Err(FilesError::Unreadable) => {
                    tracing::warn!(
                        path = %files.stored_path(root, &directory),
                        "directory not readable; what it held is kept"
                    );
                    seen.extend(
                        known
                            .keys()
                            .filter(|path| path.starts_with(&directory))
                            .cloned(),
                    );
                    continue;
                }
                Err(source) => {
                    return Err(ScanError::Files {
                        path: files.stored_path(root, &directory),
                        source,
                    })
                }
            };
            if entries.iter().any(|entry| entry.name == ".plexignore") {
                if let Some(found) = self.ignore_rules(root, &directory).await {
                    rules.push((directory.clone(), Arc::new(found)));
                }
            }
            let (media, subtitles) = sort_entries(&directory, &entries, &rules, &mut pending);

            let known_here = by_directory.get(&directory).map_or(&[][..], Vec::as_slice);
            if !self.force && unchanged(&media, known_here, &known) {
                seen.extend(media.into_iter().map(|file| file.path));
                continue;
            }
            self.scan_directory(root, &directory, &media, &subtitles, &known, &mut again)
                .await?;
            seen.extend(media.into_iter().map(|file| file.path));
        }

        if !again.is_empty() {
            self.scanner.store.queue_again(&again, self.now).await?;
            self.outcome.queued += again.len();
        }
        let gone: Vec<PartId> = known
            .iter()
            .filter(|(path, _)| !seen.contains(*path))
            .map(|(_, part)| part.part)
            .collect();
        if !gone.is_empty() {
            self.scanner.store.remove_parts(&gone).await?;
            self.outcome.removed += gone.len();
        }
        Ok(())
    }

    /// Removes the files the library holds below none of its roots: those of a root it no
    /// longer has. A library left without roots keeps its files, as an unreachable root does.
    async fn remove_outside(&mut self, roots: &[(String, RelativePath)]) -> Result<(), ScanError> {
        if roots.is_empty() {
            return Ok(());
        }
        let files = &self.scanner.files;
        let outside: Vec<PartId> = self
            .scanner
            .store
            .known_parts(self.section, "")
            .await?
            .into_iter()
            .filter(|part| {
                roots
                    .iter()
                    .all(|(root, _)| files.relative_path(root, &part.path).is_none())
            })
            .map(|part| part.part)
            .collect();
        if !outside.is_empty() {
            self.scanner.store.remove_parts(&outside).await?;
            self.outcome.removed += outside.len();
        }
        Ok(())
    }

    async fn ignore_rules(&self, root: &str, directory: &RelativePath) -> Option<IgnoreRules> {
        let file = directory.join(".plexignore").ok()?;
        match self.scanner.files.read(root, &file, MAX_IGNORE_FILE).await {
            Ok(bytes) => Some(IgnoreRules::parse(&String::from_utf8_lossy(&bytes))),
            Err(error) => {
                tracing::warn!(%error, path = %file, "cannot read .plexignore");
                None
            }
        }
    }

    /// Writes what changed in one directory; files known and unchanged that a forced scan
    /// reads again go to `again`, queued together once the root is walked.
    async fn scan_directory(
        &mut self,
        root: &str,
        directory: &RelativePath,
        media: &[FoundFile],
        subtitles: &[RelativePath],
        known: &HashMap<RelativePath, KnownPart>,
        again: &mut Vec<PartId>,
    ) -> Result<(), ScanError> {
        // Known paths keep their item, rewritten only when the file changed: a path match is
        // reused, never regrouped (discovery.md §2.4).
        let mut new = Vec::new();
        for (index, file) in media.iter().enumerate() {
            match known.get(&file.path) {
                Some(part) if part.size != file.size || part.modified_at != file.modified_at => {
                    let rewritten = self.new_part(root, file, subtitles);
                    self.scanner
                        .store
                        .update_part(part.part, rewritten, self.now)
                        .await?;
                    self.outcome.updated += 1;
                    self.outcome.queued += 1;
                }
                Some(part) if self.force => again.push(part.part),
                Some(_) => {}
                None => new.push(index),
            }
        }
        if new.is_empty() {
            return Ok(());
        }
        let names: Vec<&str> = new
            .iter()
            .filter_map(|index| media[*index].path.name())
            .collect();
        let groups = match self.kind {
            SectionKind::Movie => layout::group_movies(directory, &names),
            _ => layout::group_episodes(directory, &names),
        };
        for group in groups {
            let files: Vec<Vec<&FoundFile>> = group
                .versions
                .iter()
                .map(|version| {
                    version
                        .iter()
                        .filter_map(|index| new.get(*index).map(|file| &media[*file]))
                        .collect()
                })
                .filter(|version: &Vec<&FoundFile>| !version.is_empty())
                .collect();
            if files.is_empty() {
                continue;
            }
            let versions: Vec<NewMedia> = files
                .into_iter()
                .map(|version| NewMedia {
                    parts: version
                        .into_iter()
                        .map(|file| self.new_part(root, file, subtitles))
                        .collect(),
                })
                .collect();
            self.outcome.queued += versions
                .iter()
                .map(|version| version.parts.len())
                .sum::<usize>();
            let item = new_item(group, versions);
            self.scanner
                .store
                .add_item(self.section, item, self.now)
                .await?;
            self.outcome.added += 1;
        }
        Ok(())
    }

    fn new_part(&self, root: &str, file: &FoundFile, subtitles: &[RelativePath]) -> NewPart {
        let files = &self.scanner.files;
        let names: Vec<&str> = subtitles
            .iter()
            .filter(|subtitle| subtitle.parent() == file.path.parent())
            .filter_map(RelativePath::name)
            .collect();
        let sidecars = file
            .path
            .name()
            .map(|name| layout::sidecars(name, &names))
            .unwrap_or_default()
            .into_iter()
            .filter_map(|found| {
                let name = names.get(found.index)?;
                let sidecar = file.path.parent().join(name).ok()?;
                Some(NewSidecar {
                    path: files.stored_path(root, &sidecar),
                    codec: extension(name)?,
                    language: found.language,
                    forced: found.forced,
                    hearing_impaired: found.hearing_impaired,
                })
            })
            .collect();
        NewPart {
            path: files.stored_path(root, &file.path),
            size: file.size,
            modified_at: file.modified_at,
            sidecars,
        }
    }
}

/// A directory's entries: its media and subtitle files in name order, and its
/// subdirectories queued for the walk, all but what the ignore rules or the reference's list of
/// skipped directories leave out.
fn sort_entries(
    directory: &RelativePath,
    entries: &[Entry],
    rules: &[(RelativePath, Arc<IgnoreRules>)],
    pending: &mut Vec<Pending>,
) -> (Vec<FoundFile>, Vec<RelativePath>) {
    let ignored = |path: &RelativePath| {
        rules
            .iter()
            .any(|(base, rules)| relative_to(path, base).is_some_and(|inner| rules.ignores(&inner)))
    };
    let mut media = Vec::new();
    let mut subtitles = Vec::new();
    for entry in entries {
        let Ok(path) = directory.join(&entry.name) else {
            continue;
        };
        if entry.name.starts_with('.') || ignored(&path) {
            continue;
        }
        match entry.kind {
            EntryKind::Directory if layout::skips_directory(&entry.name, directory.is_root()) => {}
            EntryKind::Directory => pending.push(Pending {
                directory: path,
                rules: rules.to_vec(),
            }),
            EntryKind::File => match layout::classify(&entry.name) {
                FileClass::Media => media.push(FoundFile {
                    path,
                    size: entry.size,
                    modified_at: entry.modified_at,
                }),
                FileClass::Subtitle => subtitles.push(path),
                FileClass::Ignored => {}
            },
        }
    }
    media.sort_by(|a, b| a.path.cmp(&b.path));
    subtitles.sort();
    (media, subtitles)
}

/// The reference's skip rule (discovery.md §2.3): as many media files as known parts, each
/// known by name with the same size and modification time.
fn unchanged(
    media: &[FoundFile],
    known_here: &[&RelativePath],
    known: &HashMap<RelativePath, KnownPart>,
) -> bool {
    media.len() == known_here.len()
        && media.iter().all(|file| {
            known
                .get(&file.path)
                .is_some_and(|part| part.size == file.size && part.modified_at == file.modified_at)
        })
}

fn new_item(group: Grouped, versions: Vec<NewMedia>) -> NewItem {
    match group.kind {
        GroupedKind::Movie { title, year } => NewItem::Movie {
            title,
            year,
            versions,
        },
        // A file holding several episodes is filed under its first for now; the reference
        // gives each episode the file, with a display offset.
        GroupedKind::Episode {
            show,
            season,
            episode,
            released_on,
            ..
        } => NewItem::Episode {
            show,
            season,
            episode,
            released_on,
            versions,
        },
    }
}

/// `path` below `base`, as a path relative to it.
fn relative_to(path: &RelativePath, base: &RelativePath) -> Option<RelativePath> {
    if !path.starts_with(base) {
        return None;
    }
    let inner = path.as_str().get(base.as_str().len()..)?;
    RelativePath::parse(inner.trim_start_matches('/')).ok()
}

fn extension(name: &str) -> Option<String> {
    let (_, extension) = name.rsplit_once('.')?;
    Some(extension.to_ascii_lowercase())
}

/// The doubles a scan runs on in tests, here and in [`super::control`]'s.
#[cfg(test)]
pub(super) mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use std::sync::Mutex;

    use klapz_scanner::report::{Analysis, DeepAnalysis};

    use crate::files::{Entry, FileLocation};
    use crate::ids::{MediaId, RatingKey};
    use crate::library::{LibrarySection, SectionRoot};
    use crate::scan::{AnalysisProgress, MediaTask, QueueFilter, QueuedFile, Stored};
    use crate::testing::InMemoryLibrary;

    /// A directory tree in memory: paths below the root, with size and time.
    #[derive(Debug, Default)]
    pub(crate) struct Tree {
        pub(crate) files: Mutex<Vec<(String, u64, i64)>>,
        pub(crate) unavailable: bool,
        /// Directories the server may not list.
        pub(crate) unreadable: Vec<String>,
        /// Directories whose listing fails, as a disk failing mid-scan does.
        pub(crate) broken: Vec<String>,
        /// Directories whose listing never answers, as a share that stopped answering.
        pub(crate) stalled: Vec<String>,
        pub(crate) stall: Stall,
    }

    /// What became of the listings of a stalled directory.
    #[derive(Debug, Default)]
    pub(crate) struct Stall {
        pub(crate) started: tokio::sync::Notify,
        pub(crate) dropped: std::sync::atomic::AtomicBool,
    }

    struct SetOnDrop<'a>(&'a std::sync::atomic::AtomicBool);

    impl Drop for SetOnDrop<'_> {
        fn drop(&mut self) {
            self.0.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    impl Tree {
        pub(crate) fn with(files: &[(&str, u64, i64)]) -> Self {
            Self {
                files: Mutex::new(
                    files
                        .iter()
                        .map(|(path, size, time)| ((*path).to_owned(), *size, *time))
                        .collect(),
                ),
                ..Self::default()
            }
        }

        fn set(&self, files: &[(&str, u64, i64)]) {
            *self.files.lock().unwrap() = files
                .iter()
                .map(|(path, size, time)| ((*path).to_owned(), *size, *time))
                .collect();
        }
    }

    #[async_trait::async_trait]
    impl LibraryFiles for Tree {
        async fn list(
            &self,
            _root: &str,
            directory: &RelativePath,
        ) -> Result<Vec<Entry>, FilesError> {
            if self.unavailable {
                return Err(FilesError::Unavailable);
            }
            if self
                .unreadable
                .iter()
                .any(|path| path == directory.as_str())
            {
                return Err(FilesError::Unreadable);
            }
            if self.broken.iter().any(|path| path == directory.as_str()) {
                return Err(FilesError::Io("the device is not ready".into()));
            }
            if self.stalled.iter().any(|path| path == directory.as_str()) {
                let _dropped = SetOnDrop(&self.stall.dropped);
                self.stall.started.notify_one();
                std::future::pending::<()>().await;
            }
            let mut entries: Vec<Entry> = Vec::new();
            for (path, size, time) in self.files.lock().unwrap().iter() {
                let path = RelativePath::parse(path).unwrap();
                let Some(inner) = relative_to(&path, directory) else {
                    continue;
                };
                let mut names = inner.components();
                let (Some(first), rest) = (names.next(), names.next()) else {
                    continue;
                };
                let kind = if rest.is_some() {
                    EntryKind::Directory
                } else {
                    EntryKind::File
                };
                if !entries.iter().any(|entry| entry.name == first) {
                    entries.push(Entry {
                        name: first.to_owned(),
                        kind,
                        size: *size,
                        modified_at: *time,
                    });
                }
            }
            Ok(entries)
        }

        async fn read(
            &self,
            _root: &str,
            _file: &RelativePath,
            _limit: u64,
        ) -> Result<Vec<u8>, FilesError> {
            Err(FilesError::NotFound)
        }

        fn stored_path(&self, root: &str, file: &RelativePath) -> String {
            format!("{root}/{file}")
        }

        fn relative_path(&self, root: &str, stored: &str) -> Option<RelativePath> {
            RelativePath::parse(stored.strip_prefix(root)?.strip_prefix('/')?).ok()
        }

        async fn locate(
            &self,
            root: &str,
            file: &RelativePath,
        ) -> Result<FileLocation, FilesError> {
            Ok(FileLocation::Local(format!("{root}/{file}").into()))
        }

        async fn resolve_root(&self, path: &str) -> Result<String, FilesError> {
            Ok(path.to_owned())
        }

        async fn starting_points(&self) -> Result<Vec<crate::files::Directory>, FilesError> {
            Ok(Vec::new())
        }

        async fn list_directories(
            &self,
            _path: &str,
            _limit: usize,
        ) -> Result<crate::files::DirectoryListing, FilesError> {
            Err(FilesError::NotFound)
        }
    }

    /// Storage that keeps parts in a list, items as what was added, and the queue as the
    /// parts in it.
    #[derive(Debug, Default)]
    pub(crate) struct Store {
        parts: Mutex<Vec<KnownPart>>,
        added: Mutex<Vec<NewItem>>,
        updated: Mutex<Vec<PartId>>,
        queued: Mutex<Vec<PartId>>,
        finished: Mutex<Vec<bool>>,
    }

    #[async_trait::async_trait]
    impl ScanRepository for Store {
        async fn known_parts(
            &self,
            _section: SectionId,
            prefix: &str,
        ) -> Result<Vec<KnownPart>, ScanError> {
            Ok(self
                .parts
                .lock()
                .unwrap()
                .iter()
                .filter(|part| part.path.starts_with(prefix))
                .cloned()
                .collect())
        }

        async fn add_item(
            &self,
            _section: SectionId,
            item: NewItem,
            _now: i64,
        ) -> Result<RatingKey, ScanError> {
            let mut parts = self.parts.lock().unwrap();
            let key = i64::try_from(parts.len()).unwrap() + 1;
            let versions = match &item {
                NewItem::Movie { versions, .. } | NewItem::Episode { versions, .. } => versions,
            };
            for part in versions.iter().flat_map(|version| &version.parts) {
                let id = i64::try_from(parts.len()).unwrap() + 1;
                self.queued.lock().unwrap().push(PartId::new(id));
                parts.push(KnownPart {
                    part: PartId::new(id),
                    media: MediaId::new(id),
                    item: RatingKey::new(key),
                    path: part.path.clone(),
                    size: part.size,
                    modified_at: part.modified_at,
                });
            }
            self.added.lock().unwrap().push(item);
            Ok(RatingKey::new(key))
        }

        async fn update_part(
            &self,
            part: PartId,
            file: NewPart,
            _now: i64,
        ) -> Result<(), ScanError> {
            for known in self.parts.lock().unwrap().iter_mut() {
                if known.part == part {
                    known.size = file.size;
                    known.modified_at = file.modified_at;
                }
            }
            self.updated.lock().unwrap().push(part);
            self.queued.lock().unwrap().push(part);
            Ok(())
        }

        async fn queue_again(&self, parts: &[PartId], _now: i64) -> Result<(), ScanError> {
            self.queued.lock().unwrap().extend_from_slice(parts);
            Ok(())
        }

        async fn queued_files(
            &self,
            _limit: usize,
            _filter: &QueueFilter,
        ) -> Result<Vec<QueuedFile>, ScanError> {
            Ok(Vec::new())
        }

        async fn queued_file(&self, _part: PartId) -> Result<Option<QueuedFile>, ScanError> {
            Ok(None)
        }

        async fn store_headers(
            &self,
            _file: &QueuedFile,
            _analysis: Option<&Analysis>,
            _now: i64,
        ) -> Result<Stored, ScanError> {
            Ok(Stored::Stale)
        }

        async fn store_measurement(
            &self,
            _file: &QueuedFile,
            _measured: Option<&DeepAnalysis>,
            _now: i64,
        ) -> Result<Stored, ScanError> {
            Ok(Stored::Stale)
        }

        async fn finish_task(&self, _file: &QueuedFile) -> Result<Stored, ScanError> {
            Ok(Stored::Stale)
        }

        async fn analysis_progress(
            &self,
            _section: SectionId,
            _counted: &[MediaTask],
        ) -> Result<AnalysisProgress, ScanError> {
            Ok(AnalysisProgress::default())
        }

        async fn contents_changed(&self, _section: SectionId) -> Result<(), ScanError> {
            Ok(())
        }

        async fn remove_parts(&self, parts: &[PartId]) -> Result<(), ScanError> {
            self.parts
                .lock()
                .unwrap()
                .retain(|part| !parts.contains(&part.part));
            Ok(())
        }

        async fn finish(
            &self,
            _section: SectionId,
            _at: i64,
            changed: bool,
        ) -> Result<(), ScanError> {
            self.finished.lock().unwrap().push(changed);
            Ok(())
        }
    }

    pub(crate) fn scanner(kind: SectionKind, tree: Arc<Tree>, store: Arc<Store>) -> Scanner {
        let library = InMemoryLibrary::new(vec![LibrarySection {
            id: SectionId::new(1),
            kind,
            title: "Films".to_owned(),
            title_sort: None,
            language: crate::language::LanguageTag::parse("fr-FR").unwrap(),
            uuid: "uuid".to_owned(),
            created_at: 0,
            updated_at: 0,
            scanned_at: None,
            content_changed_at: 0,
            hidden: false,
        }])
        .with_roots(vec![SectionRoot {
            id: 1,
            section_id: SectionId::new(1),
            path: "/media".to_owned(),
            created_at: 0,
        }]);
        Scanner {
            library: Arc::new(library),
            files: tree,
            store,
        }
    }

    pub(crate) fn whole_library() -> ScanRequest {
        ScanRequest {
            section: SectionId::new(1),
            below: None,
            force: false,
        }
    }

    async fn scan(scanner: &Scanner, request: &ScanRequest) -> ScanOutcome {
        scan_section(scanner, request).await.unwrap()
    }

    #[tokio::test]
    async fn adds_new_films_then_skips_what_did_not_change() {
        let tree = Arc::new(Tree::with(&[
            ("Soul (2020)/Soul (2020).mkv", 100, 10),
            ("Up (2009)/Up (2009).mkv", 200, 20),
        ]));
        let store = Arc::new(Store::default());
        let scanner = scanner(SectionKind::Movie, tree, Arc::clone(&store));

        let first = scan(&scanner, &whole_library()).await;
        assert_eq!(first.added, 2);
        assert_eq!(first.queued, 2, "each new file is queued to be read");
        let titles: Vec<String> = store
            .added
            .lock()
            .unwrap()
            .iter()
            .map(|item| match item {
                NewItem::Movie { title, year, .. } => format!("{title} {year:?}"),
                NewItem::Episode { .. } => String::new(),
            })
            .collect();
        assert!(titles.contains(&"Soul Some(2020)".to_owned()), "{titles:?}");

        let second = scan(&scanner, &whole_library()).await;
        assert_eq!(second, ScanOutcome::default());
        assert_eq!(
            store.queued.lock().unwrap().len(),
            2,
            "what did not change is not queued again"
        );
        assert_eq!(*store.finished.lock().unwrap(), vec![true, false]);
    }

    #[tokio::test]
    async fn a_forced_scan_queues_every_file_again_and_rewrites_none() {
        let tree = Arc::new(Tree::with(&[
            ("Soul (2020)/Soul (2020).mkv", 100, 10),
            ("Up (2009)/Up (2009).mkv", 200, 20),
        ]));
        let store = Arc::new(Store::default());
        let scanner = scanner(SectionKind::Movie, tree, Arc::clone(&store));
        scan(&scanner, &whole_library()).await;

        let forced = scan(
            &scanner,
            &ScanRequest {
                force: true,
                ..whole_library()
            },
        )
        .await;
        assert_eq!((forced.added, forced.updated, forced.queued), (0, 0, 2));
        assert_eq!(store.queued.lock().unwrap().len(), 4);
        assert!(store.updated.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn rewrites_a_changed_file_and_removes_a_missing_one() {
        let tree = Arc::new(Tree::with(&[
            ("Soul (2020)/Soul (2020).mkv", 100, 10),
            ("Up (2009)/Up (2009).mkv", 200, 20),
        ]));
        let store = Arc::new(Store::default());
        let scanner = scanner(SectionKind::Movie, Arc::clone(&tree), Arc::clone(&store));
        scan(&scanner, &whole_library()).await;

        tree.set(&[("Soul (2020)/Soul (2020).mkv", 150, 30)]);
        let outcome = scan(&scanner, &whole_library()).await;
        assert_eq!(outcome.updated, 1);
        assert_eq!(outcome.removed, 1);
        assert_eq!(outcome.added, 0, "a known path keeps its item");
        assert_eq!(outcome.queued, 1, "a changed file is read again");
        let parts = store.parts.lock().unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!((parts[0].size, parts[0].modified_at), (150, 30));
    }

    #[tokio::test]
    async fn an_unreachable_root_keeps_its_items() {
        let store = Arc::new(Store::default());
        let reachable = scanner(
            SectionKind::Movie,
            Arc::new(Tree::with(&[("Soul (2020)/Soul (2020).mkv", 100, 10)])),
            Arc::clone(&store),
        );
        scan(&reachable, &whole_library()).await;

        let unmounted = scanner(
            SectionKind::Movie,
            Arc::new(Tree {
                unavailable: true,
                ..Tree::default()
            }),
            Arc::clone(&store),
        );
        let outcome = scan(&unmounted, &whole_library()).await;
        assert_eq!(outcome.unavailable_roots, 1);
        assert_eq!(outcome.removed, 0);
        assert_eq!(store.parts.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_partial_scan_removes_only_below_its_directory() {
        let tree = Arc::new(Tree::with(&[
            ("Soul (2020)/Soul (2020).mkv", 100, 10),
            ("Up (2009)/Up (2009).mkv", 200, 20),
        ]));
        let store = Arc::new(Store::default());
        let scanner = scanner(SectionKind::Movie, Arc::clone(&tree), Arc::clone(&store));
        scan(&scanner, &whole_library()).await;

        tree.set(&[]);
        let below = ScanRequest {
            below: Some((
                "/media".to_owned(),
                RelativePath::parse("Up (2009)").unwrap(),
            )),
            ..whole_library()
        };
        let outcome = scan(&scanner, &below).await;
        assert_eq!(outcome.removed, 1);
        let parts = store.parts.lock().unwrap();
        assert_eq!(parts.len(), 1);
        assert!(parts[0].path.contains("Soul"));
    }

    #[tokio::test]
    async fn refuses_a_directory_outside_the_roots() {
        let scanner = scanner(
            SectionKind::Movie,
            Arc::new(Tree::default()),
            Arc::new(Store::default()),
        );
        let request = ScanRequest {
            below: Some(("/elsewhere".to_owned(), RelativePath::root())),
            ..whole_library()
        };
        let refused = scan_section(&scanner, &request).await;
        assert!(matches!(refused, Err(ScanError::OutsideRoots(_))));
    }

    #[tokio::test]
    async fn an_unreadable_directory_is_passed_over_and_keeps_its_items() {
        let files = [
            ("Soul (2020)/Soul (2020).mkv", 100, 10),
            ("Up (2009)/Up (2009).mkv", 200, 20),
        ];
        let store = Arc::new(Store::default());
        let readable = scanner(
            SectionKind::Movie,
            Arc::new(Tree::with(&files[1..])),
            Arc::clone(&store),
        );
        scan(&readable, &whole_library()).await;

        let locked = scanner(
            SectionKind::Movie,
            Arc::new(Tree {
                unreadable: vec!["Up (2009)".to_owned()],
                ..Tree::with(&files)
            }),
            Arc::clone(&store),
        );
        let outcome = scan(&locked, &whole_library()).await;
        assert_eq!((outcome.added, outcome.removed), (1, 0));
        assert_eq!(store.parts.lock().unwrap().len(), 2);
        assert_eq!(*store.finished.lock().unwrap(), vec![true, true]);
    }

    #[tokio::test]
    async fn an_unreadable_root_is_left_as_an_unreachable_one() {
        let scanner = scanner(
            SectionKind::Movie,
            Arc::new(Tree {
                unreadable: vec![String::new()],
                ..Tree::with(&[("Soul (2020)/Soul (2020).mkv", 100, 10)])
            }),
            Arc::new(Store::default()),
        );
        let outcome = scan(&scanner, &whole_library()).await;
        assert_eq!((outcome.added, outcome.unavailable_roots), (0, 1));
    }

    #[tokio::test]
    async fn a_directory_failing_mid_scan_stops_it_and_is_named() {
        let store = Arc::new(Store::default());
        let scanner = scanner(
            SectionKind::Movie,
            Arc::new(Tree {
                broken: vec!["Up (2009)".to_owned()],
                ..Tree::with(&[
                    ("Soul (2020)/Soul (2020).mkv", 100, 10),
                    ("Up (2009)/Up (2009).mkv", 200, 20),
                ])
            }),
            Arc::clone(&store),
        );
        let failed = scan_section(&scanner, &whole_library()).await;
        assert!(
            matches!(&failed, Err(error @ ScanError::Files { .. })
                if error.to_string() == "cannot list /media/Up (2009): reading library files failed"),
            "{failed:?}"
        );
        assert!(store.finished.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_whole_scan_removes_the_files_of_a_root_the_library_lost() {
        let store = Arc::new(Store::default());
        store.parts.lock().unwrap().push(KnownPart {
            part: PartId::new(90),
            media: MediaId::new(90),
            item: RatingKey::new(90),
            path: "/old/Up (2009)/Up (2009).mkv".to_owned(),
            size: 200,
            modified_at: 20,
        });
        let scanner = scanner(
            SectionKind::Movie,
            Arc::new(Tree::with(&[("Soul (2020)/Soul (2020).mkv", 100, 10)])),
            Arc::clone(&store),
        );

        let below = ScanRequest {
            below: Some(("/media".to_owned(), RelativePath::root())),
            ..whole_library()
        };
        scan(&scanner, &below).await;
        assert_eq!(
            store.parts.lock().unwrap().len(),
            2,
            "a partial scan keeps it"
        );

        let outcome = scan(&scanner, &whole_library()).await;
        assert_eq!(outcome.removed, 1);
        let parts = store.parts.lock().unwrap();
        assert_eq!(parts.len(), 1);
        assert!(parts[0].path.starts_with("/media/"));
    }

    #[tokio::test]
    async fn lists_new_files_by_their_names_alone() {
        let store = Arc::new(Store::default());
        let scanner = scanner(
            SectionKind::Show,
            Arc::new(Tree::with(&[
                ("Bull/Season 05/Bull - S05E01.mkv", 100, 10),
                ("Bull/Season 05/Bull - S05E01.fr.srt", 1, 10),
                ("Bull/Season 05/Bull - S05E02.mkv", 100, 10),
            ])),
            Arc::clone(&store),
        );
        let outcome = scan(&scanner, &whole_library()).await;
        assert_eq!((outcome.added, outcome.queued), (2, 2));
        let added = store.added.lock().unwrap();
        let first = added.iter().find_map(|item| match item {
            NewItem::Episode {
                episode: 1,
                versions,
                ..
            } => Some(&versions[0].parts[0]),
            _ => None,
        });
        let file = first.unwrap();
        assert_eq!((file.size, file.modified_at), (100, 10));
        assert_eq!(
            file.sidecars.len(),
            1,
            "sidecars are known from their names"
        );
    }
}
