//! The goal lifecycle state machine's value objects.
//!
//! The key type-level guarantees (§11):
//!
//! * [`GoalLifecycle::Active`] *always* carries a [`GoalLease`] (queued or running),
//!   so "active but no owner" is unrepresentable.
//! * [`GoalLifecycle::Waiting`] *always* carries a [`GoalWake`] with a durable
//!   [`WakeId`], so "waiting with no wake identity" is unrepresentable. (Liveness of
//!   the volatile watcher is a supervisor concern, not a DTO claim.)

use coco_types::TurnId;
use serde::{Deserialize, Serialize};

use crate::budget::GoalUsage;
use crate::completion::CompletionEvidenceSummary;
use crate::disposition::{BlockerEvidence, WaitCondition};
use crate::id::{GoalLeaseId, Timestamp, WakeId};
use crate::text::BoundedText;

/// A durable record of one goal-owned work attempt. Distinct from the cross-process
/// session write lease: this identifies work *inside* the owning runtime (§10.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GoalLease {
    /// A turn is queued but not yet running. `attempt` distinguishes retries.
    Queued { lease_id: GoalLeaseId, attempt: u32 },
    /// A turn is running under this lease.
    Running {
        lease_id: GoalLeaseId,
        turn_id: TurnId,
    },
}

impl GoalLease {
    pub fn lease_id(&self) -> &GoalLeaseId {
        match self {
            Self::Queued { lease_id, .. } | Self::Running { lease_id, .. } => lease_id,
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self, Self::Running { .. })
    }

    pub fn is_queued(&self) -> bool {
        matches!(self, Self::Queued { .. })
    }

    pub fn running_turn(&self) -> Option<&TurnId> {
        match self {
            Self::Running { turn_id, .. } => Some(turn_id),
            Self::Queued { .. } => None,
        }
    }
}

/// A durable wake obligation. Proves the obligation to register a wake, not that a
/// volatile watcher is currently alive (§11.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalWake {
    pub wake_id: WakeId,
    pub condition: WaitCondition,
}

/// Why an active/waiting goal stopped without an impasse or limit (§9.1, §11.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PauseReason {
    /// Explicit user interrupt/pause.
    UserInterrupt,
    /// Three consecutive signal-free goal turns (§9.5).
    NoProgress,
    /// Goal or plan context could not be materialized.
    ContextUnavailable,
    /// Required completion verification could not run.
    VerificationUnavailable,
    /// The autonomous scheduler exhausted its bounded retries.
    SchedulerUnavailable,
    /// A pending permission approval could not be recovered on resume.
    ApprovalRecoveryFailed,
    /// The latest snapshot could not be safely recovered.
    RecoveryError,
}

/// Provider/account quota exhaustion, with an optional reset deadline that arms an
/// automatic wake (§11.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageLimitReason {
    pub detail: BoundedText,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_deadline: Option<Timestamp>,
}

/// Which budget was exhausted (§11.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetKind {
    /// Autonomous-continuation cap.
    Turns,
    /// Optional token ceiling.
    Tokens,
}

/// The closed goal lifecycle. `Active`/`Waiting` carry their owner/wake by
/// construction; stopped and terminal statuses carry typed reasons.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum GoalLifecycle {
    Active { lease: GoalLease },
    Waiting { wake: GoalWake },
    Paused { reason: PauseReason },
    Blocked { evidence: BlockerEvidence },
    UsageLimited { reason: UsageLimitReason },
    BudgetLimited { kind: BudgetKind, usage: GoalUsage },
    Completed { evidence: CompletionEvidenceSummary },
}

impl GoalLifecycle {
    pub fn status(&self) -> GoalStatus {
        match self {
            Self::Active { .. } => GoalStatus::Active,
            Self::Waiting { .. } => GoalStatus::Waiting,
            Self::Paused { .. } => GoalStatus::Paused,
            Self::Blocked { .. } => GoalStatus::Blocked,
            Self::UsageLimited { .. } => GoalStatus::UsageLimited,
            Self::BudgetLimited { .. } => GoalStatus::BudgetLimited,
            Self::Completed { .. } => GoalStatus::Completed,
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(self, Self::Active { .. })
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed { .. })
    }

    /// Whether the runtime is (or will be) running a turn for this goal.
    pub fn has_automatic_work(&self) -> bool {
        self.is_active()
    }

    /// The current lease, if the goal is active.
    pub fn lease(&self) -> Option<&GoalLease> {
        match self {
            Self::Active { lease } => Some(lease),
            _ => None,
        }
    }

    /// The running lease id, if a turn is running.
    pub fn running_lease_id(&self) -> Option<&GoalLeaseId> {
        match self {
            Self::Active { lease } if lease.is_running() => Some(lease.lease_id()),
            _ => None,
        }
    }

    /// The registered wake, if the goal is waiting.
    pub fn wake(&self) -> Option<&GoalWake> {
        match self {
            Self::Waiting { wake } => Some(wake),
            _ => None,
        }
    }
}

/// Copy discriminant for protocol, UI, and coarse matching. Mirrors the recommended
/// closed status set (§11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    Active,
    Waiting,
    Paused,
    Blocked,
    UsageLimited,
    BudgetLimited,
    Completed,
}

impl GoalStatus {
    /// A stopped status that needs an explicit resume/edit action (not terminal, not
    /// automatically continuing).
    pub fn is_stopped(self) -> bool {
        matches!(
            self,
            Self::Paused | Self::Blocked | Self::UsageLimited | Self::BudgetLimited
        )
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed)
    }
}

#[cfg(test)]
#[path = "status.test.rs"]
mod tests;
