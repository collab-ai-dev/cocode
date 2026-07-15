//! Durable persistence seam for the goal aggregate.
//!
//! The goal snapshot is stored in the session's append-only JSONL metadata
//! (§13.1) — the highest valid state version is the authority. This trait is the
//! narrow boundary; the concrete session-store-backed implementation lives in the
//! session runtime (agent-host), which holds the write lease and the session id.
//! Keeping it a trait lets the runtime handle be exercised without touching disk.

use coco_goals::{GoalId, GoalSnapshot};

use crate::error::Result;

/// Persist and recover the durable goal snapshot for one session.
///
/// All methods run under the session write lease. Persistence is append-only and
/// happens *before* the live projection advances, so a crash never leaves a
/// visible state without its durable record.
pub trait GoalStore: Send + Sync {
    /// Append a committed snapshot. The newest append with the highest
    /// `state_version` is authoritative on reload.
    fn persist(&self, snapshot: &GoalSnapshot) -> Result<()>;

    /// Record a clear-audit event and drop the durable current snapshot so a
    /// subsequent [`GoalStore::load`] returns `None`.
    fn clear(&self, goal_id: &GoalId) -> Result<()>;

    /// Load the latest durable snapshot, or `None` when no (uncleared) goal
    /// exists for the session.
    fn load(&self) -> Result<Option<GoalSnapshot>>;
}

/// In-memory [`GoalStore`] for tests and ephemeral sessions. Models the
/// append-only log plus a clear tombstone.
#[derive(Debug, Default)]
pub struct InMemoryGoalStore {
    inner: std::sync::Mutex<InMemoryGoalState>,
}

#[derive(Debug, Default)]
struct InMemoryGoalState {
    log: Vec<GoalSnapshot>,
    cleared: bool,
}

impl InMemoryGoalStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, InMemoryGoalState> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Number of persisted snapshots (append count), for test assertions.
    pub fn append_count(&self) -> usize {
        self.lock().log.len()
    }
}

impl GoalStore for InMemoryGoalStore {
    fn persist(&self, snapshot: &GoalSnapshot) -> Result<()> {
        let mut state = self.lock();
        state.cleared = false;
        state.log.push(snapshot.clone());
        Ok(())
    }

    fn clear(&self, _goal_id: &GoalId) -> Result<()> {
        self.lock().cleared = true;
        Ok(())
    }

    fn load(&self) -> Result<Option<GoalSnapshot>> {
        let state = self.lock();
        if state.cleared {
            return Ok(None);
        }
        Ok(state
            .log
            .iter()
            .max_by_key(|snapshot| snapshot.state_version)
            .cloned())
    }
}

#[cfg(test)]
#[path = "store.test.rs"]
mod tests;
