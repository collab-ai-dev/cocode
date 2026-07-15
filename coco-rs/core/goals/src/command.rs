//! The command surface the pure reducer accepts.
//!
//! Commands are **in-process** values, not persisted wire types: they carry the
//! non-serializable [`CompletionAuthorization`] capability and are consumed by
//! [`crate::decide`]. Only a `FinishTurn`/`AcceptCompletion` carrying a gate-minted
//! authorization can drive a `completed` transition — there is no directly
//! constructible completed command (§10.2).

use coco_types::{SessionId, TurnId};

use crate::budget::{GoalBudget, GoalTurnTrigger, UsageDelta};
use crate::completion::{
    CompletionAuthorization, CompletionContract, CompletionPolicy, CompletionRejection,
};
use crate::disposition::{
    BlockerEvidence, ModeGate, ProgressSignal, WaitCondition, WaitResolution,
};
use crate::id::{GoalId, GoalLeaseId, SpecRevision, Timestamp, WakeId};
use crate::plan::GoalPlanRef;
use crate::snapshot::{GoalObjective, ProgressCheckpoint};
use crate::status::{BudgetKind, PauseReason, UsageLimitReason};

/// A discrete goal command. Every variant carries the identity it targets so the
/// reducer can reject stale work without ambient state.
#[derive(Debug, Clone)]
pub enum GoalCommand {
    Create(CreateGoal),
    StartTurn(StartTurn),
    FinishTurn(FinishTurn),
    Wake(Wake),
    Pause(Pause),
    Resume(Resume),
    Edit(Edit),
    Clear(Clear),
    AcceptCompletion(AcceptCompletion),
    RejectCompletion(RejectCompletion),
}

/// Create a goal. Commits a queued lease in the same transaction (§9.1 invariant 9),
/// or `waiting(mode_gate)` when Plan/Review mode is selected.
#[derive(Debug, Clone)]
pub struct CreateGoal {
    pub goal_id: GoalId,
    pub session_id: SessionId,
    pub lease_id: GoalLeaseId,
    pub objective: GoalObjective,
    pub contract: Option<CompletionContract>,
    pub policy: CompletionPolicy,
    pub budget: GoalBudget,
    pub plan: Option<GoalPlanRef>,
    /// `Some` => persist `waiting(mode_gate)`; no automatic turn starts.
    pub mode_gate: Option<ModeGate>,
    /// Wake identity used only when `mode_gate` is `Some`.
    pub wake_id: WakeId,
    pub at: Timestamp,
}

/// Bind a queued lease to a starting turn: `active(queued)` → `active(running)`.
#[derive(Debug, Clone)]
pub struct StartTurn {
    pub goal_id: GoalId,
    pub lease_id: GoalLeaseId,
    pub turn_id: TurnId,
    pub trigger: GoalTurnTrigger,
    pub at: Timestamp,
}

/// Finalize a running turn. The host coordinator has already decided the `outcome`
/// (progress/wait/blocked/completed/…); the reducer validates identity, folds usage
/// and counters, and enacts the transition.
#[derive(Debug, Clone)]
pub struct FinishTurn {
    pub goal_id: GoalId,
    pub lease_id: GoalLeaseId,
    pub turn_id: TurnId,
    /// Whether the worker called `report_goal_turn` (drives the unreported streak).
    pub reported: bool,
    /// Accepted runtime progress signals produced during the turn (§9.5).
    pub signals: Vec<ProgressSignal>,
    pub usage: UsageDelta,
    pub outcome: TurnFinishOutcome,
    pub at: Timestamp,
}

/// The transition a finalized turn enacts. Determined by the coordinator/gate.
#[derive(Debug, Clone)]
pub enum TurnFinishOutcome {
    /// Progress or unreported: queue the next lease. A `rejection` records why a
    /// completion candidate was sent back to `active` (§9.4); `None` on ordinary
    /// progress leaves any prior rejection in place.
    Continue {
        next_lease_id: GoalLeaseId,
        checkpoint: Option<ProgressCheckpoint>,
        rejection: Option<CompletionRejection>,
    },
    /// Register a durable wait (task/deadline/mode/permission/backoff/acceptance).
    Wait {
        wake_id: WakeId,
        condition: WaitCondition,
    },
    /// A typed, evidenced impasse or terminal execution error.
    Blocked { evidence: BlockerEvidence },
    /// A no-progress or system-generated pause.
    Paused { reason: PauseReason },
    /// Provider/account quota exhaustion.
    UsageLimited { reason: UsageLimitReason },
    /// Turn or token budget exhausted.
    BudgetLimited { kind: BudgetKind },
    /// Gate-authorized completion (sealed capability required).
    Completed {
        authorization: CompletionAuthorization,
    },
}

/// Fire a registered wake: `waiting` → `active(queued)`.
#[derive(Debug, Clone)]
pub struct Wake {
    pub goal_id: GoalId,
    pub wake_id: WakeId,
    pub next_lease_id: GoalLeaseId,
    pub resolution: Option<WaitResolution>,
    pub at: Timestamp,
}

/// User interrupt or system pause: cancel queued/running work and wait, persist
/// `paused`.
#[derive(Debug, Clone)]
pub struct Pause {
    pub goal_id: GoalId,
    pub reason: PauseReason,
    pub at: Timestamp,
}

/// Resume a stopped goal: commit `active` plus a queued lease in one transaction.
/// Rejected from `budget_limited` without a prior budget raise.
#[derive(Debug, Clone)]
pub struct Resume {
    pub goal_id: GoalId,
    pub next_lease_id: GoalLeaseId,
    pub at: Timestamp,
}

/// A user edit to objective/contract/policy/budget/plan-binding. Advances
/// `SpecRevision`; compares `expected_spec_revision` for optimistic concurrency.
/// May atomically resume a `budget_limited` goal when it raises the exhausted
/// budget (§11.3).
#[derive(Debug, Clone)]
pub struct Edit {
    pub goal_id: GoalId,
    pub expected_spec_revision: SpecRevision,
    pub objective: Option<GoalObjective>,
    pub contract: Option<CompletionContract>,
    pub clear_contract: bool,
    pub policy: Option<CompletionPolicy>,
    pub budget: Option<GoalBudget>,
    pub plan_binding: Option<GoalPlanRef>,
    /// Queued lease for an atomic budget-edit-and-resume from `budget_limited`.
    pub next_lease_id: Option<GoalLeaseId>,
    pub at: Timestamp,
}

/// Clear the current goal: remove the snapshot, append an audit event.
#[derive(Debug, Clone)]
pub struct Clear {
    pub goal_id: GoalId,
    pub at: Timestamp,
}

/// Accept a parked `waiting(user_acceptance)` candidate: persist `completed`.
#[derive(Debug, Clone)]
pub struct AcceptCompletion {
    pub authorization: CompletionAuthorization,
    pub at: Timestamp,
}

/// Reject a parked `waiting(user_acceptance)` candidate: return to `active(queued)`
/// with bounded reasons so the next turn addresses the gaps.
#[derive(Debug, Clone)]
pub struct RejectCompletion {
    pub goal_id: GoalId,
    pub next_lease_id: GoalLeaseId,
    pub rejection: CompletionRejection,
    pub at: Timestamp,
}
