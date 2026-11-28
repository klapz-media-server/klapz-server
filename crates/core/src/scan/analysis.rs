//! Working through the tasks scans queue, in the background, a few files at a time: every
//! file's headers before anything else. Headers take a fraction of a second and make a file
//! playable and fully described; the rest reads a whole file each — its packets, which refine
//! its rates and duration (`docs/plex/scanner/analysis.md`, §1 and §2), its subtitle tracks, its
//! seeking thumbnails (§4) — and is done where its setting is on, read before every round so
//! that a change applies from the next file.
//!
//! Each kind of task runs as many files at once as its setting says
//! (`analysis.concurrency.*`), apart from the others: a long measurement holds no slot of the
//! headers, so a new library is described while the last one is still being read. A file has
//! one task done at a time.
//!
//! The queue is storage's, so it outlives the process: what a stopped server did not do is
//! done when it starts again. A task turned off stays queued, and is done once turned on.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::{Notify, Semaphore};
use tokio::task::{AbortHandle, JoinHandle};

use crate::files::{FileLocation, LibraryFiles};
use crate::ids::{PartId, SectionId};
use crate::library::LibraryRepository;
use crate::now_epoch;
use crate::scan::{
    AnalysisProgress, AnalyzeError, Extraction, MediaAnalyzer, MediaExtractor, MediaTask,
    QueueFilter, QueuedFile, ScanError, ScanRepository, Stored, TaskSwitch,
};
use crate::settings::{self, SettingsRepository};

/// Reads a player asks for at once, besides the queue's: a viewer does not wait behind a
/// measurement that reads a whole film, and a client cannot start more than this many.
const ON_DEMAND: usize = 4;

/// How long the queue waits after storage failed to hand it files.
const STORAGE_RETRY: Duration = Duration::from_secs(30);

/// What the queue reads files and settings with.
#[derive(Debug, Clone)]
pub struct AnalysisTools {
    pub analyzer: Arc<dyn MediaAnalyzer>,
    pub extractor: Arc<dyn MediaExtractor>,
    pub settings: Arc<dyn SettingsRepository>,
}

/// Works through what scans queue; one per server, cheap to clone.
#[derive(Debug, Clone)]
pub struct BackgroundAnalysis {
    shared: Arc<Shared>,
}

#[derive(Debug)]
struct Shared {
    store: Arc<dyn ScanRepository>,
    library: Arc<dyn LibraryRepository>,
    files: Arc<dyn LibraryFiles>,
    tools: AnalysisTools,
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
    work: MediaTask,
    task: AbortHandle,
}

/// What working on one file came to.
enum Outcome {
    /// Stored as far as it went, or found done already or changed: this task is over.
    Done,
    /// Nothing was written; the task stays queued as it was.
    Left,
}

impl BackgroundAnalysis {
    pub fn new(
        store: Arc<dyn ScanRepository>,
        library: Arc<dyn LibraryRepository>,
        files: Arc<dyn LibraryFiles>,
        tools: AnalysisTools,
    ) -> Self {
        Self {
            shared: Arc::new(Shared {
                store,
                library,
                files,
                tools,
                state: Mutex::default(),
                wake: Notify::new(),
                on_demand: Semaphore::new(ON_DEMAND),
            }),
        }
    }

    /// Works through the queue as long as the server runs, starting with what a stopped one
    /// left.
    pub fn spawn(&self) -> JoinHandle<()> {
        let analysis = self.clone();
        tokio::spawn(async move {
            // Without a library in scope, the loop only ends with the runtime.
            let _ = analysis.run(None).await;
        })
    }

    /// Works through one library's queue to its end, outside a server: what its settings
    /// turn off is left queued.
    ///
    /// # Errors
    ///
    /// [`ScanError::AnalyzerUnavailable`] when the tools cannot run, what was left queued.
    pub async fn analyze_library(&self, section: SectionId) -> Result<(), ScanError> {
        self.run(Some(section)).await
    }

    /// The loop that takes tasks off the queue. One runs per instance: wake-ups go to one
    /// waiter.
    async fn run(&self, scope: Option<SectionId>) -> Result<(), ScanError> {
        loop {
            let woken = self.shared.wake.notified();
            let settings = match self.shared.round_settings().await {
                Ok(settings) => settings,
                Err(error) => {
                    tracing::error!(%error, "cannot read the analysis settings");
                    tokio::time::sleep(STORAGE_RETRY).await;
                    continue;
                }
            };
            let mut taken = 0;
            for (work, limit) in MediaTask::ALL.into_iter().zip(settings.limits) {
                let (free, filter) = {
                    let state = self.shared.lock();
                    if let (Some(reason), Some(_)) = (&state.unavailable, scope) {
                        return Err(ScanError::AnalyzerUnavailable(reason.clone()));
                    }
                    if state.unavailable.is_some() {
                        break;
                    }
                    let running = state.busy.values().filter(|busy| busy.work == work).count();
                    let filter = QueueFilter {
                        section: scope,
                        task: Some(work),
                        // Read again for each task: what the one before started is busy now.
                        busy: state.busy.keys().copied().collect(),
                        held: state
                            .walking
                            .iter()
                            .chain(state.paused.keys())
                            .copied()
                            .collect(),
                        switches: settings.switches.clone(),
                    };
                    (limit.saturating_sub(running), filter)
                };
                if free == 0 {
                    continue;
                }
                match self.shared.store.queued_files(free, &filter).await {
                    Ok(files) => {
                        taken += files.len();
                        for file in files {
                            self.start(file);
                        }
                    }
                    Err(error) => {
                        tracing::error!(%error, "cannot read the analysis queue");
                        tokio::time::sleep(STORAGE_RETRY).await;
                        break;
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
        let (part, section, work) = (file.part, file.section, file.task);
        // Spawned under the lock, so that the task, which removes its entry as it ends, cannot
        // end before the entry is there.
        let task = tokio::spawn(async move {
            let _done = Done {
                shared: Arc::clone(&shared),
                part,
            };
            if let Err(error) = shared.work(&file).await {
                tracing::error!(
                    path = %file.path,
                    task = ?file.task,
                    %error,
                    "cannot store what a file's task came to"
                );
            }
        });
        state.busy.insert(
            part,
            Busy {
                section,
                work,
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
        if file.task != MediaTask::Headers {
            return Ok(true);
        }
        let Ok(_permit) = self.shared.on_demand.acquire().await else {
            return Ok(false);
        };
        // The queue may be reading it too: whichever stores first is kept, the other is stale.
        Ok(match self.shared.work(&file).await? {
            Outcome::Done => true,
            Outcome::Left => false,
        })
    }

    /// A setting changed: what it turned on is taken at once.
    pub(crate) fn settings_changed(&self) {
        self.shared.wake.notify_one();
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

    /// How far the library's files are, counting the tasks its settings turn on.
    ///
    /// # Errors
    ///
    /// Storage's failure.
    pub async fn progress(&self, section: SectionId) -> Result<AnalysisProgress, ScanError> {
        let counted: Vec<MediaTask> = self
            .shared
            .switches()
            .await
            .map_err(|error| ScanError::Storage(Box::new(error)))?
            .into_iter()
            .filter(|switch| switch.switch.is_on(section))
            .map(|switch| switch.task)
            .collect();
        self.shared.store.analysis_progress(section, &counted).await
    }
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// What a round of the queue follows: how many of each task may run, in
    /// [`MediaTask::ALL`]'s order, and where each optional one is on.
    async fn round_settings(&self) -> Result<RoundSettings, settings::SettingsError> {
        let definitions = MediaTask::ALL.map(MediaTask::concurrency);
        let limits = settings::values_of(self.tools.settings.as_ref(), &definitions)
            .await?
            .iter()
            .map(|value| value.parse::<usize>().unwrap_or(1).max(1))
            .collect();
        Ok(RoundSettings {
            limits,
            switches: self.switches().await?,
        })
    }

    async fn switches(&self) -> Result<Vec<TaskSwitch>, settings::SettingsError> {
        let (tasks, definitions): (Vec<MediaTask>, Vec<_>) = MediaTask::OPTIONAL
            .into_iter()
            .filter_map(|task| Some((task, task.setting()?)))
            .unzip();
        let switches = settings::switches(self.tools.settings.as_ref(), &definitions).await?;
        Ok(tasks
            .into_iter()
            .zip(switches)
            .map(|(task, switch)| TaskSwitch { task, switch })
            .collect())
    }

    /// Does the file's task and stores what it came to.
    async fn work(&self, file: &QueuedFile) -> Result<Outcome, ScanError> {
        let location = match self.locate(file).await {
            Ok(location) => location,
            Err(reason) => {
                self.unreachable(file, &reason);
                return Ok(Outcome::Left);
            }
        };
        let now = now_epoch();
        let analyzer = &self.tools.analyzer;
        let stored = match file.task {
            MediaTask::Headers => {
                let analysis = match analyzer.read_headers(&location).await {
                    Ok(analysis) => Some(analysis),
                    Err(error) if self.leaves_queued(file, &error) => return Ok(Outcome::Left),
                    Err(_) => None,
                };
                self.store
                    .store_headers(file, analysis.as_ref(), now)
                    .await?
            }
            MediaTask::Packets => {
                let measured = match analyzer.measure(&location, file.duration_ms).await {
                    Ok(measured) => Some(measured),
                    Err(error) if self.leaves_queued(file, &error) => return Ok(Outcome::Left),
                    Err(_) => None,
                };
                self.store
                    .store_measurement(file, measured.as_ref(), now)
                    .await?
            }
            MediaTask::Subtitles | MediaTask::PreviewThumbnails => {
                if let Outcome::Left = self.extract(file, &location).await {
                    return Ok(Outcome::Left);
                }
                self.store.finish_task(file).await?
            }
        };
        if stored == Stored::Written {
            self.count_change(file).await?;
        }
        Ok(Outcome::Done)
    }

    /// Makes the task's files, unless they are kept already. A failure that is the file's
    /// alone ends the task, as a measurement that cannot finish does.
    async fn extract(&self, file: &QueuedFile, location: &FileLocation) -> Outcome {
        let Some(hash) = &file.content_hash else {
            // An ffmpeg-plex too old to state it: a forced scan with a newer one reads it.
            tracing::warn!(
                path = %file.path,
                task = ?file.task,
                "no content hash to keep the extracted files by"
            );
            return Outcome::Done;
        };
        let extractor = &self.tools.extractor;
        let extracted = if file.task == MediaTask::Subtitles {
            extractor.extract_subtitles(location, hash).await
        } else {
            extractor.generate_preview_thumbnails(location, hash).await
        };
        match extracted {
            Ok(extraction) => {
                tracing::debug!(
                    path = %file.path,
                    task = ?file.task,
                    kept_already = extraction == Extraction::AlreadyKept,
                    "file extracted"
                );
                Outcome::Done
            }
            Err(error) if self.leaves_queued(file, &error) => Outcome::Left,
            Err(_) => Outcome::Done,
        }
    }

    /// Whether a failure leaves the task queued — for its library's next scan, or for tools
    /// that can run — rather than over as far as it went.
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
                tracing::warn!(path = %file.path, task = ?file.task, %reason, "file is not readable as media");
                false
            }
            error @ AnalyzeError::Failed(_) => {
                tracing::error!(path = %file.path, task = ?file.task, %error, "media analysis failed");
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
    /// the headers give, and a client refreshes them on each change. What is extracted shows
    /// in no listing.
    async fn count_change(&self, file: &QueuedFile) -> Result<(), ScanError> {
        let round_over = match file.task {
            MediaTask::Headers => {
                let progress = self.store.analysis_progress(file.section, &[]).await?;
                progress.read == progress.files
            }
            MediaTask::Packets => {
                let progress = self
                    .store
                    .analysis_progress(file.section, &[MediaTask::Packets])
                    .await?;
                progress.complete == progress.files
            }
            MediaTask::Subtitles | MediaTask::PreviewThumbnails => false,
        };
        if round_over {
            self.store.contents_changed(file.section).await?;
            tracing::info!(
                section = file.section.get(),
                task = ?file.task,
                "library files read"
            );
        }
        Ok(())
    }
}

struct RoundSettings {
    limits: Vec<usize>,
    switches: Vec<TaskSwitch>,
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
    use crate::ids::{RatingKey, UserId};
    use crate::library::{LibrarySection, SectionKind, SectionRoot};
    use crate::scan::{ContentHash, KnownPart, NewItem, NewPart};
    use crate::settings::SettingUpdate;
    use crate::testing::{InMemoryLibrary, InMemorySettings};

    const FILMS: SectionId = SectionId::new(1);
    const SERIES: SectionId = SectionId::new(2);
    const HASH: &str = "0123456789abcdef0123456789abcdef01234567";

    /// A queue in memory: one entry per task left on a file, and what was stored.
    #[derive(Debug, Default)]
    struct Queue {
        tasks: Mutex<Vec<QueuedFile>>,
        headers: Mutex<Vec<(PartId, bool)>>,
        measured: Mutex<Vec<(PartId, bool)>>,
        finished: Mutex<Vec<(PartId, MediaTask)>>,
        changes: Mutex<Vec<SectionId>>,
    }

    impl Queue {
        fn with(files: &[(i64, SectionId)]) -> Self {
            Self {
                tasks: Mutex::new(
                    files
                        .iter()
                        .map(|(part, section)| QueuedFile {
                            part: PartId::new(*part),
                            section: *section,
                            path: format!("/media/{part}.mkv"),
                            size: 100,
                            modified_at: 10,
                            task: MediaTask::Headers,
                            duration_ms: None,
                            content_hash: None,
                        })
                        .collect(),
                ),
                ..Self::default()
            }
        }

        /// The tasks left on a file, in the queue's order.
        fn left(&self, part: i64) -> Vec<MediaTask> {
            let mut left: Vec<MediaTask> = self
                .tasks
                .lock()
                .unwrap()
                .iter()
                .filter(|queued| queued.part.get() == part)
                .map(|queued| queued.task)
                .collect();
            left.sort();
            left
        }

        fn take(&self, file: &QueuedFile) -> bool {
            let mut tasks = self.tasks.lock().unwrap();
            let Some(index) = tasks.iter().position(|queued| queued == file) else {
                return false;
            };
            tasks.remove(index);
            true
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
            let mut tasks = self.tasks.lock().unwrap();
            let Some(first) = tasks.iter().find(|queued| queued.part == part).cloned() else {
                return Ok(());
            };
            tasks.retain(|queued| queued.part != part);
            tasks.push(QueuedFile {
                size: file.size,
                modified_at: file.modified_at,
                task: MediaTask::Headers,
                content_hash: None,
                ..first
            });
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
            let mut tasks: Vec<QueuedFile> = self
                .tasks
                .lock()
                .unwrap()
                .iter()
                .filter(|queued| {
                    filter
                        .section
                        .is_none_or(|section| section == queued.section)
                })
                .filter(|queued| filter.task.is_none_or(|task| task == queued.task))
                .filter(|queued| !filter.busy.contains(&queued.part))
                .filter(|queued| !filter.held.contains(&queued.section))
                .filter(|queued| {
                    filter
                        .switches
                        .iter()
                        .filter(|switch| switch.task == queued.task)
                        .all(|switch| switch.switch.is_on(queued.section))
                })
                .cloned()
                .collect();
            tasks.sort_by_key(|queued| queued.task);
            let mut parts = HashSet::new();
            tasks.retain(|queued| parts.insert(queued.part));
            tasks.truncate(limit);
            Ok(tasks)
        }

        async fn queued_file(&self, part: PartId) -> Result<Option<QueuedFile>, ScanError> {
            Ok(self
                .tasks
                .lock()
                .unwrap()
                .iter()
                .filter(|queued| queued.part == part)
                .min_by_key(|queued| queued.task)
                .cloned())
        }

        async fn store_headers(
            &self,
            file: &QueuedFile,
            analysis: Option<&Analysis>,
            _now: i64,
        ) -> Result<Stored, ScanError> {
            if !self.take(file) {
                return Ok(Stored::Stale);
            }
            if let Some(analysis) = analysis {
                let follow = |task| QueuedFile {
                    task,
                    content_hash: analysis
                        .file
                        .content_hash
                        .as_deref()
                        .and_then(ContentHash::parse),
                    ..file.clone()
                };
                self.tasks.lock().unwrap().extend(
                    [
                        MediaTask::Packets,
                        MediaTask::Subtitles,
                        MediaTask::PreviewThumbnails,
                    ]
                    .map(follow),
                );
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
            if !self.take(file) {
                return Ok(Stored::Stale);
            }
            self.measured
                .lock()
                .unwrap()
                .push((file.part, measured.is_some()));
            Ok(Stored::Written)
        }

        async fn finish_task(&self, file: &QueuedFile) -> Result<Stored, ScanError> {
            if !self.take(file) {
                return Ok(Stored::Stale);
            }
            self.finished.lock().unwrap().push((file.part, file.task));
            Ok(Stored::Written)
        }

        async fn analysis_progress(
            &self,
            section: SectionId,
            counted: &[MediaTask],
        ) -> Result<AnalysisProgress, ScanError> {
            let tasks = self.tasks.lock().unwrap();
            let left = |of: &dyn Fn(MediaTask) -> bool| {
                tasks
                    .iter()
                    .filter(|queued| queued.section == section && of(queued.task))
                    .map(|queued| queued.part)
                    .collect::<HashSet<_>>()
                    .len() as u64
            };
            let unread = left(&|task| task == MediaTask::Headers);
            let incomplete = left(&|task| task == MediaTask::Headers || counted.contains(&task));
            Ok(AnalysisProgress {
                files: 10,
                read: 10 - unread,
                complete: 10 - incomplete,
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
        /// What the headers state as the file's content hash.
        hash: Option<&'static str>,
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

        fn steps(&self) -> Vec<&'static str> {
            self.reads
                .lock()
                .unwrap()
                .iter()
                .map(|(_, step)| *step)
                .collect()
        }

        fn count(&self, step: &str) -> usize {
            self.steps().iter().filter(|done| **done == step).count()
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
                    content_hash: self.hash.map(str::to_owned),
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

    #[async_trait::async_trait]
    impl MediaExtractor for Reader {
        async fn extract_subtitles(
            &self,
            file: &FileLocation,
            hash: &ContentHash,
        ) -> Result<Extraction, AnalyzeError> {
            assert_eq!(hash.as_str(), HASH);
            self.read(file, "subtitles").await?;
            Ok(Extraction::Written)
        }

        async fn generate_preview_thumbnails(
            &self,
            file: &FileLocation,
            hash: &ContentHash,
        ) -> Result<Extraction, AnalyzeError> {
            assert_eq!(hash.as_str(), HASH);
            self.read(file, "thumbnails").await?;
            Ok(Extraction::Written)
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
            language: "fr-FR".to_owned(),
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

    fn analysis_with(
        queue: &Arc<Queue>,
        reader: &Arc<Reader>,
        settings: &Arc<InMemorySettings>,
    ) -> BackgroundAnalysis {
        BackgroundAnalysis::new(
            Arc::clone(queue) as Arc<dyn ScanRepository>,
            library(),
            Arc::new(Disk),
            AnalysisTools {
                analyzer: Arc::clone(reader) as Arc<dyn MediaAnalyzer>,
                extractor: Arc::clone(reader) as Arc<dyn MediaExtractor>,
                settings: Arc::clone(settings) as Arc<dyn SettingsRepository>,
            },
        )
    }

    fn analysis(queue: &Arc<Queue>, reader: &Arc<Reader>) -> BackgroundAnalysis {
        analysis_with(queue, reader, &Arc::new(InMemorySettings::default()))
    }

    /// Settings under which each task runs these many files at once, in [`MediaTask::ALL`]'s
    /// order.
    async fn running(limits: [u32; 4]) -> Arc<InMemorySettings> {
        let settings = Arc::new(InMemorySettings::default());
        let updates: Vec<SettingUpdate> = MediaTask::ALL
            .into_iter()
            .zip(limits)
            .map(|(task, limit)| SettingUpdate {
                key: task.concurrency().key.to_owned(),
                value: Some(limit.to_string()),
            })
            .collect();
        settings::update_settings(settings.as_ref(), &updates, UserId::new(1), 0)
            .await
            .unwrap()
            .unwrap();
        settings
    }

    /// One file at a time, whatever the task.
    async fn one_at_a_time(queue: &Arc<Queue>, reader: &Arc<Reader>) -> BackgroundAnalysis {
        analysis_with(queue, reader, &running([1; 4]).await)
    }

    fn turn(key: &str, on: bool) -> SettingUpdate {
        SettingUpdate {
            key: key.to_owned(),
            value: Some(on.to_string()),
        }
    }

    /// Lets the tasks spawned so far run until they wait.
    async fn settle() {
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test]
    async fn does_every_task_of_every_file_one_task_of_a_file_at_a_time() {
        let queue = Arc::new(Queue::with(&[(1, FILMS), (2, FILMS), (3, FILMS)]));
        let reader = Arc::new(Reader {
            hash: Some(HASH),
            ..Reader::default()
        });
        one_at_a_time(&queue, &reader)
            .await
            .analyze_library(FILMS)
            .await
            .unwrap();

        let reads = reader.reads.lock().unwrap().clone();
        for part in 1..=3 {
            let steps: Vec<&str> = reads
                .iter()
                .filter(|(path, _)| *path == format!("/media/{part}.mkv"))
                .map(|(_, step)| *step)
                .collect();
            assert_eq!(steps.len(), 4, "{steps:?}");
            assert_eq!(steps[0], "headers", "what follows comes once they are read");
            for step in ["packets", "subtitles", "thumbnails"] {
                assert!(steps.contains(&step), "{step} of {part}: {steps:?}");
            }
        }
        assert!(
            reader.most.load(Ordering::SeqCst) <= 4,
            "one of each kind at most"
        );
        assert!(queue.tasks.lock().unwrap().is_empty());
        assert_eq!(
            *queue.changes.lock().unwrap(),
            [FILMS, FILMS],
            "one change when every header is read, one when every packet is; none for what \
             is extracted"
        );
    }

    #[tokio::test]
    async fn a_task_turned_off_stays_queued_and_is_done_once_turned_on() {
        let queue = Arc::new(Queue::with(&[(1, FILMS), (2, SERIES)]));
        let reader = Arc::new(Reader {
            hash: Some(HASH),
            ..Reader::default()
        });
        let settings = Arc::new(InMemorySettings::default());
        let owner = UserId::new(1);
        // Off for the server, on again for the series; thumbnails off for the films alone.
        settings::update_settings(
            settings.as_ref(),
            &[turn(settings::DEEP_ANALYSIS.key, false)],
            owner,
            1,
        )
        .await
        .unwrap()
        .unwrap();
        settings::update_library_settings(
            settings.as_ref(),
            SERIES,
            &[turn(settings::DEEP_ANALYSIS.key, true)],
            owner,
            1,
        )
        .await
        .unwrap()
        .unwrap();
        settings::update_library_settings(
            settings.as_ref(),
            FILMS,
            &[turn(settings::PREVIEW_THUMBNAILS.key, false)],
            owner,
            1,
        )
        .await
        .unwrap()
        .unwrap();
        let analysis = analysis_with(&queue, &reader, &settings);

        analysis.analyze_library(FILMS).await.unwrap();
        assert_eq!(reader.steps(), ["headers", "subtitles"]);
        assert_eq!(
            queue.left(1),
            [MediaTask::Packets, MediaTask::PreviewThumbnails]
        );
        let progress = analysis.progress(FILMS).await.unwrap();
        assert_eq!(
            (progress.read, progress.complete),
            (10, 10),
            "what is off does not count as left"
        );

        analysis.analyze_library(SERIES).await.unwrap();
        assert!(queue.left(2).is_empty(), "everything is on for the series");

        settings::update_settings(
            settings.as_ref(),
            &[turn(settings::DEEP_ANALYSIS.key, true)],
            owner,
            2,
        )
        .await
        .unwrap()
        .unwrap();
        analysis.analyze_library(FILMS).await.unwrap();
        assert_eq!(queue.left(1), [MediaTask::PreviewThumbnails]);
        assert_eq!(
            *queue.measured.lock().unwrap(),
            [(PartId::new(2), true), (PartId::new(1), true)]
        );
    }

    #[tokio::test]
    async fn a_file_whose_content_hash_is_unknown_is_not_extracted() {
        let queue = Arc::new(Queue::with(&[(1, FILMS)]));
        let reader = Arc::new(Reader::default());
        analysis(&queue, &reader)
            .analyze_library(FILMS)
            .await
            .unwrap();
        assert_eq!(reader.steps(), ["headers", "packets"]);
        assert_eq!(
            *queue.finished.lock().unwrap(),
            [
                (PartId::new(1), MediaTask::Subtitles),
                (PartId::new(1), MediaTask::PreviewThumbnails)
            ],
            "nothing to keep them by: the tasks end"
        );
    }

    #[tokio::test]
    async fn turning_a_task_on_wakes_the_queue() {
        let queue = Arc::new(Queue::with(&[(1, FILMS)]));
        let reader = Arc::new(Reader {
            hash: Some(HASH),
            ..Reader::default()
        });
        let settings = Arc::new(InMemorySettings::default());
        let owner = UserId::new(1);
        settings::update_settings(
            settings.as_ref(),
            &[turn(settings::SUBTITLE_EXTRACTION.key, false)],
            owner,
            1,
        )
        .await
        .unwrap()
        .unwrap();
        let analysis = analysis_with(&queue, &reader, &settings);
        let worker = analysis.spawn();
        settle().await;
        assert_eq!(queue.left(1), [MediaTask::Subtitles]);

        settings::update_settings(
            settings.as_ref(),
            &[turn(settings::SUBTITLE_EXTRACTION.key, true)],
            owner,
            2,
        )
        .await
        .unwrap()
        .unwrap();
        analysis.settings_changed();
        settle().await;
        assert!(queue.left(1).is_empty());
        worker.abort();
    }

    #[tokio::test]
    async fn each_task_runs_no_more_files_at_once_than_its_setting_says() {
        // Six files to read, six to extract subtitles from, six to make thumbnails of.
        let queue = Arc::new(Queue::with(&[]));
        let tasks = [
            MediaTask::Headers,
            MediaTask::Subtitles,
            MediaTask::PreviewThumbnails,
        ];
        for (part, task) in (1..=18).zip(tasks.iter().flat_map(|task| [*task; 6])) {
            queue.tasks.lock().unwrap().push(QueuedFile {
                part: PartId::new(part),
                section: FILMS,
                path: format!("/media/{part}.mkv"),
                size: 100,
                modified_at: 10,
                task,
                duration_ms: None,
                content_hash: ContentHash::parse(HASH),
            });
        }
        let gate = Arc::new(Notify::new());
        let reader = Arc::new(Reader {
            gate: Some(Arc::clone(&gate)),
            hash: Some(HASH),
            ..Reader::default()
        });
        let analysis = analysis_with(&queue, &reader, &running([2, 1, 4, 1]).await);
        let worker = analysis.spawn();
        settle().await;
        assert_eq!(reader.count("headers"), 2);
        assert_eq!(reader.count("subtitles"), 4);
        assert_eq!(reader.count("thumbnails"), 1);

        // Lowered while they run: those under way end, and no more start.
        settings::update_settings(
            analysis.shared.tools.settings.as_ref(),
            &[SettingUpdate {
                key: settings::SUBTITLE_EXTRACTION_CONCURRENCY.key.to_owned(),
                value: Some("1".to_owned()),
            }],
            UserId::new(1),
            1,
        )
        .await
        .unwrap()
        .unwrap();
        analysis.settings_changed();
        gate.notify_waiters();
        settle().await;
        assert_eq!(reader.count("subtitles"), 5, "one more, where four ended");

        for _ in 0..80 {
            gate.notify_waiters();
            settle().await;
        }
        assert!(queue.tasks.lock().unwrap().is_empty());
        worker.abort();
    }

    #[tokio::test]
    async fn a_library_being_walked_or_paused_is_not_read() {
        let queue = Arc::new(Queue::with(&[(1, FILMS), (2, SERIES)]));
        let reader = Arc::new(Reader::default());
        let analysis = analysis(&queue, &reader);
        analysis.walking(FILMS);
        analysis.pause(SERIES);
        let worker = analysis.spawn();
        settle().await;
        assert!(reader.reads.lock().unwrap().is_empty());

        analysis.walked(FILMS);
        settle().await;
        assert!(queue.left(1).is_empty());
        assert_eq!(
            queue.left(2),
            [MediaTask::Headers],
            "paused until its next scan"
        );

        analysis.walking(SERIES);
        analysis.walked(SERIES);
        settle().await;
        assert!(queue.left(2).is_empty());
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
        let analysis = analysis(&queue, &reader);
        let worker = analysis.spawn();
        settle().await;
        assert!(analysis.reads(FILMS));

        assert!(analysis.pause(FILMS));
        settle().await;
        assert!(!analysis.reads(FILMS));
        assert!(analysis.reads(SERIES), "another library goes on");
        assert_eq!(queue.left(1), [MediaTask::Headers]);
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
        let analysis = analysis(&queue, &reader);
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
        assert_eq!(reader.count("headers"), 2, "read again, as it now is");
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
        analysis(&queue, &reader)
            .analyze_library(FILMS)
            .await
            .unwrap();
        assert_eq!(*queue.headers.lock().unwrap(), [(PartId::new(1), false)]);
        assert!(queue.left(1).is_empty());
    }

    #[tokio::test]
    async fn missing_tools_leave_the_queue_as_it_is() {
        let queue = Arc::new(Queue::with(&[(1, FILMS), (2, FILMS)]));
        let reader = Arc::new(Reader {
            failing: Some(|| AnalyzeError::Unavailable("no ffprobe".to_owned())),
            ..Reader::default()
        });
        let analysis = one_at_a_time(&queue, &reader).await;
        let stopped = analysis.analyze_library(FILMS).await;
        assert!(
            matches!(&stopped, Err(ScanError::AnalyzerUnavailable(reason)) if reason == "no ffprobe"),
            "{stopped:?}"
        );
        assert_eq!(reader.count("headers"), 1, "the next file is not tried");
        assert_eq!(queue.tasks.lock().unwrap().len(), 2);
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
        let analysis = one_at_a_time(&queue, &reader).await;
        analysis.analyze_library(FILMS).await.unwrap();
        assert_eq!(reader.count("headers"), 1);
        assert_eq!(
            queue.tasks.lock().unwrap().len(),
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
        let analysis = analysis(&queue, &reader);
        // No worker runs: the read is the player's own.
        assert!(analysis.read_now(PartId::new(2)).await.unwrap());
        assert_eq!(queue.left(2)[0], MediaTask::Packets);
        assert_eq!(queue.left(1), [MediaTask::Headers]);

        assert!(
            analysis.read_now(PartId::new(2)).await.unwrap(),
            "read already"
        );
        assert!(
            analysis.read_now(PartId::new(9)).await.unwrap(),
            "not queued"
        );
        assert_eq!(reader.count("headers"), 1);
    }

    #[tokio::test]
    async fn a_player_is_told_when_a_file_cannot_be_read_yet() {
        let queue = Arc::new(Queue::with(&[(1, FILMS)]));
        let reader = Arc::new(Reader {
            failing: Some(|| AnalyzeError::Unreachable("no such file".to_owned())),
            ..Reader::default()
        });
        assert!(!analysis(&queue, &reader)
            .read_now(PartId::new(1))
            .await
            .unwrap());
        assert_eq!(queue.left(1), [MediaTask::Headers]);
    }

    #[test]
    fn a_content_hash_is_forty_hex_digits_and_nothing_else() {
        assert_eq!(
            ContentHash::parse(&HASH.to_ascii_uppercase())
                .unwrap()
                .as_str(),
            HASH
        );
        for refused in [
            "",
            "../../etc",
            &HASH[1..],
            &format!("{HASH}0"),
            &HASH.replace('a', "g"),
        ] {
            assert!(ContentHash::parse(refused).is_none(), "{refused}");
        }
    }
}
