//! Scans running in the background: at most one per library, each cancellable at once, and the
//! analysis of what they find, which goes on after them.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::Notify;

use crate::files::LibraryFiles;
use crate::ids::{PartId, SectionId};
use crate::scan::{
    scan_section, AnalysisProgress, BackgroundAnalysis, ScanError, ScanRequest, Scanner,
};
use crate::source_metadata::{Matching, SourceMetadataError};

/// How far a library's files are read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LibraryAnalysis {
    pub progress: AnalysisProgress,
    /// Whether some of its files are being read now.
    pub reading: bool,
    /// Why its files are not being read: the tools cannot run, or a file could not be opened.
    pub failure: Option<String>,
}

/// Starts and stops scans, and the analysis that follows them; what the API drives.
#[async_trait::async_trait]
pub trait ScanControl: Send + Sync + std::fmt::Debug {
    /// Starts a scan and returns at once. The library's files are read once it is walked.
    ///
    /// # Errors
    ///
    /// [`ScanError::AlreadyRunning`] when the library is being scanned.
    async fn start(&self, request: ScanRequest) -> Result<(), ScanError>;

    /// Asks the library's running scan to stop, and stops reading its files until its next
    /// scan; whether either was under way.
    fn cancel(&self, section: SectionId) -> bool;

    /// Whether the library is being walked, or what was found in it matched.
    fn is_running(&self, section: SectionId) -> bool;

    /// Why the library's last scan stopped before its end, cancelled aside. `None` when it
    /// ended, or when none ran since the server started.
    fn last_failure(&self, section: SectionId) -> Option<String>;

    /// Stops the reads under way in a library being deleted or given other roots: its files
    /// go with it, or with the scan that follows.
    fn forget(&self, section: SectionId);

    /// Reads a file's headers at once when they have not been read yet: a player is about to
    /// play it. Whether nothing is left to wait for.
    ///
    /// # Errors
    ///
    /// Storage's failure.
    async fn read_now(&self, part: PartId) -> Result<bool, ScanError>;

    /// # Errors
    ///
    /// Storage's failure.
    async fn analysis(&self, section: SectionId) -> Result<LibraryAnalysis, ScanError>;

    /// A setting of the analysis changed, for the server or a library: what it turned on is
    /// taken at once rather than at the next scan.
    fn settings_changed(&self);

    /// What the scans read libraries through, and so where a root an owner names is looked
    /// up before a library is given it.
    fn files(&self) -> &dyn LibraryFiles;
}

/// Runs each scan as a task of the current Tokio runtime, and hands what it found to the
/// analysis.
#[derive(Debug)]
pub struct BackgroundScans {
    scanner: Scanner,
    analysis: BackgroundAnalysis,
    /// What a finished scan matches its new items with; `None` when no provider is set up.
    matching: Option<Matching>,
    /// Each running scan's cancel. Its one waiter is the scan's task, so a cancel sent before
    /// the task first waits is kept, not lost.
    running: Arc<Mutex<HashMap<SectionId, Arc<Notify>>>>,
    failures: Arc<Mutex<HashMap<SectionId, String>>>,
}

impl BackgroundScans {
    pub fn new(scanner: Scanner, analysis: BackgroundAnalysis) -> Self {
        Self {
            scanner,
            analysis,
            matching: None,
            running: Arc::default(),
            failures: Arc::default(),
        }
    }

    /// Matches what each scan finds once it ends, as the reference runs its agent after the
    /// scanner: in the same task, so that the library counts as being scanned until its new
    /// items have their metadata, and no second scan starts in between. Matching reads names,
    /// not files, so it does not wait for the analysis.
    #[must_use]
    pub fn with_matching(mut self, matching: Matching) -> Self {
        self.matching = Some(matching);
        self
    }
}

#[async_trait::async_trait]
impl ScanControl for BackgroundScans {
    async fn start(&self, request: ScanRequest) -> Result<(), ScanError> {
        let section = request.section;
        let cancel = Arc::new(Notify::new());
        {
            let mut running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
            if running.contains_key(&section) {
                return Err(ScanError::AlreadyRunning);
            }
            running.insert(section, Arc::clone(&cancel));
        }
        self.analysis.walking(section);
        let scanner = self.scanner.clone();
        let analysis = self.analysis.clone();
        let matching = self.matching.clone();
        let running = Arc::clone(&self.running);
        let failures = Arc::clone(&self.failures);
        tokio::spawn(async move {
            // Dropped however the task ends, a panic included, so a library is never left
            // marked as being scanned, nor its files held.
            let mut finished = Finished {
                running,
                analysis: Some(analysis),
                section,
            };
            tokio::select! {
                biased;
                // Whatever the scan waits on goes with it: a write in progress is rolled back,
                // a share that stopped answering left to answer no one, the matching dropped.
                () = cancel.notified() => {
                    failures.lock().unwrap_or_else(PoisonError::into_inner).remove(&section);
                    tracing::info!(section = section.get(), "library scan cancelled");
                }
                () = scan_and_match(
                    &scanner,
                    matching.as_ref(),
                    &request,
                    &failures,
                    &mut finished,
                ) => {}
            }
        });
        Ok(())
    }

    fn cancel(&self, section: SectionId) -> bool {
        let scanning = {
            let running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
            match running.get(&section) {
                Some(cancel) => {
                    cancel.notify_one();
                    true
                }
                None => false,
            }
        };
        let reading = self.analysis.pause(section);
        scanning || reading
    }

    fn is_running(&self, section: SectionId) -> bool {
        self.running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains_key(&section)
    }

    fn last_failure(&self, section: SectionId) -> Option<String> {
        self.failures
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&section)
            .cloned()
    }

    fn forget(&self, section: SectionId) {
        self.analysis.forget(section);
    }

    async fn read_now(&self, part: PartId) -> Result<bool, ScanError> {
        self.analysis.read_now(part).await
    }

    async fn analysis(&self, section: SectionId) -> Result<LibraryAnalysis, ScanError> {
        Ok(LibraryAnalysis {
            progress: self.analysis.progress(section).await?,
            reading: self.analysis.reads(section),
            failure: self.analysis.failure(section),
        })
    }

    fn settings_changed(&self) {
        self.analysis.settings_changed();
    }

    fn files(&self) -> &dyn LibraryFiles {
        self.scanner.files.as_ref()
    }
}

/// One scan to its end, then the matching of what it added, its files read meanwhile.
async fn scan_and_match(
    scanner: &Scanner,
    matching: Option<&Matching>,
    request: &ScanRequest,
    failures: &Mutex<HashMap<SectionId, String>>,
    finished: &mut Finished,
) {
    let section = request.section;
    let result = scan_section(scanner, request).await;
    finished.walked();
    {
        let mut failures = failures.lock().unwrap_or_else(PoisonError::into_inner);
        match &result {
            Ok(_) => failures.remove(&section),
            Err(error) => failures.insert(section, error.to_string()),
        };
    }
    let outcome = match result {
        Ok(outcome) => outcome,
        Err(error) => {
            tracing::error!(section = section.get(), %error, "library scan failed");
            return;
        }
    };
    tracing::info!(
        section = section.get(),
        added = outcome.added,
        updated = outcome.updated,
        removed = outcome.removed,
        unavailable_roots = outcome.unavailable_roots,
        queued = outcome.queued,
        "library scanned"
    );
    let Some(matching) = matching else {
        return;
    };
    match matching.library(section, false, crate::now_epoch()).await {
        Ok(report) => tracing::info!(
            section = section.get(),
            matched = report.matched.len(),
            unmatched = report.unmatched.len(),
            failed = report.failed.len(),
            "library matched"
        ),
        Err(SourceMetadataError::UnsupportedKind | SourceMetadataError::NoSource) => {}
        Err(error) => {
            tracing::warn!(section = section.get(), %error, "matching failed");
        }
    }
}

struct Finished {
    running: Arc<Mutex<HashMap<SectionId, Arc<Notify>>>>,
    /// Until the walk is over: what it found may then be read, while matching goes on.
    analysis: Option<BackgroundAnalysis>,
    section: SectionId,
}

impl Finished {
    fn walked(&mut self) {
        if let Some(analysis) = self.analysis.take() {
            analysis.walked(self.section);
        }
    }
}

impl Drop for Finished {
    fn drop(&mut self) {
        self.walked();
        self.running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.section);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::sync::atomic::Ordering;
    use std::time::Duration;

    use klapz_scanner::report::{Analysis, DeepAnalysis};

    use super::*;
    use crate::files::FileLocation;
    use crate::library::SectionKind;
    use crate::scan::walk::tests::{scanner, whole_library, Store, Tree};
    use crate::scan::{
        AnalysisTools, AnalyzeError, ContentHash, Extraction, MediaAnalyzer, MediaExtractor,
    };
    use crate::testing::InMemorySettings;

    const FILMS: SectionId = SectionId::new(1);

    /// The analysis is not run here: these tests are about the walk.
    #[derive(Debug)]
    struct Unused;

    #[async_trait::async_trait]
    impl MediaAnalyzer for Unused {
        async fn read_headers(&self, _file: &FileLocation) -> Result<Analysis, AnalyzeError> {
            Err(AnalyzeError::Unavailable(
                "not run in these tests".to_owned(),
            ))
        }

        async fn measure(
            &self,
            _file: &FileLocation,
            _duration_ms: Option<u64>,
        ) -> Result<DeepAnalysis, AnalyzeError> {
            Err(AnalyzeError::Unavailable(
                "not run in these tests".to_owned(),
            ))
        }
    }

    #[async_trait::async_trait]
    impl MediaExtractor for Unused {
        async fn extract_subtitles(
            &self,
            _file: &FileLocation,
            _hash: &ContentHash,
        ) -> Result<Extraction, AnalyzeError> {
            Err(AnalyzeError::Unavailable(
                "not run in these tests".to_owned(),
            ))
        }

        async fn generate_preview_thumbnails(
            &self,
            _file: &FileLocation,
            _hash: &ContentHash,
        ) -> Result<Extraction, AnalyzeError> {
            Err(AnalyzeError::Unavailable(
                "not run in these tests".to_owned(),
            ))
        }
    }

    async fn ended(scans: &BackgroundScans) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while scans.is_running(FILMS) {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("the scan ends");
    }

    fn films(tree: Arc<Tree>) -> BackgroundScans {
        let scanner = scanner(SectionKind::Movie, tree, Arc::new(Store::default()));
        let analysis = BackgroundAnalysis::new(
            Arc::clone(&scanner.store),
            Arc::clone(&scanner.library),
            Arc::clone(&scanner.files),
            AnalysisTools {
                analyzer: Arc::new(Unused),
                extractor: Arc::new(Unused),
                settings: Arc::new(InMemorySettings::default()),
            },
        );
        BackgroundScans::new(scanner, analysis)
    }

    fn stalled() -> Arc<Tree> {
        Arc::new(Tree {
            stalled: vec!["Soul (2020)".to_owned()],
            ..Tree::with(&[("Soul (2020)/Soul (2020).mkv", 100, 10)])
        })
    }

    #[tokio::test]
    async fn a_scan_failing_mid_way_releases_its_library_and_says_why() {
        let scans = films(Arc::new(Tree {
            broken: vec!["Up (2009)".to_owned()],
            ..Tree::with(&[
                ("Soul (2020)/Soul (2020).mkv", 100, 10),
                ("Up (2009)/Up (2009).mkv", 200, 20),
            ])
        }));
        scans.start(whole_library()).await.unwrap();
        ended(&scans).await;
        assert_eq!(
            scans.last_failure(FILMS).as_deref(),
            Some("cannot list /media/Up (2009): reading library files failed")
        );
        assert!(!scans.cancel(FILMS));
        scans.start(whole_library()).await.unwrap();
        ended(&scans).await;
    }

    #[tokio::test]
    async fn a_cancel_ends_a_scan_at_once_whatever_it_waits_on() {
        let tree = stalled();
        let scans = films(Arc::clone(&tree));
        scans.start(whole_library()).await.unwrap();
        tree.stall.started.notified().await;
        assert!(matches!(
            scans.start(whole_library()).await,
            Err(ScanError::AlreadyRunning)
        ));

        assert!(scans.cancel(FILMS));
        ended(&scans).await;
        assert!(
            tree.stall.dropped.load(Ordering::SeqCst),
            "the listing is dropped with the scan"
        );
        assert_eq!(scans.last_failure(FILMS), None, "a cancel is no failure");
        scans.start(whole_library()).await.unwrap();
    }

    #[tokio::test]
    async fn a_cancel_sent_before_the_scan_runs_is_not_lost() {
        let tree = stalled();
        let scans = films(Arc::clone(&tree));
        scans.start(whole_library()).await.unwrap();
        assert!(scans.cancel(FILMS));
        ended(&scans).await;
        assert!(!tree.stall.dropped.load(Ordering::SeqCst), "it never began");
    }
}
