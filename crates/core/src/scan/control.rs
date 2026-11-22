//! Scans running in the background: at most one per library, each cancellable.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

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
    running: Arc<Mutex<HashMap<SectionId, Arc<AtomicBool>>>>,
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
        let cancelled = Arc::new(AtomicBool::new(false));
        {
            let mut running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
            if running.contains_key(&section) {
                return Err(ScanError::AlreadyRunning);
            }
            running.insert(section, Arc::clone(&cancelled));
        }
        let scanner = self.scanner.clone();
        let matching = self.matching.clone();
        let running = Arc::clone(&self.running);
        let failures = Arc::clone(&self.failures);
        tokio::spawn(async move {
            // Dropped however the task ends, a panic included, so a library is never left
            // marked as being scanned.
            let _finished = Finished { running, section };
            let result = scan_section(&scanner, &request, &cancelled).await;
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
        let running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
        match running.get(&section) {
            Some(flag) => {
                flag.store(true, Ordering::Relaxed);
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

struct Finished {
    running: Arc<Mutex<HashMap<SectionId, Arc<AtomicBool>>>>,
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
