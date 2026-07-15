//! Per-turn worker disposition and the runtime signals a turn produces.
//!
//! The worker *reports* a [`GoalTurnDisposition`]; the runtime independently
//! *derives* [`ProgressSignal`]s from accepted observations. Prose is never a
//! signal (§9.5), so the two are kept strictly separate: a report cannot
//! manufacture progress, and real tool activity counts even without a report.

use coco_types::TaskId;
use serde::{Deserialize, Serialize};

use crate::evidence::EvidenceRef;
use crate::id::Timestamp;
use crate::text::BoundedText;

/// Closed set of accepted progress observations. Produced by the runtime, never by
/// the model. Assistant prose alone is not a signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressSignal {
    /// An accepted tool call completed.
    ToolObservation,
    /// The workspace changed (file write, patch applied).
    WorkspaceChange,
    /// The plan artifact digest changed.
    PlanChange,
    /// A runtime evidence record was minted.
    EvidenceRecorded,
    /// Background work was delegated to a task.
    TaskDelegated,
    /// A durable wait was registered.
    WaitRegistered,
}

/// The mode that gates autonomous goal execution while selected (§9.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModeGate {
    Plan,
    Review,
}

/// A temporary asynchronous condition the goal waits on (§11.2). Every variant maps
/// to a durable wake obligation; a textual "wait" with no wake is not representable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WaitCondition {
    /// Live background tasks must reach a terminal state.
    Task { task_ids: Vec<TaskId> },
    /// A wall-clock deadline.
    Deadline { deadline: Timestamp },
    /// A pending tool permission approval.
    Permission { request: BoundedText },
    /// Plan/Review mode is selected and gates execution.
    ModeGate { mode: ModeGate },
    /// A transient provider failure backoff.
    ProviderBackoff { attempt: u32, deadline: Timestamp },
    /// A gate-validated candidate awaiting explicit user acceptance.
    UserAcceptance,
    /// A provider/account usage window reset.
    UsageReset { deadline: Timestamp },
    /// An external condition described in bounded text.
    External { description: BoundedText },
}

/// Typed record delivered to the next turn when a wait resolves (§11.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitResolution {
    pub resolved: WaitCondition,
    pub detail: BoundedText,
}

/// A typed, evidenced impasse or a terminal execution error (§9.3). Never conflated
/// with completion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BlockerEvidence {
    /// An external dependency the worker cannot satisfy on its own.
    Dependency {
        dependency: BoundedText,
        attempted: Vec<BoundedText>,
        evidence: Vec<EvidenceRef>,
        required_change: BoundedText,
    },
    /// A non-retryable execution error that stopped the turn.
    ExecutionError { message: BoundedText },
}

/// One requirement's result within a completion candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequirementResult {
    pub requirement: BoundedText,
    pub satisfied: bool,
    pub evidence: Vec<EvidenceRef>,
}

/// Requirement-by-requirement completion claim plus the worker's assertion that no
/// required work remains (§12.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequirementCoverage {
    pub requirements: Vec<RequirementResult>,
    pub asserts_complete: bool,
}

impl RequirementCoverage {
    /// Whether every listed requirement is marked satisfied and completion is
    /// asserted. A necessary (not sufficient) structural precondition for the gate.
    pub fn all_satisfied(&self) -> bool {
        self.asserts_complete && self.requirements.iter().all(|r| r.satisfied)
    }
}

/// The worker's disposition for a goal-owned turn (§12.2). `Unreported` is
/// runtime-synthesized and never accepted from the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "disposition", rename_all = "snake_case")]
pub enum GoalTurnDisposition {
    Progress {
        summary: BoundedText,
        next_step: BoundedText,
        #[serde(default)]
        evidence: Vec<EvidenceRef>,
    },
    Waiting {
        condition: WaitCondition,
    },
    CompletionCandidate {
        coverage: RequirementCoverage,
        #[serde(default)]
        evidence: Vec<EvidenceRef>,
    },
    BlockedCandidate {
        evidence: BlockerEvidence,
    },
    Unreported,
}

impl GoalTurnDisposition {
    /// Whether the worker actually submitted a report (`false` only for the
    /// synthesized `Unreported`). Drives the unreported streak (§12.2).
    pub fn is_reported(&self) -> bool {
        !matches!(self, Self::Unreported)
    }

    /// Whether this disposition proposes completion.
    pub fn is_completion_candidate(&self) -> bool {
        matches!(self, Self::CompletionCandidate { .. })
    }
}

#[cfg(test)]
#[path = "disposition.test.rs"]
mod tests;
