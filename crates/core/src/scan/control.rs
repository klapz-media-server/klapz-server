//! Scans running in the background: at most one per library, each cancellable, and the
//! analysis of what they find, which goes on after them.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

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
    running: Arc<Mutex<HashMap<SectionId, Arc<AtomicBool>>>>,
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
        let cancelled = Arc::new(AtomicBool::new(false));
        {
            let mut running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
            if running.contains_key(&section) {
                return Err(ScanError::AlreadyRunning);
            }
            running.insert(section, Arc::clone(&cancelled));
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
            let result = scan_section(&scanner, &request, &cancelled).await;
            finished.walked();
            {
                let mut failures = failures.lock().unwrap_or_else(PoisonError::into_inner);
                match &result {
                    Ok(_) => {
                        failures.remove(&section);
                    }
                    Err(ScanError::Cancelled) => {}
                    Err(error) => {
                        failures.insert(section, error.to_string());
                    }
                }
            }
            match result {
                Ok(outcome) => {
                    tracing::info!(
                        section = section.get(),
                        added = outcome.added,
                        updated = outcome.updated,
                        removed = outcome.removed,
                        unavailable_roots = outcome.unavailable_roots,
                        queued = outcome.queued,
                        "library scanned"
                    );
                    if let Some(matching) = matching {
                        match matching.library(section, false, crate::now_epoch()).await {
                            Ok(report) => tracing::info!(
                                section = section.get(),
                                matched = report.matched.len(),
                                unmatched = report.unmatched.len(),
                                failed = report.failed.len(),
                                "library matched"
                            ),
                            Err(
                                SourceMetadataError::UnsupportedKind
                                | SourceMetadataError::NoSource,
                            ) => {}
                            Err(error) => {
                                tracing::warn!(section = section.get(), %error, "matching failed");
                            }
                        }
                    }
                }
                Err(ScanError::Cancelled) => {
                    tracing::info!(section = section.get(), "library scan cancelled");
                }
                Err(error) => {
                    tracing::error!(section = section.get(), %error, "library scan failed");
                }
            }
        });
        Ok(())
    }

    fn cancel(&self, section: SectionId) -> bool {
        let scanning = {
            let running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
            match running.get(&section) {
                Some(flag) => {
                    flag.store(true, Ordering::Relaxed);
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

    fn files(&self) -> &dyn LibraryFiles {
        self.scanner.files.as_ref()
    }
}

struct Finished {
    running: Arc<Mutex<HashMap<SectionId, Arc<AtomicBool>>>>,
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
