//! Concrete [`GoalStore`] over the session transcript JSONL (§13.1).
//!
//! Bridges the durable `MetadataEntry::GoalSnapshot` format to the goal runtime's
//! `GoalStore` seam. All writes go through the session store while the owning
//! `SessionRuntime` holds the write lease, so the append-only log has one writer
//! per session across processes. The typed `GoalSnapshot` (de)serializes here, at
//! the agent-host boundary, keeping `coco-session` free of a `coco-goals` dep.

use std::sync::Arc;

use coco_goal_runtime::{GoalRuntimeError, GoalStore, Result};
use coco_goals::{GoalId, GoalSnapshot};
use coco_session::{MetadataEntry, SessionStore, latest_goal_snapshot};
use coco_types::SessionId;

/// A session-transcript-backed goal store.
pub struct TranscriptGoalStore {
    store: Arc<dyn SessionStore>,
    session_id: SessionId,
}

impl TranscriptGoalStore {
    pub fn new(store: Arc<dyn SessionStore>, session_id: SessionId) -> Self {
        Self { store, session_id }
    }
}

impl GoalStore for TranscriptGoalStore {
    fn persist(&self, snapshot: &GoalSnapshot) -> Result<()> {
        let value = serde_json::to_value(snapshot)
            .map_err(|e| GoalRuntimeError::store(format!("serialize goal snapshot: {e}")))?;
        let entry = MetadataEntry::GoalSnapshot {
            session_id: self.session_id.clone(),
            goal_id: snapshot.goal_id.to_string(),
            state_version: snapshot.state_version.get(),
            snapshot: value,
        };
        self.store
            .append_metadata(self.session_id.as_str(), &entry)
            .map_err(|e| GoalRuntimeError::store(format!("append goal snapshot: {e}")))
    }

    fn clear(&self, goal_id: &GoalId) -> Result<()> {
        let entry = MetadataEntry::GoalCleared {
            session_id: self.session_id.clone(),
            goal_id: goal_id.to_string(),
        };
        self.store
            .append_metadata(self.session_id.as_str(), &entry)
            .map_err(|e| GoalRuntimeError::store(format!("append goal clear: {e}")))
    }

    fn load(&self) -> Result<Option<GoalSnapshot>> {
        let entries = match self.store.load_entries(self.session_id.as_str()) {
            Ok(entries) => entries,
            // A session with no transcript yet (fresh) simply has no goal.
            Err(coco_session::SessionError::TranscriptNotFound { .. }) => return Ok(None),
            Err(e) => return Err(GoalRuntimeError::store(format!("load goal entries: {e}"))),
        };
        match latest_goal_snapshot(&entries) {
            Some(record) => {
                let snapshot = serde_json::from_value(record.snapshot).map_err(|e| {
                    GoalRuntimeError::store(format!("deserialize goal snapshot: {e}"))
                })?;
                Ok(Some(snapshot))
            }
            None => Ok(None),
        }
    }
}

#[cfg(test)]
#[path = "goal_store.test.rs"]
mod tests;
