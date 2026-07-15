//! The reducer's output: the next snapshot plus typed effects the host executes.
//!
//! The domain layer never performs I/O. It returns [`GoalEffect`]s describing what
//! the host must do (schedule a turn, register a wake, emit a reminder), and the
//! host commits the snapshot *before* publishing live state and events (§10.1).

use crate::id::{GoalLeaseId, WakeId};
use crate::snapshot::GoalSnapshot;
use crate::status::GoalWake;

/// The result of applying one [`crate::GoalCommand`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalDecision {
    /// The next durable snapshot, or `None` after `Clear`.
    pub snapshot: Option<GoalSnapshot>,
    /// Side effects for the host to execute after the durable commit.
    pub effects: Vec<GoalEffect>,
    /// What transition occurred, for one concise transcript cell (§9.4).
    pub event: GoalTransitionEvent,
}

/// A side effect the host executes after committing the snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalEffect {
    /// A queued lease is ready; the supervisor should start its turn.
    ScheduleTurn { lease_id: GoalLeaseId },
    /// Register a durable wake watcher for the given obligation.
    RegisterWake { wake: GoalWake },
    /// Cancel a previously registered wake.
    CancelWake { wake_id: WakeId },
    /// Release a queued/running lease's resources.
    ReleaseLease { lease_id: GoalLeaseId },
    /// Inject a one-shot goal reminder on the next goal-owned turn.
    EmitReminder(GoalReminderKind),
    /// Append an audit-only event (no live state change).
    RecordAudit(GoalAuditKind),
}

/// One-shot goal reminders (§5.5). The reducer emits the transition-driven ones;
/// the rest are host-driven (probe, report-missing, plan-drift).
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::Display)]
#[strum(serialize_all = "snake_case")]
pub enum GoalReminderKind {
    /// The user edited the objective.
    ObjectiveChanged,
    /// The plan artifact was (re)bound, e.g. after exiting Plan mode.
    PlanActivated,
    /// The plan file digest changed outside the current worker turn.
    PlanChanged,
    /// A wait resolved; carries the task/deadline completion.
    WaitResolved,
    /// The last turn omitted `report_goal_turn`.
    ReportMissing,
    /// A `LikelyComplete` probe verdict nudges the worker to report.
    CompletionProbe,
}

/// Audit-only lifecycle events that do not change live state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::Display)]
#[strum(serialize_all = "snake_case")]
pub enum GoalAuditKind {
    Cleared,
}

/// What transition a decision enacted, for transcript/protocol rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::Display)]
#[strum(serialize_all = "snake_case")]
pub enum GoalTransitionEvent {
    Created,
    TurnStarted,
    Continued,
    EnteredWaiting,
    Paused,
    Blocked,
    UsageLimited,
    BudgetLimited,
    Completed,
    Woken,
    Resumed,
    Edited,
    Cleared,
    CompletionRejected,
}
