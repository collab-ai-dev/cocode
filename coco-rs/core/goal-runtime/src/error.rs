//! Tier-3 main-trunk error for the goal runtime.

use coco_error::{ErrorExt, StackError, StatusCode};
use coco_goals::GoalTransitionError;
use thiserror::Error;

/// A failure applying a goal command or persisting its result.
#[derive(Debug, Error)]
pub enum GoalRuntimeError {
    /// The pure reducer rejected the command (stale identity, invalid
    /// transition, budget-raise required, …).
    #[error("goal transition rejected: {source}")]
    Transition {
        #[from]
        source: GoalTransitionError,
    },

    /// Durable persistence of the committed snapshot failed. The live
    /// projection is not advanced, preserving durable-before-visible ordering.
    #[error("goal store failure: {message}")]
    Store { message: String },

    /// Goal or plan context could not be materialized for a turn. The supervisor
    /// pauses the goal as `context_unavailable` rather than starting an
    /// unanchored turn (§5.5).
    #[error("goal context unavailable: {message}")]
    ContextUnavailable { message: String },
}

impl GoalRuntimeError {
    pub fn store(message: impl Into<String>) -> Self {
        Self::Store {
            message: message.into(),
        }
    }

    pub fn context_unavailable(message: impl Into<String>) -> Self {
        Self::ContextUnavailable {
            message: message.into(),
        }
    }

    /// Whether this failure is a stale-identity/version conflict the caller
    /// should resolve by refreshing the snapshot rather than retrying verbatim.
    pub fn is_conflict(&self) -> bool {
        matches!(
            self,
            Self::Transition {
                source: GoalTransitionError::StaleGoalId { .. }
                    | GoalTransitionError::SpecRevisionMismatch { .. }
                    | GoalTransitionError::StateVersionMismatch { .. }
                    | GoalTransitionError::LeaseMismatch
            }
        )
    }
}

impl StackError for GoalRuntimeError {
    fn debug_fmt(&self, layer: usize, buf: &mut Vec<String>) {
        buf.push(format!("{layer}: {self}"));
    }

    fn next(&self) -> Option<&dyn StackError> {
        None
    }
}

impl ErrorExt for GoalRuntimeError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::Transition { .. } => StatusCode::InvalidArguments,
            Self::Store { .. } => StatusCode::Internal,
            Self::ContextUnavailable { .. } => StatusCode::Internal,
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub type Result<T, E = GoalRuntimeError> = std::result::Result<T, E>;
