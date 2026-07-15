//! `coco-goals` — the pure domain layer for the first-class session goal runtime.
//!
//! This crate owns the goal state machine and its value objects, with **no** Tokio,
//! model-client, filesystem, protocol, or UI dependency. The host ([`GoalRuntimeHandle`]
//! and friends, in the session runtime) executes effects, performs I/O, and commits
//! snapshots; this crate only decides.
//!
//! [`GoalRuntimeHandle`]: https://internal/coco-rs/app/agent-host
//!
//! # Core guarantees
//!
//! The type system makes the two liveness defects proven for the legacy Stop-Hook
//! implementation (§7) unrepresentable:
//!
//! * [`GoalLifecycle::Active`] always carries a [`GoalLease`] (queued or running), so
//!   "active but no owner" cannot be constructed.
//! * [`GoalLifecycle::Waiting`] always carries a [`GoalWake`] with a durable
//!   [`WakeId`], so "waiting with no wake identity" cannot be constructed.
//!
//! And the completion-authority defect (§6, §10.2) is closed by sealing: `completed`
//! is reachable only with a [`CompletionAuthorization`], which has no public
//! constructor and is minted solely by [`authorize_completion`].
//!
//! # Entry point
//!
//! [`decide`] applies one [`GoalCommand`] to the current [`GoalSnapshot`] and returns
//! a [`GoalDecision`] (next snapshot + typed effects). It is pure, exhaustively
//! matched, and never fails open.

mod budget;
mod command;
mod completion;
mod decision;
mod disposition;
mod error;
mod evidence;
mod id;
mod plan;
mod reducer;
mod snapshot;
mod status;
mod text;

#[cfg(test)]
mod test_support;

pub use budget::{
    DEFAULT_MAX_AUTONOMOUS_TURNS, DEFAULT_PROBE_INTERVAL, GoalBudget, GoalCounters,
    GoalTurnTrigger, GoalUsage, MAX_SCHEDULER_RETRIES, NO_PROGRESS_LIMIT, UsageDelta,
};
pub use command::{
    AcceptCompletion, Clear, CreateGoal, Edit, FinishTurn, GoalCommand, Pause, RejectCompletion,
    Resume, StartTurn, TurnFinishOutcome, Wake,
};
pub use completion::{
    CandidateSource, CheckExpectation, CheckKind, CompletionAuthorization, CompletionCandidate,
    CompletionContract, CompletionEvidenceSummary, CompletionOutcome, CompletionPolicy,
    CompletionRejectReason, CompletionRejection, ContractItem, DeterministicCheck, ProbeVerdict,
    ReferencedDoc, SemanticCriterion, VerificationOutcome, authorize_completion,
    policy_can_judge_contract, precheck_candidate,
};
pub use decision::{
    GoalAuditKind, GoalDecision, GoalEffect, GoalReminderKind, GoalTransitionEvent,
};
pub use disposition::{
    BlockerEvidence, GoalTurnDisposition, ModeGate, ProgressSignal, RequirementCoverage,
    RequirementResult, WaitCondition, WaitResolution,
};
pub use error::GoalTransitionError;
pub use evidence::{DurableResultRef, EvidenceRef, EvidenceSource, GoalEvidenceRecord};
pub use id::{
    ContentDigest, EffectId, EvidenceId, GoalId, GoalLeaseId, PlanArtifactId, PlanRevision,
    SpecRevision, StateVersion, Timestamp, VerificationAttemptId, WakeId,
};
pub use plan::{GoalPlanRef, PlanCheckpoint};
pub use reducer::decide;
pub use snapshot::{
    Amendment, ArtifactRef, GoalObjective, GoalSnapshot, ProgressCheckpoint, SCHEMA_VERSION,
};
pub use status::{
    BudgetKind, GoalLease, GoalLifecycle, GoalStatus, GoalWake, PauseReason, UsageLimitReason,
};
pub use text::{BoundedText, OBJECTIVE_BUDGET, SHORT_TEXT_BUDGET};
