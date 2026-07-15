//! Evidence provenance store (§10.2, §12.2).
//!
//! The runtime mints a [`GoalEvidenceRecord`] when it accepts a durable result;
//! the gate resolves cited ids back to records to prove ownership. The worker can
//! cite an id but can never create a record here — production impls only accept
//! records the runtime itself produced.

use coco_goals::{EvidenceId, GoalEvidenceRecord, GoalId};

use crate::error::Result;

/// Records and resolves runtime-owned evidence for one session's goal.
pub trait EvidenceStore: Send + Sync {
    /// Resolve cited ids to their durable records; unknown ids are omitted so the
    /// gate's ownership check fails closed on them.
    fn resolve(&self, ids: &[EvidenceId]) -> Result<Vec<GoalEvidenceRecord>>;

    /// Persist a runtime-minted evidence record produced during a turn.
    fn record(&self, record: GoalEvidenceRecord) -> Result<()>;

    /// The most-recently minted records owned by `goal_id`, newest first, capped
    /// at `limit`. The goal-context reminder surfaces these so the worker learns
    /// the ids it may cite in `report_goal_turn` (§10.2 #9); it can cite an id but
    /// never mint one.
    fn recent_for_goal(&self, goal_id: &GoalId, limit: usize) -> Result<Vec<GoalEvidenceRecord>>;
}

/// In-memory [`EvidenceStore`] for tests and ephemeral sessions.
#[derive(Debug, Default)]
pub struct InMemoryEvidenceStore {
    records: std::sync::Mutex<Vec<GoalEvidenceRecord>>,
}

impl InMemoryEvidenceStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<GoalEvidenceRecord>> {
        self.records
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl EvidenceStore for InMemoryEvidenceStore {
    fn resolve(&self, ids: &[EvidenceId]) -> Result<Vec<GoalEvidenceRecord>> {
        let records = self.lock();
        Ok(ids
            .iter()
            .filter_map(|id| {
                records
                    .iter()
                    .find(|record| &record.evidence_id == id)
                    .cloned()
            })
            .collect())
    }

    fn record(&self, record: GoalEvidenceRecord) -> Result<()> {
        let mut records = self.lock();
        // Provenance is captured once per result: a re-mint of the same id (e.g.
        // the streaming and non-streaming commit paths both observing a tool) is
        // the same provenance, so keep the first and stay idempotent.
        if records
            .iter()
            .any(|existing| existing.evidence_id == record.evidence_id)
        {
            return Ok(());
        }
        records.push(record);
        Ok(())
    }

    fn recent_for_goal(&self, goal_id: &GoalId, limit: usize) -> Result<Vec<GoalEvidenceRecord>> {
        let records = self.lock();
        Ok(records
            .iter()
            .rev()
            .filter(|record| record.owned_by(goal_id))
            .take(limit)
            .cloned()
            .collect())
    }
}

#[cfg(test)]
#[path = "evidence.test.rs"]
mod tests;
