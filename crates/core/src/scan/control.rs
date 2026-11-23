//! Scans running in the background: at most one per library, each cancellable at once.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::Notify;

use crate::files::LibraryFiles;
use crate::ids::SectionId;
use crate::scan::{scan_section, ScanError, ScanRequest, Scanner};
use crate::source_metadata::{Matching, SourceMetadataError};

/// Starts and stops scans; what the API drives.
#[async_trait::async_trait]
pub trait ScanControl: Send + Sync + std::fmt::Debug {
    /// Starts a scan and returns at once.
    ///
    /// # Errors
    ///
    /// [`ScanError::AlreadyRunning`] when the library is being scanned.
    async fn start(&self, request: ScanRequest) -> Result<(), ScanError>;

    /// Asks the library's running scan to stop; whether one was running.
    fn cancel(&self, section: SectionId) -> bool;

    fn is_running(&self, section: SectionId) -> bool;

    /// Why the library's last scan stopped before its end, cancelled aside. `None` when it
    /// ended, or when none ran since the server started.
    fn last_failure(&self, section: SectionId) -> Option<String>;

    /// What the scans read libraries through, and so where a root an owner names is looked
    /// up before a library is given it.
    fn files(&self) -> &dyn LibraryFiles;
}

/// Runs each scan as a task of the current Tokio runtime.
#[derive(Debug)]
pub struct BackgroundScans {
    scanner: Scanner,
    /// What a finished scan matches its new items with; `None` when no provider is set up.
    matching: Option<Matching>,
    /// Each running scan's cancel. Its one waiter is the scan's task, so a cancel sent before
    /// the task first waits is kept, not lost.
    running: Arc<Mutex<HashMap<SectionId, Arc<Notify>>>>,
    failures: Arc<Mutex<HashMap<SectionId, String>>>,
}

impl BackgroundScans {
    pub fn new(scanner: Scanner) -> Self {
        Self {
            scanner,
            matching: None,
            running: Arc::default(),
            failures: Arc::default(),
        }
    }

    /// Matches what each scan finds once it ends, as the reference runs its agent after the
    /// scanner: in the same task, so that the library counts as being scanned until its new
    /// items have their metadata, and no second scan starts in between.
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
        let scanner = self.scanner.clone();
        let matching = self.matching.clone();
        let running = Arc::clone(&self.running);
        let failures = Arc::clone(&self.failures);
        tokio::spawn(async move {
            // Dropped however the task ends, a panic included, so a library is never left
            // marked as being scanned.
            let _finished = Finished { running, section };
            tokio::select! {
                biased;
                // Whatever the scan waits on goes with it: a probe reading a large file is
                // killed, a write in progress rolled back, a share that stopped answering left
                // to answer no one.
                () = cancel.notified() => {
                    failures.lock().unwrap_or_else(PoisonError::into_inner).remove(&section);
                    tracing::info!(section = section.get(), "library scan cancelled");
                }
                () = scan_and_match(&scanner, matching.as_ref(), &request, &failures) => {}
            }
        });
        Ok(())
    }

    fn cancel(&self, section: SectionId) -> bool {
        let running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
        match running.get(&section) {
            Some(cancel) => {
                cancel.notify_one();
                true
            }
            None => false,
        }
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

    fn files(&self) -> &dyn LibraryFiles {
        self.scanner.files.as_ref()
    }
}

/// One scan to its end, then the matching of what it added.
async fn scan_and_match(
    scanner: &Scanner,
    matching: Option<&Matching>,
    request: &ScanRequest,
    failures: &Mutex<HashMap<SectionId, String>>,
) {
    let section = request.section;
    let result = scan_section(scanner, request).await;
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
    section: SectionId,
}

impl Drop for Finished {
    fn drop(&mut self) {
        self.running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.section);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use klapz_scanner::report::Analysis;

    use super::*;
    use crate::files::FileLocation;
    use crate::library::SectionKind;
    use crate::scan::walk::tests::{scanner, whole_library, Analyzer, Store, Tree};
    use crate::scan::{AnalyzeError, MediaAnalyzer};

    const FILMS: SectionId = SectionId::new(1);

    /// An analysis that never ends, as ffprobe reading a share that stopped answering.
    #[derive(Debug, Default)]
    struct Endless {
        started: Notify,
        dropped: AtomicBool,
    }

    struct SetOnDrop<'a>(&'a AtomicBool);

    impl Drop for SetOnDrop<'_> {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[async_trait::async_trait]
    impl MediaAnalyzer for Endless {
        async fn analyze(&self, _file: &FileLocation) -> Result<Analysis, AnalyzeError> {
            let _dropped = SetOnDrop(&self.dropped);
            self.started.notify_one();
            std::future::pending().await
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

    fn films(tree: Tree) -> Scanner {
        scanner(
            SectionKind::Movie,
            Arc::new(tree),
            Arc::new(Store::default()),
            Arc::new(Analyzer::default()),
        )
    }

    #[tokio::test]
    async fn a_scan_failing_mid_way_releases_its_library_and_says_why() {
        let scans = BackgroundScans::new(films(Tree {
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
        let endless = Arc::new(Endless::default());
        let mut stuck = films(Tree::with(&[("Soul (2020)/Soul (2020).mkv", 100, 10)]));
        stuck.analyzer = Arc::clone(&endless) as Arc<dyn MediaAnalyzer>;
        let scans = BackgroundScans::new(stuck);
        scans.start(whole_library()).await.unwrap();
        endless.started.notified().await;
        assert!(matches!(
            scans.start(whole_library()).await,
            Err(ScanError::AlreadyRunning)
        ));

        assert!(scans.cancel(FILMS));
        ended(&scans).await;
        assert!(
            endless.dropped.load(Ordering::SeqCst),
            "the analysis is dropped, and its process with it"
        );
        assert_eq!(scans.last_failure(FILMS), None, "a cancel is no failure");
        scans.start(whole_library()).await.unwrap();
    }

    #[tokio::test]
    async fn a_cancel_sent_before_the_scan_runs_is_not_lost() {
        let endless = Arc::new(Endless::default());
        let mut stuck = films(Tree::with(&[("Soul (2020)/Soul (2020).mkv", 100, 10)]));
        stuck.analyzer = Arc::clone(&endless) as Arc<dyn MediaAnalyzer>;
        let scans = BackgroundScans::new(stuck);
        scans.start(whole_library()).await.unwrap();
        assert!(scans.cancel(FILMS));
        ended(&scans).await;
        assert!(!endless.dropped.load(Ordering::SeqCst), "it never began");
    }
}
