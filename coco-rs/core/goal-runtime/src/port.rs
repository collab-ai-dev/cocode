//! `SessionTurnPort` — the explicit session scheduling seam (§10.2).
//!
//! Starts a goal-owned turn for one session with typed context and returns an
//! owned handle whose completion resolves **exactly once** to an exhaustive
//! outcome. The port wrapper synthesizes an error outcome if the underlying runner
//! exits without a proper turn end, so the supervisor never infers completion from
//! an optional protocol event. A first version adapts the existing local AppServer
//! turn slot; this crate owns the contract and a test double.

use async_trait::async_trait;
use coco_goals::{
    GoalId, GoalLeaseId, GoalTurnDisposition, GoalTurnTrigger, ProgressSignal, UsageDelta,
};
use coco_types::{SessionId, TurnId};

use crate::error::Result;
use crate::materializer::GoalTurnContext;

/// A request to start one goal-owned turn. The supervisor mints `turn_id` (the
/// runner owns turn ids) and records `running(lease, turn_id)` before the port
/// starts, so the durable state never trails the live runner.
pub struct GoalTurnRequest {
    pub session_id: SessionId,
    pub goal_id: GoalId,
    pub lease_id: GoalLeaseId,
    pub turn_id: TurnId,
    pub trigger: GoalTurnTrigger,
    pub context: GoalTurnContext,
}

/// An owned handle to a started turn. The completion resolves once.
pub struct GoalTurnHandle {
    pub turn_id: TurnId,
    pub completion: GoalTurnCompletion,
}

/// Resolves exactly once to the turn's outcome. Backed by a oneshot so a dropped
/// sender surfaces as [`GoalTurnOutcome::ChannelClosed`] rather than hanging.
pub struct GoalTurnCompletion {
    receiver: tokio::sync::oneshot::Receiver<GoalTurnOutcome>,
}

impl GoalTurnCompletion {
    pub fn new(receiver: tokio::sync::oneshot::Receiver<GoalTurnOutcome>) -> Self {
        Self { receiver }
    }

    /// Await the single outcome. A dropped sender (runner died) resolves to
    /// [`GoalTurnOutcome::ChannelClosed`].
    pub async fn wait(self) -> GoalTurnOutcome {
        self.receiver
            .await
            .unwrap_or(GoalTurnOutcome::ChannelClosed)
    }
}

/// Whether a provider error is worth an automatic backoff retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderErrorKind {
    /// Transient (429/529/network): wait under `provider_backoff` and retry.
    Retryable,
    /// Non-retryable: block until the user resumes.
    Fatal,
}

/// The exhaustive outcome of a goal-owned turn.
pub enum GoalTurnOutcome {
    /// The turn ended normally; carries what the worker produced.
    Ended {
        disposition: GoalTurnDisposition,
        signals: Vec<ProgressSignal>,
        usage: UsageDelta,
    },
    /// The turn was interrupted (user cancel / preemption).
    Interrupted,
    /// A provider/API error stopped the turn.
    ProviderError {
        kind: ProviderErrorKind,
        message: String,
    },
    /// The account/provider usage limit was hit.
    UsageLimited { message: String },
    /// A tool error stopped the turn.
    ToolError { message: String },
    /// The runner panicked or exited without emitting a turn end.
    RunnerFailed,
    /// The completion channel closed before an outcome arrived.
    ChannelClosed,
}

/// Starts goal-owned turns for an explicit session.
#[async_trait]
pub trait SessionTurnPort: Send + Sync {
    async fn start_goal_turn(&self, request: GoalTurnRequest) -> Result<GoalTurnHandle>;
}
