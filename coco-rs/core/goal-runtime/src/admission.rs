//! `AutonomousAdmission` — process-wide bounded concurrency for autonomous goal
//! continuations (§10.2).
//!
//! It provides bounded cross-session concurrency so no single goal monopolizes
//! provider/tool capacity. It does **not** own goal state or continuation policy;
//! it only admits an already-durable queued lease. Normal user-started turns are
//! *not* routed through it — they keep interactive AppServer priority.

use std::sync::Arc;

use coco_types::SessionId;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// A held admission slot; the turn runs until this is dropped.
pub struct AdmissionPermit {
    _permit: OwnedSemaphorePermit,
}

/// Bounds how many autonomous goal turns run concurrently across the process.
/// tokio's semaphore is FIFO-fair, giving waiting sessions round-robin-like
/// service under contention.
#[derive(Clone)]
pub struct AutonomousAdmission {
    semaphore: Arc<Semaphore>,
}

impl AutonomousAdmission {
    /// Create an admission service allowing `max_concurrent` simultaneous
    /// autonomous turns.
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(max_concurrent.max(1))),
        }
    }

    /// Wait for an admission slot. The semaphore is never closed, so this only
    /// resolves by acquiring.
    pub async fn acquire(&self, _session_id: &SessionId) -> AdmissionPermit {
        match Arc::clone(&self.semaphore).acquire_owned().await {
            Ok(permit) => AdmissionPermit { _permit: permit },
            Err(_) => panic!("autonomous admission semaphore was closed unexpectedly"),
        }
    }

    /// Currently available slots (for diagnostics/tests).
    pub fn available_permits(&self) -> usize {
        self.semaphore.available_permits()
    }
}

#[cfg(test)]
#[path = "admission.test.rs"]
mod tests;
