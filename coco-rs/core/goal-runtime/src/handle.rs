//! `GoalRuntimeHandle` — the session-local goal transaction boundary (§10.2).
//!
//! Serializes goal mutations, validates each command through the pure reducer,
//! commits the durable snapshot *before* advancing the live projection, and hands
//! the caller the typed effects and transition event to execute/emit. It is the
//! single writer of the live goal projection; tools, TUI, and context
//! materialization only read snapshots.

use std::sync::Arc;

use coco_goals::{GoalCommand, GoalEffect, GoalSnapshot, GoalStatus, GoalTransitionEvent, decide};
use coco_types::SessionId;
use tokio::sync::Mutex;

use crate::error::Result;
use crate::store::GoalStore;

/// The committed result of one applied command, for the caller to act on after
/// the durable commit: execute effects, emit protocol events, render a cell.
#[derive(Debug, Clone)]
pub struct AppliedGoalDecision {
    /// The new durable snapshot, or `None` after a clear.
    pub snapshot: Option<GoalSnapshot>,
    /// Typed effects (schedule turn, register/cancel wake, reminders, audit).
    pub effects: Vec<GoalEffect>,
    /// What transition occurred, for one concise transcript cell.
    pub event: GoalTransitionEvent,
}

/// Session-local transaction boundary owning the goal aggregate. Holds the write
/// lease indirectly through the injected [`GoalStore`], which the session runtime
/// wires to lease-guarded persistence.
pub struct GoalRuntimeHandle {
    session_id: SessionId,
    store: Arc<dyn GoalStore>,
    state: Mutex<GoalRuntimeState>,
    /// Sync-readable "a live (non-terminal) goal exists" flag, maintained on
    /// every commit so cheap sync callers (`Tool::is_enabled`) avoid awaiting the
    /// async mutex.
    has_live: std::sync::atomic::AtomicBool,
}

#[derive(Default)]
struct GoalRuntimeState {
    snapshot: Option<GoalSnapshot>,
    /// The worker's `report_goal_turn` disposition for the in-flight turn, drained
    /// by the completion coordinator at turn finalization.
    pending_report: Option<coco_goals::GoalTurnDisposition>,
}

fn is_live(snapshot: Option<&GoalSnapshot>) -> bool {
    snapshot.is_some_and(|snapshot| !snapshot.is_terminal())
}

impl GoalRuntimeHandle {
    /// Build a handle over an already-known projection (fresh session, or after
    /// the caller loaded the snapshot itself).
    pub fn new(
        session_id: SessionId,
        store: Arc<dyn GoalStore>,
        initial: Option<GoalSnapshot>,
    ) -> Self {
        let has_live = std::sync::atomic::AtomicBool::new(is_live(initial.as_ref()));
        Self {
            session_id,
            store,
            state: Mutex::new(GoalRuntimeState {
                snapshot: initial,
                pending_report: None,
            }),
            has_live,
        }
    }

    /// Build a handle by loading the latest durable snapshot from the store
    /// (resume). Liveness (scheduling/wakes) is restored by the supervisor, not
    /// here — this only recovers the projection.
    pub fn restore(session_id: SessionId, store: Arc<dyn GoalStore>) -> Result<Self> {
        let snapshot = store.load()?;
        Ok(Self::new(session_id, store, snapshot))
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    /// Apply one command: `decide` → durable commit → advance projection →
    /// return effects + event. Serialized under the goal mutex.
    pub async fn apply(&self, command: GoalCommand) -> Result<AppliedGoalDecision> {
        let mut state = self.state.lock().await;
        let decision = decide(state.snapshot.as_ref(), command)?;

        // Durable-before-visible (§10.1): persist first, advance projection only
        // on success. A store failure leaves the live projection unchanged.
        match &decision.snapshot {
            Some(snapshot) => self.store.persist(snapshot)?,
            None => {
                if let Some(previous) = &state.snapshot {
                    self.store.clear(&previous.goal_id)?;
                }
            }
        }
        state.snapshot = decision.snapshot.clone();
        self.has_live.store(
            is_live(state.snapshot.as_ref()),
            std::sync::atomic::Ordering::Relaxed,
        );

        Ok(AppliedGoalDecision {
            snapshot: decision.snapshot,
            effects: decision.effects,
            event: decision.event,
        })
    }

    /// A clone of the current durable snapshot.
    pub async fn snapshot(&self) -> Option<GoalSnapshot> {
        self.state.lock().await.snapshot.clone()
    }

    /// Sync check: whether a live (non-terminal) goal exists. For `is_enabled`
    /// tool gating that cannot await the async projection lock.
    pub fn has_live_goal_sync(&self) -> bool {
        self.has_live.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Record the worker's `report_goal_turn` disposition for the in-flight turn.
    /// Last write wins; the coordinator drains it at finalization.
    pub async fn set_pending_report(&self, disposition: coco_goals::GoalTurnDisposition) {
        self.state.lock().await.pending_report = Some(disposition);
    }

    /// Take and clear the pending turn disposition (called at turn finalization).
    pub async fn take_pending_report(&self) -> Option<coco_goals::GoalTurnDisposition> {
        self.state.lock().await.pending_report.take()
    }

    /// The current status, if a goal exists.
    pub async fn status(&self) -> Option<GoalStatus> {
        self.state
            .lock()
            .await
            .snapshot
            .as_ref()
            .map(GoalSnapshot::status)
    }

    /// Whether a non-terminal goal currently exists (active/waiting/stopped).
    pub async fn has_live_goal(&self) -> bool {
        self.state
            .lock()
            .await
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| !snapshot.is_terminal())
    }
}

impl std::fmt::Debug for GoalRuntimeHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoalRuntimeHandle")
            .field("session_id", &self.session_id)
            .finish_non_exhaustive()
    }
}

// Convenience: expose the conflict classification without importing the error
// module at every call site.
impl AppliedGoalDecision {
    /// Whether this decision scheduled a turn for the given lease.
    pub fn schedules_turn(&self) -> bool {
        self.effects
            .iter()
            .any(|effect| matches!(effect, GoalEffect::ScheduleTurn { .. }))
    }
}

#[cfg(test)]
#[path = "handle.test.rs"]
mod tests;
