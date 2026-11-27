//! Reading the files scans queued, in the background, a few at a time: every file's headers
//! before any file's packets. Headers take a fraction of a second and make a file playable and
//! fully described; packets take as long as the whole file takes to read, and only refine its
//! rates and duration (`docs/scanner/analysis.md`, §1 and §2).
//!
//! The queue is storage's, so it outlives the process: what a stopped server did not read is
//! read when it starts again.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::{Notify, Semaphore};
use tokio::task::{AbortHandle, JoinHandle};

use crate::files::{FileLocation, LibraryFiles};
use crate::ids::{PartId, SectionId};
use crate::library::LibraryRepository;
use crate::metadata::FileAnalysis;
use crate::now_epoch;
use crate::scan::{
    AnalysisProgress, AnalyzeError, MediaAnalyzer, QueueFilter, QueuedFile, ScanError,
    ScanRepository, Stored,
};

/// Reads a player asks for at once, besides the queue's: a viewer does not wait behind a
/// measurement that reads a whole film, and a client cannot start more than this many.
const ON_DEMAND: usize = 4;

/// How long the queue waits after storage failed to hand it files.
const STORAGE_RETRY: Duration = Duration::from_secs(30);

/// Reads what scans queue; one per server, cheap to clone.
#[derive(Debug, Clone)]
pub struct BackgroundAnalysis {
    shared: Arc<Shared>,
}

#[derive(Debug)]
struct Shared {
    store: Arc<dyn ScanRepository>,
    library: Arc<dyn LibraryRepository>,
    files: Arc<dyn LibraryFiles>,
    analyzer: Arc<dyn MediaAnalyzer>,
    concurrency: usize,
    state: Mutex<State>,
    wake: Notify,
    on_demand: Semaphore,
}

#[derive(Debug, Default)]
struct State {
    busy: HashMap<PartId, Busy>,
    /// Libraries a scan is walking: it may remove or change what is queued, and is short.
    walking: HashSet<SectionId>,
    /// Libraries whose files wait for their next scan, and why, when it is not an owner's
    /// cancellation.
    paused: HashMap<SectionId, Option<String>>,
    /// Why no file can be read at all: the tools are missing.
    unavailable: Option<String>,
}

#[derive(Debug)]
struct Busy {
    section: SectionId,
    task: AbortHandle,
}

/// What reading one file came to.
enum Outcome {
    /// Stored as far as it was read, or found read already or changed: this read is over.
    Done,
    /// Nothing was written; the file stays queued as it was.
    Left,
}

impl BackgroundAnalysis {
    /// `concurrency` files are read at once, at least one.
    pub fn new(
        store: Arc<dyn ScanRepository>,
        library: Arc<dyn LibraryRepository>,
        files: Arc<dyn LibraryFiles>,
        analyzer: Arc<dyn MediaAnalyzer>,
        concurrency: usize,
    ) -> Self {
        Self {
            shared: Arc::new(Shared {
                store,
                library,
                files,
                analyzer,
                concurrency: concurrency.max(1),
                state: Mutex::default(),
                wake: Notify::new(),
                on_demand: Semaphore::new(ON_DEMAND),
            }),
        }
    }

    /// Reads the queue as long as the server runs, starting with what a stopped one left.
    pub fn spawn(&self) -> JoinHandle<()> {
        let analysis = self.clone();
        tokio::spawn(async move {
            // Without a library in scope, the loop only ends with the runtime.
            let _ = analysis.run(None).await;
        })
    }

    /// Reads one library's queue to its end, outside a server.
    ///
    /// # Errors
    ///
    /// [`ScanError::AnalyzerUnavailable`] when the tools cannot run, what was left queued.
    pub async fn analyze_library(&self, section: SectionId) -> Result<(), ScanError> {
        self.run(Some(section)).await
    }

    /// The loop that takes files off the queue. One runs per instance: wake-ups go to one
    /// waiter.
    async fn run(&self, scope: Option<SectionId>) -> Result<(), ScanError> {
        loop {
            let woken = self.shared.wake.notified();
            let (free, filter) = {
                let state = self.shared.lock();
                if let (Some(reason), Some(_)) = (&state.unavailable, scope) {
                    return Err(ScanError::AnalyzerUnavailable(reason.clone()));
                }
                let free = if state.unavailable.is_some() {
                    0
                } else {
                    self.shared.concurrency.saturating_sub(state.busy.len())
                };
                let filter = QueueFilter {
                    section: scope,
                    busy: state.busy.keys().copied().collect(),
                    held: state
                        .walking
                        .iter()
                        .chain(state.paused.keys())
                        .copied()
                        .collect(),
                };
                (free, filter)
            };
            let mut taken = 0;
            if free > 0 {
                match self.shared.store.queued_files(free, &filter).await {
                    Ok(files) => {
                        taken = files.len();
                        for file in files {
                            self.start(file);
                        }
                    }
                    Err(error) => {
                        tracing::error!(%error, "cannot read the analysis queue");
                        tokio::time::sleep(STORAGE_RETRY).await;
                        continue;
                    }
                }
            }
            if let Some(section) = scope {
                let reading = self.shared.lock().reads(section);
                if taken == 0 && !reading {
                    return Ok(());
                }
            }
            woken.await;
        }
    }

    fn start(&self, file: QueuedFile) {
        let shared = Arc::clone(&self.shared);
        let mut state = self.shared.lock();
        let (part, section) = (file.part, file.section);
        // Spawned under the lock, so that the task, which removes its entry as it ends, cannot
        // end before the entry is there.
        let task = tokio::spawn(async move {
            let _done = Done {
                shared: Arc::clone(&shared),
                part,
            };
            if let Err(error) = shared.read(&file).await {
                tracing::error!(path = %file.path, %error, "cannot store the file's analysis");
            }
        });
        state.busy.insert(
            part,
            Busy {
                section,
                task: task.abort_handle(),
            },
        );
    }

    /// Reads a file's headers now when they are what it waits for: a player is about to play
    /// it. Whether nothing is left to wait for — read now, before, or found not to be media.
    ///
    /// # Errors
    ///
    /// Storage's failure.
    pub async fn read_now(&self, part: PartId) -> Result<bool, ScanError> {
        let Some(file) = self.shared.store.queued_file(part).await? else {
            return Ok(true);
        };
        if file.analysis != FileAnalysis::Pending {
            return Ok(true);
        }
        let Ok(_permit) = self.shared.on_demand.acquire().await else {
            return Ok(false);
        };
        // The queue may be reading it too: whichever stores first is kept, the other is stale.
        Ok(match self.shared.read(&file).await? {
            Outcome::Done => true,
            Outcome::Left => false,
        })
    }

    /// Holds the library's files while a scan walks it.
    pub(crate) fn walking(&self, section: SectionId) {
        let mut state = self.shared.lock();
        state.walking.insert(section);
        // Its next scan is what a pause waits for, and a missing tool may have been installed
        // since.
        state.paused.remove(&section);
        state.unavailable = None;
    }

    pub(crate) fn walked(&self, section: SectionId) {
        self.shared.lock().walking.remove(&section);
        self.shared.wake.notify_one();
    }

    /// Stops reading the library's files until its next scan; whether any was being read.
    pub(crate) fn pause(&self, section: SectionId) -> bool {
        let mut state = self.shared.lock();
        state.paused.entry(section).or_insert(None);
        state.abort(section)
    }

    /// Stops reading the files being read: the library is being deleted, or given other
    /// roots, whose scan removes what is no longer below them.
    pub(crate) fn forget(&self, section: SectionId) {
        self.shared.lock().abort(section);
    }

    pub(crate) fn reads(&self, section: SectionId) -> bool {
        self.shared.lock().reads(section)
    }

    /// Why the library's files are not being read, when something is wrong.
    pub(crate) fn failure(&self, section: SectionId) -> Option<String> {
        let state = self.shared.lock();
        state
            .unavailable
            .clone()
            .or_else(|| state.paused.get(&section).cloned().flatten())
    }

    pub(crate) async fn progress(&self, section: SectionId) -> Result<AnalysisProgress, ScanError> {
        self.shared.store.analysis_progress(section).await
    }
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Reads what is left of one file and stores it.
    async fn read(&self, file: &QueuedFile) -> Result<Outcome, ScanError> {
        let location = match self.locate(file).await {
            Ok(location) => location,
            Err(reason) => {
                self.unreachable(file, &reason);
                return Ok(Outcome::Left);
            }
        };
        let now = now_epoch();
        let stored = match file.analysis {
            FileAnalysis::Pending => {
                let analysis = match self.analyzer.read_headers(&location).await {
                    Ok(analysis) => Some(analysis),
                    Err(error) if self.leaves_queued(file, &error) => return Ok(Outcome::Left),
                    Err(_) => None,
                };
                self.store
                    .store_headers(file, analysis.as_ref(), now)
                    .await?
            }
            FileAnalysis::HeadersRead => {
                let measured = match self.analyzer.measure(&location, file.duration_ms).await {
                    Ok(measured) => Some(measured),
                    Err(error) if self.leaves_queued(file, &error) => return Ok(Outcome::Left),
                    Err(_) => None,
                };
                self.store
                    .store_measurement(file, measured.as_ref(), now)
                    .await?
            }
            FileAnalysis::Complete => return Ok(Outcome::Done),
        };
        if stored == Stored::Written {
            self.count_change(file).await?;
        }
        Ok(Outcome::Done)
    }

    /// Whether a failed read leaves the file queued — for its library's next scan, or for
    /// tools that can run — rather than stored as far as it was read.
    fn leaves_queued(&self, file: &QueuedFile, error: &AnalyzeError) -> bool {
        match error {
            AnalyzeError::Unavailable(reason) => {
                tracing::error!(%reason, "files cannot be read until the media tools run");
                self.lock().unavailable = Some(reason.clone());
                true
            }
            AnalyzeError::Unreachable(reason) => {
                self.unreachable(file, reason);
                true
            }
            AnalyzeError::Unreadable(reason) => {
                tracing::warn!(path = %file.path, %reason, "file is not readable as media");
                false
            }
            error @ AnalyzeError::Failed(_) => {
                tracing::error!(path = %file.path, %error, "media analysis failed");
                false
            }
        }
    }

    /// A file that cannot be opened is on a disk that went away, most likely: an unmounted
    /// disk is not a file without media, so its library waits for its next scan.
    fn unreachable(&self, file: &QueuedFile, reason: &str) {
        tracing::warn!(path = %file.path, %reason, "cannot open the file; its library waits for its next scan");
        self.lock().paused.insert(
            file.section,
            Some(format!("cannot open {}: {reason}", file.path)),
        );
    }

    async fn locate(&self, file: &QueuedFile) -> Result<FileLocation, String> {
        let roots = self
            .library
            .roots_for(&[file.section])
            .await
            .map_err(|error| error.to_string())?;
        let (root, path) = roots
            .iter()
            .find_map(|root| {
                let path = self.files.relative_path(&root.path, &file.path)?;
                Some((root.path.as_str(), path))
            })
            .ok_or_else(|| "below none of its library's roots".to_owned())?;
        self.files
            .locate(root, &path)
            .await
            .map_err(|error| error.to_string())
    }

    /// Counts a change of the library once a round of reading it is over — every file's
    /// headers, then every file's packets — rather than once per file: listings show what
    /// the headers give, and a client refreshes them on each change.
    async fn count_change(&self, file: &QueuedFile) -> Result<(), ScanError> {
        let progress = self.store.analysis_progress(file.section).await?;
        let round_over = match file.analysis {
            FileAnalysis::Pending => progress.read == progress.files,
            _ => progress.complete == progress.files,
        };
        if round_over {
            self.store.contents_changed(file.section).await?;
            tracing::info!(
                section = file.section.get(),
                files = progress.files,
                measured = progress.complete == progress.files,
                "library files read"
            );
        }
        Ok(())
    }
}

impl State {
    fn reads(&self, section: SectionId) -> bool {
        self.busy.values().any(|busy| busy.section == section)
    }

    /// Aborts the reads of the library: dropping one kills the tool it runs. Its files stay
    /// queued. Whether any was being read.
    fn abort(&mut self, section: SectionId) -> bool {
        let mut aborted = false;
        for busy in self.busy.values().filter(|busy| busy.section == section) {
            busy.task.abort();
            aborted = true;
        }
        aborted
    }
}

/// Takes a read off the busy list however its task ends, aborted included, and lets the loop
/// take the next one.
struct Done {
    shared: Arc<Shared>,
    part: PartId,
}

impl Drop for Done {
    fn drop(&mut self) {
        self.shared.lock().busy.remove(&self.part);
        self.shared.wake.notify_one();
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use klapz_scanner::report::{Analysis, DeepAnalysis, FileIdentity};

    use crate::files::{Directory, DirectoryListing, Entry, FilesError, RelativePath};
    use crate::ids::RatingKey;
    use crate::library::{LibrarySection, SectionKind, SectionRoot};
    use crate::scan::{KnownPart, NewItem, NewPart};
    use crate::testing::InMemoryLibrary;

    const FILMS: SectionId = SectionId::new(1);
    const SERIES: SectionId = SectionId::new(2);

    /// A queue in memory: each file's state, and what was stored for it.
    #[derive(Debug, Default)]
    struct Queue {
        files: Mutex<Vec<QueuedFile>>,
        headers: Mutex<Vec<(PartId, bool)>>,
        measured: Mutex<Vec<(PartId, bool)>>,
        changes: Mutex<Vec<SectionId>>,
    }

    impl Queue {
        fn with(files: &[(i64, SectionId)]) -> Self {
            Self {
                files: Mutex::new(
                    files
                        .iter()
                        .map(|(part, section)| QueuedFile {
                            part: PartId::new(*part),
                            section: *section,
                            path: format!("/media/{part}.mkv"),
                            size: 100,
                            modified_at: 10,
                            analysis: FileAnalysis::Pending,
                            duration_ms: None,
                        })
                        .collect(),
                ),
                ..Self::default()
            }
        }

        fn state(&self, part: i64) -> FileAnalysis {
            self.files
                .lock()
                .unwrap()
                .iter()
                .find(|file| file.part.get() == part)
                .map_or(FileAnalysis::Complete, |file| file.analysis)
        }

        /// The file as storage holds it, against what the read started from.
        fn current(&self, file: &QueuedFile) -> Option<usize> {
            self.files
                .lock()
                .unwrap()
                .iter()
                .position(|queued| queued == file)
        }
    }

    #[async_trait::async_trait]
    impl ScanRepository for Queue {
        async fn known_parts(
            &self,
            _section: SectionId,
            _prefix: &str,
        ) -> Result<Vec<KnownPart>, ScanError> {
            Ok(Vec::new())
        }

        async fn add_item(
            &self,
            _section: SectionId,
            _item: NewItem,
            _now: i64,
        ) -> Result<RatingKey, ScanError> {
            Ok(RatingKey::new(1))
        }

        async fn update_part(
            &self,
            part: PartId,
            file: NewPart,
            _now: i64,
        ) -> Result<(), ScanError> {
            for queued in self.files.lock().unwrap().iter_mut() {
                if queued.part == part {
                    queued.size = file.size;
                    queued.modified_at = file.modified_at;
                    queued.analysis = FileAnalysis::Pending;
                }
            }
            Ok(())
        }

        async fn queue_again(&self, _parts: &[PartId], _now: i64) -> Result<(), ScanError> {
            Ok(())
        }

        async fn remove_parts(&self, _parts: &[PartId]) -> Result<(), ScanError> {
            Ok(())
        }

        async fn finish(
            &self,
            _section: SectionId,
            _at: i64,
            _changed: bool,
        ) -> Result<(), ScanError> {
            Ok(())
        }

        async fn queued_files(
            &self,
            limit: usize,
            filter: &QueueFilter,
        ) -> Result<Vec<QueuedFile>, ScanError> {
            let mut files: Vec<QueuedFile> = self
                .files
                .lock()
                .unwrap()
                .iter()
                .filter(|file| filter.section.is_none_or(|section| section == file.section))
                .filter(|file| !filter.busy.contains(&file.part))
                .filter(|file| !filter.held.contains(&file.section))
                .cloned()
                .collect();
            files.sort_by_key(|file| file.analysis != FileAnalysis::Pending);
            files.truncate(limit);
            Ok(files)
        }

        async fn queued_file(&self, part: PartId) -> Result<Option<QueuedFile>, ScanError> {
            Ok(self
                .files
                .lock()
                .unwrap()
                .iter()
                .find(|file| file.part == part)
                .cloned())
        }

        async fn store_headers(
            &self,
            file: &QueuedFile,
            analysis: Option<&Analysis>,
            _now: i64,
        ) -> Result<Stored, ScanError> {
            let Some(index) = self.current(file) else {
                return Ok(Stored::Stale);
            };
            let mut files = self.files.lock().unwrap();
            if analysis.is_some() {
                files[index].analysis = FileAnalysis::HeadersRead;
            } else {
                files.remove(index);
            }
            self.headers
                .lock()
                .unwrap()
                .push((file.part, analysis.is_some()));
            Ok(Stored::Written)
        }

        async fn store_measurement(
            &self,
            file: &QueuedFile,
            measured: Option<&DeepAnalysis>,
            _now: i64,
        ) -> Result<Stored, ScanError> {
            let Some(index) = self.current(file) else {
                return Ok(Stored::Stale);
            };
            self.files.lock().unwrap().remove(index);
            self.measured
                .lock()
                .unwrap()
                .push((file.part, measured.is_some()));
            Ok(Stored::Written)
        }

        async fn analysis_progress(
            &self,
            section: SectionId,
        ) -> Result<AnalysisProgress, ScanError> {
            let files = self.files.lock().unwrap();
            let of = |state: FileAnalysis| {
                files
                    .iter()
                    .filter(|file| file.section == section && file.analysis == state)
                    .count() as u64
            };
            let (pending, measuring) = (of(FileAnalysis::Pending), of(FileAnalysis::HeadersRead));
            Ok(AnalysisProgress {
                files: 10,
                read: 10 - pending,
                complete: 10 - pending - measuring,
            })
        }

        async fn contents_changed(&self, section: SectionId) -> Result<(), ScanError> {
            self.changes.lock().unwrap().push(section);
            Ok(())
        }
    }

    /// Reads that end when the test says so, counting how many run at once.
    #[derive(Debug, Default)]
    struct Reader {
        running: AtomicUsize,
        most: AtomicUsize,
        reads: Mutex<Vec<(String, &'static str)>>,
        gate: Option<Arc<Notify>>,
        failing: Option<fn() -> AnalyzeError>,
    }

    impl Reader {
        async fn read(&self, file: &FileLocation, step: &'static str) -> Result<(), AnalyzeError> {
            let FileLocation::Local(path) = file;
            self.reads
                .lock()
                .unwrap()
                .push((path.to_string_lossy().into_owned(), step));
            let running = self.running.fetch_add(1, Ordering::SeqCst) + 1;
            self.most.fetch_max(running, Ordering::SeqCst);
            if let Some(gate) = &self.gate {
                gate.notified().await;
            }
            self.running.fetch_sub(1, Ordering::SeqCst);
            match self.failing {
                Some(error) => Err(error()),
                None => Ok(()),
            }
        }

        fn steps(&self, step: &str) -> usize {
            self.reads
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, done)| *done == step)
                .count()
        }
    }

    #[async_trait::async_trait]
    impl MediaAnalyzer for Reader {
        async fn read_headers(&self, file: &FileLocation) -> Result<Analysis, AnalyzeError> {
            self.read(file, "headers").await?;
            Ok(Analysis {
                file: FileIdentity {
                    size: 100,
                    modified_at: 10,
                    content_hash: None,
                    open_subtitles_hash: None,
                },
                container: "matroska,webm".to_owned(),
                duration_ms: Some(1000),
                duration_us: Some(1_000_000),
                bitrate: None,
                optimized_for_streaming: None,
                has_64bit_offsets: None,
                packet_length: None,
                tracks: Vec::new(),
                chapters: Vec::new(),
                deep: None,
            })
        }

        async fn measure(
            &self,
            file: &FileLocation,
            _duration_ms: Option<u64>,
        ) -> Result<DeepAnalysis, AnalyzeError> {
            self.read(file, "packets").await?;
            Ok(DeepAnalysis {
                duration_ms: Some(999),
                bitrate: 8000,
                tracks: Vec::new(),
            })
        }
    }

    /// Files below `/media`, each found where its path says.
    #[derive(Debug)]
    struct Disk;

    #[async_trait::async_trait]
    impl LibraryFiles for Disk {
        async fn list(&self, _root: &str, _dir: &RelativePath) -> Result<Vec<Entry>, FilesError> {
            Ok(Vec::new())
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

        async fn starting_points(&self) -> Result<Vec<Directory>, FilesError> {
            Ok(Vec::new())
        }

        async fn list_directories(
            &self,
            _path: &str,
            _limit: usize,
        ) -> Result<DirectoryListing, FilesError> {
            Err(FilesError::NotFound)
        }
    }

    fn library() -> Arc<InMemoryLibrary> {
        let section = |id: SectionId, kind| LibrarySection {
            id,
            kind,
            title: "Library".to_owned(),
            title_sort: None,
            language: crate::language::LanguageTag::parse("fr-FR").unwrap(),
            uuid: "uuid".to_owned(),
            created_at: 0,
            updated_at: 0,
            scanned_at: None,
            content_changed_at: 0,
            hidden: false,
        };
        let root = |id: i64, section: SectionId| SectionRoot {
            id,
            section_id: section,
            path: "/media".to_owned(),
            created_at: 0,
        };
        Arc::new(
            InMemoryLibrary::new(vec![
                section(FILMS, SectionKind::Movie),
                section(SERIES, SectionKind::Show),
            ])
            .with_roots(vec![root(1, FILMS), root(2, SERIES)]),
        )
    }

    fn analysis(
        queue: &Arc<Queue>,
        reader: &Arc<Reader>,
        concurrency: usize,
    ) -> BackgroundAnalysis {
        BackgroundAnalysis::new(
            Arc::clone(queue) as Arc<dyn ScanRepository>,
            library(),
            Arc::new(Disk),
            Arc::clone(reader) as Arc<dyn MediaAnalyzer>,
            concurrency,
        )
    }

    /// Lets the tasks spawned so far run until they wait.
    async fn settle() {
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test]
    async fn reads_every_files_headers_before_any_files_packets() {
        let queue = Arc::new(Queue::with(&[(1, FILMS), (2, FILMS), (3, FILMS)]));
        let reader = Arc::new(Reader::default());
        analysis(&queue, &reader, 1)
            .analyze_library(FILMS)
            .await
            .unwrap();

        let steps: Vec<&str> = reader
            .reads
            .lock()
            .unwrap()
            .iter()
            .map(|(_, step)| *step)
            .collect();
        assert_eq!(
            steps,
            ["headers", "headers", "headers", "packets", "packets", "packets"]
        );
        assert!(queue.files.lock().unwrap().is_empty());
        assert_eq!(
            *queue.changes.lock().unwrap(),
            [FILMS, FILMS],
            "one change when every header is read, one when every packet is"
        );
    }

    #[tokio::test]
    async fn reads_no_more_files_at_once_than_it_is_given() {
        let files: Vec<(i64, SectionId)> = (1..=6).map(|part| (part, FILMS)).collect();
        let queue = Arc::new(Queue::with(&files));
        let gate = Arc::new(Notify::new());
        let reader = Arc::new(Reader {
            gate: Some(Arc::clone(&gate)),
            ..Reader::default()
        });
        let analysis = analysis(&queue, &reader, 2);
        let worker = analysis.spawn();
        settle().await;
        assert_eq!(reader.running.load(Ordering::SeqCst), 2);
        for _ in 0..40 {
            gate.notify_waiters();
            settle().await;
        }
        assert_eq!(reader.most.load(Ordering::SeqCst), 2);
        assert!(queue.files.lock().unwrap().is_empty());
        worker.abort();
    }

    #[tokio::test]
    async fn a_library_being_walked_or_paused_is_not_read() {
        let queue = Arc::new(Queue::with(&[(1, FILMS), (2, SERIES)]));
        let reader = Arc::new(Reader::default());
        let analysis = analysis(&queue, &reader, 2);
        analysis.walking(FILMS);
        analysis.pause(SERIES);
        let worker = analysis.spawn();
        settle().await;
        assert!(reader.reads.lock().unwrap().is_empty());

        analysis.walked(FILMS);
        settle().await;
        assert_eq!(queue.state(1), FileAnalysis::Complete);
        assert_eq!(
            queue.state(2),
            FileAnalysis::Pending,
            "paused until its next scan"
        );

        analysis.walking(SERIES);
        analysis.walked(SERIES);
        settle().await;
        assert_eq!(queue.state(2), FileAnalysis::Complete);
        worker.abort();
    }

    #[tokio::test]
    async fn pausing_a_library_stops_the_reads_under_way_and_keeps_them_queued() {
        let queue = Arc::new(Queue::with(&[(1, FILMS), (2, SERIES)]));
        let gate = Arc::new(Notify::new());
        let reader = Arc::new(Reader {
            gate: Some(Arc::clone(&gate)),
            ..Reader::default()
        });
        let analysis = analysis(&queue, &reader, 2);
        let worker = analysis.spawn();
        settle().await;
        assert!(analysis.reads(FILMS));

        assert!(analysis.pause(FILMS));
        settle().await;
        assert!(!analysis.reads(FILMS));
        assert!(analysis.reads(SERIES), "another library goes on");
        assert_eq!(queue.state(1), FileAnalysis::Pending);
        assert!(queue.headers.lock().unwrap().is_empty());
        worker.abort();
    }

    #[tokio::test]
    async fn a_result_read_from_a_file_that_changed_meanwhile_is_dropped() {
        let queue = Arc::new(Queue::with(&[(1, FILMS)]));
        let gate = Arc::new(Notify::new());
        let reader = Arc::new(Reader {
            gate: Some(Arc::clone(&gate)),
            ..Reader::default()
        });
        let analysis = analysis(&queue, &reader, 1);
        let worker = analysis.spawn();
        settle().await;

        // A rescan finds the file changed while its headers are being read.
        queue
            .update_part(
                PartId::new(1),
                NewPart {
                    path: "/media/1.mkv".to_owned(),
                    size: 150,
                    modified_at: 20,
                    sidecars: Vec::new(),
                },
                0,
            )
            .await
            .unwrap();
        gate.notify_waiters();
        settle().await;
        assert_eq!(reader.steps("headers"), 2, "read again, as it now is");
        gate.notify_waiters();
        settle().await;
        assert_eq!(
            queue.headers.lock().unwrap().len(),
            1,
            "the first read was not stored"
        );
        worker.abort();
    }

    #[tokio::test]
    async fn a_file_that_is_not_media_is_listed_and_not_read_again() {
        let queue = Arc::new(Queue::with(&[(1, FILMS)]));
        let reader = Arc::new(Reader {
            failing: Some(|| AnalyzeError::Unreadable("invalid data".to_owned())),
            ..Reader::default()
        });
        analysis(&queue, &reader, 1)
            .analyze_library(FILMS)
            .await
            .unwrap();
        assert_eq!(*queue.headers.lock().unwrap(), [(PartId::new(1), false)]);
        assert_eq!(queue.state(1), FileAnalysis::Complete);
    }

    #[tokio::test]
    async fn missing_tools_leave_the_queue_as_it_is() {
        let queue = Arc::new(Queue::with(&[(1, FILMS), (2, FILMS)]));
        let reader = Arc::new(Reader {
            failing: Some(|| AnalyzeError::Unavailable("no ffprobe".to_owned())),
            ..Reader::default()
        });
        let analysis = analysis(&queue, &reader, 1);
        let stopped = analysis.analyze_library(FILMS).await;
        assert!(
            matches!(&stopped, Err(ScanError::AnalyzerUnavailable(reason)) if reason == "no ffprobe"),
            "{stopped:?}"
        );
        assert_eq!(reader.steps("headers"), 1, "the next file is not tried");
        assert_eq!(queue.files.lock().unwrap().len(), 2);
        assert_eq!(analysis.failure(FILMS).as_deref(), Some("no ffprobe"));

        // A scan's start tries again.
        analysis.walking(FILMS);
        assert_eq!(analysis.failure(FILMS), None);
    }

    #[tokio::test]
    async fn a_file_that_cannot_be_opened_waits_for_its_librarys_next_scan() {
        let queue = Arc::new(Queue::with(&[(1, FILMS), (2, FILMS), (3, SERIES)]));
        let reader = Arc::new(Reader {
            failing: Some(|| AnalyzeError::Unreachable("no such file".to_owned())),
            ..Reader::default()
        });
        let analysis = analysis(&queue, &reader, 1);
        analysis.analyze_library(FILMS).await.unwrap();
        assert_eq!(reader.steps("headers"), 1);
        assert_eq!(
            queue.files.lock().unwrap().len(),
            3,
            "nothing is taken off the queue"
        );
        assert!(analysis.failure(FILMS).unwrap().contains("no such file"));
        assert_eq!(analysis.failure(SERIES), None);
    }

    #[tokio::test]
    async fn a_player_has_a_files_headers_read_at_once() {
        let queue = Arc::new(Queue::with(&[(1, FILMS), (2, FILMS)]));
        let reader = Arc::new(Reader::default());
        let analysis = analysis(&queue, &reader, 1);
        // No worker runs: the read is the player's own.
        assert!(analysis.read_now(PartId::new(2)).await.unwrap());
        assert_eq!(queue.state(2), FileAnalysis::HeadersRead);
        assert_eq!(queue.state(1), FileAnalysis::Pending);

        assert!(
            analysis.read_now(PartId::new(2)).await.unwrap(),
            "read already"
        );
        assert!(
            analysis.read_now(PartId::new(9)).await.unwrap(),
            "not queued"
        );
        assert_eq!(reader.steps("headers"), 1);
    }

    #[tokio::test]
    async fn a_player_is_told_when_a_file_cannot_be_read_yet() {
        let queue = Arc::new(Queue::with(&[(1, FILMS)]));
        let reader = Arc::new(Reader {
            failing: Some(|| AnalyzeError::Unreachable("no such file".to_owned())),
            ..Reader::default()
        });
        assert!(!analysis(&queue, &reader, 1)
            .read_now(PartId::new(1))
            .await
            .unwrap());
        assert_eq!(queue.state(1), FileAnalysis::Pending);
    }
}
