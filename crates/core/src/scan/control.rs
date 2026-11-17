//! Scans running in the background: at most one per library, each cancellable.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use crate::ids::SectionId;
use crate::scan::{scan_section, ScanError, ScanRequest, Scanner};

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
}

/// Runs each scan as a task of the current Tokio runtime.
#[derive(Debug)]
pub struct BackgroundScans {
    scanner: Scanner,
    running: Arc<Mutex<HashMap<SectionId, Arc<AtomicBool>>>>,
}

impl BackgroundScans {
    pub fn new(scanner: Scanner) -> Self {
        Self {
            scanner,
            running: Arc::default(),
        }
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
        let running = Arc::clone(&self.running);
        tokio::spawn(async move {
            // Dropped however the task ends, a panic included, so a library is never left
            // marked as being scanned.
            let _finished = Finished { running, section };
            match scan_section(&scanner, &request, &cancelled).await {
                Ok(outcome) => tracing::info!(
                    section = section.get(),
                    added = outcome.added,
                    updated = outcome.updated,
                    removed = outcome.removed,
                    unavailable_roots = outcome.unavailable_roots,
                    "library scanned"
                ),
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
