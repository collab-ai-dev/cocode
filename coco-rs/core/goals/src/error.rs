//! Typed transition errors. The reducer never uses string status matching, wildcard
//! fallbacks, async locks, or I/O (§11).

use thiserror::Error;

use crate::id::{SpecRevision, StateVersion};
use crate::status::GoalStatus;

/// Why a [`crate::GoalCommand`] was rejected by the reducer.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum GoalTransitionError {
    #[error("no current goal to apply this command to")]
    NoCurrentGoal,

    #[error("a goal is already active; clear or confirm replacement first")]
    GoalAlreadyActive,

    #[error("stale goal id: command targets `{actual}`, current goal is `{expected}`")]
    StaleGoalId { expected: String, actual: String },

    #[error("spec revision mismatch: expected {expected}, found {actual}")]
    SpecRevisionMismatch {
        expected: SpecRevision,
        actual: SpecRevision,
    },

    #[error("state version mismatch: expected {expected}, found {actual}")]
    StateVersionMismatch {
        expected: StateVersion,
        actual: StateVersion,
    },

    #[error("goal lease mismatch: the command does not hold the current lease")]
    LeaseMismatch,

    #[error("running turn mismatch: the command does not match the running turn")]
    TurnMismatch,

    #[error("invalid transition from status `{from:?}` for this command")]
    InvalidTransition { from: GoalStatus },

    #[error("no registered wake matches this wake id")]
    WakeNotFound,

    #[error("resume from budget_limited requires raising the exhausted budget first")]
    BudgetRaiseRequired,

    #[error("budget edit does not raise the exhausted budget above committed usage")]
    InvalidBudgetEdit,

    #[error("completion policy cannot judge the supplied contract")]
    InvalidPolicyForContract,

    #[error("completion authorization does not match the current goal/spec/lease")]
    CompletionAuthorizationMismatch,
}
