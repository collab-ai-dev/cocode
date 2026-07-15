//! `GoalSupervisor` — the sole owner of autonomous continuation (§10.2).
//!
//! Level-triggered: `advance()` reconciles the durable snapshot against the turn
//! slot and starts at most one autonomous turn when the goal is active with a
//! queued lease. The caller drives it on every relevant edge (idle, resume, turn
//! stop, wake); missing or duplicated edges cause an idempotent reconciliation
//! rather than an ownerless state. It never infers completion from a protocol
//! event — the [`SessionTurnPort`] completion handle resolves exactly once.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use coco_goals::{
    BlockerEvidence, BoundedText, FinishTurn, GoalCommand, GoalId, GoalLease, GoalLeaseId,
    GoalLifecycle, GoalStatus, GoalTurnTrigger, Pause, PauseReason, ProgressSignal, StartTurn,
    Timestamp, TurnFinishOutcome, UsageDelta, UsageLimitReason, VerificationAttemptId,
    WaitCondition, WakeId,
};
use coco_types::TurnId;

use crate::admission::AutonomousAdmission;
use crate::coordinator::{GoalCompletionCoordinator, GoalTurnResult};
use crate::error::Result;
use crate::handle::GoalRuntimeHandle;
use crate::materializer::GoalContextMaterializer;
use crate::port::{GoalTurnOutcome, GoalTurnRequest, ProviderErrorKind, SessionTurnPort};

/// Bounded provider-backoff before an automatic retry.
const PROVIDER_BACKOFF_MS: i64 = 30_000;

/// What one reconciliation did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdvanceOutcome {
    /// No goal exists.
    NoGoal,
    /// The goal is not in a startable state (running/waiting/stopped/terminal).
    NotStartable,
    /// Context could not be materialized; the goal was paused.
    PausedContextUnavailable,
    /// The autonomous turn budget was exhausted; the goal is budget-limited.
    BudgetLimited,
    /// The goal changed (paused/cleared/replaced) while the turn ran.
    Superseded,
    /// One turn was started and finalized.
    Advanced,
}

/// Drives autonomous continuation for one session's goal.
pub struct GoalSupervisor {
    handle: Arc<GoalRuntimeHandle>,
    turn_port: Arc<dyn SessionTurnPort>,
    materializer: Arc<GoalContextMaterializer>,
    coordinator: Arc<GoalCompletionCoordinator>,
    admission: AutonomousAdmission,
}

impl GoalSupervisor {
    pub fn new(
        handle: Arc<GoalRuntimeHandle>,
        turn_port: Arc<dyn SessionTurnPort>,
        materializer: Arc<GoalContextMaterializer>,
        coordinator: Arc<GoalCompletionCoordinator>,
        admission: AutonomousAdmission,
    ) -> Self {
        Self {
            handle,
            turn_port,
            materializer,
            coordinator,
            admission,
        }
    }

    /// Reconcile once: start and finalize one autonomous turn if the goal is
    /// active with a queued lease. Idempotent when the goal is not startable.
    pub async fn advance(&self) -> Result<AdvanceOutcome> {
        let Some(snapshot) = self.handle.snapshot().await else {
            return Ok(AdvanceOutcome::NoGoal);
        };
        let GoalLifecycle::Active {
            lease: GoalLease::Queued { lease_id, .. },
        } = &snapshot.lifecycle
        else {
            return Ok(AdvanceOutcome::NotStartable);
        };
        let lease_id = lease_id.clone();

        // Materialize from the queued snapshot *before* starting, so a context
        // failure pauses without a running turn (§5.5).
        let context = match self.materializer.materialize(&snapshot) {
            Ok(context) => context,
            Err(_) => {
                self.handle
                    .apply(GoalCommand::Pause(Pause {
                        goal_id: snapshot.goal_id.clone(),
                        reason: PauseReason::ContextUnavailable,
                        at: now(),
                    }))
                    .await?;
                return Ok(AdvanceOutcome::PausedContextUnavailable);
            }
        };

        // Record running before the port starts (closes the persist-then-schedule
        // window). The reducer converts an over-budget autonomous start into
        // budget_limited, so no orphaned turn is possible.
        let turn_id = mint_turn();
        let trigger = GoalTurnTrigger::Autonomous;
        let started = self
            .handle
            .apply(GoalCommand::StartTurn(StartTurn {
                goal_id: snapshot.goal_id.clone(),
                lease_id: lease_id.clone(),
                turn_id: turn_id.clone(),
                trigger,
                at: now(),
            }))
            .await?;
        match started
            .snapshot
            .as_ref()
            .map(coco_goals::GoalSnapshot::status)
        {
            Some(GoalStatus::Active) => {}
            Some(GoalStatus::BudgetLimited) => return Ok(AdvanceOutcome::BudgetLimited),
            _ => return Ok(AdvanceOutcome::Superseded),
        }

        let permit = self.admission.acquire(&snapshot.session_id).await;
        let turn = self
            .turn_port
            .start_goal_turn(GoalTurnRequest {
                session_id: snapshot.session_id.clone(),
                goal_id: snapshot.goal_id.clone(),
                lease_id: lease_id.clone(),
                turn_id: turn_id.clone(),
                trigger,
                context,
            })
            .await?;
        let outcome = turn.completion.wait().await;
        drop(permit);

        self.finalize(&snapshot.goal_id, &lease_id, &turn_id, outcome)
            .await
    }

    async fn finalize(
        &self,
        goal_id: &GoalId,
        lease_id: &GoalLeaseId,
        turn_id: &TurnId,
        outcome: GoalTurnOutcome,
    ) -> Result<AdvanceOutcome> {
        let Some(snapshot) = self.handle.snapshot().await else {
            return Ok(AdvanceOutcome::Superseded);
        };
        // Only finalize if the goal still runs THIS turn; otherwise it was paused,
        // cleared, or replaced concurrently (e.g. Ctrl+C paused before cancel).
        let still_running = matches!(
            &snapshot.lifecycle,
            GoalLifecycle::Active {
                lease: GoalLease::Running { lease_id: l, turn_id: t },
            } if l == lease_id && t == turn_id
        );
        if !still_running {
            return Ok(AdvanceOutcome::Superseded);
        }

        let finalization = match outcome {
            GoalTurnOutcome::Ended {
                disposition,
                signals,
                usage,
            } => {
                let reported = disposition.is_reported();
                let signals_for_finish = signals.clone();
                let result = GoalTurnResult {
                    disposition,
                    signals,
                    next_lease_id: mint_lease(),
                    wake_id: mint_wake(),
                    verification_attempt: mint_attempt(),
                    at: now(),
                };
                let coordinated = self.coordinator.coordinate(&snapshot, result).await?;
                Finalization {
                    reported,
                    signals: signals_for_finish,
                    usage,
                    outcome: coordinated.outcome,
                }
            }
            GoalTurnOutcome::Interrupted => {
                // Fallback: the control plane normally pauses before cancelling.
                self.handle
                    .apply(GoalCommand::Pause(Pause {
                        goal_id: goal_id.clone(),
                        reason: PauseReason::UserInterrupt,
                        at: now(),
                    }))
                    .await?;
                return Ok(AdvanceOutcome::Advanced);
            }
            GoalTurnOutcome::ProviderError { kind, message } => {
                let outcome = match kind {
                    ProviderErrorKind::Retryable => TurnFinishOutcome::Wait {
                        wake_id: mint_wake(),
                        condition: WaitCondition::ProviderBackoff {
                            attempt: 1,
                            deadline: now_plus(PROVIDER_BACKOFF_MS),
                        },
                    },
                    ProviderErrorKind::Fatal => TurnFinishOutcome::Blocked {
                        evidence: BlockerEvidence::ExecutionError {
                            message: BoundedText::short(message),
                        },
                    },
                };
                Finalization::system(outcome)
            }
            GoalTurnOutcome::UsageLimited { message } => {
                Finalization::system(TurnFinishOutcome::UsageLimited {
                    reason: UsageLimitReason {
                        detail: BoundedText::short(message),
                        reset_deadline: None,
                    },
                })
            }
            GoalTurnOutcome::ToolError { message } => {
                Finalization::system(TurnFinishOutcome::Blocked {
                    evidence: BlockerEvidence::ExecutionError {
                        message: BoundedText::short(message),
                    },
                })
            }
            GoalTurnOutcome::RunnerFailed | GoalTurnOutcome::ChannelClosed => {
                Finalization::system(TurnFinishOutcome::Blocked {
                    evidence: BlockerEvidence::ExecutionError {
                        message: BoundedText::short("goal turn runner exited without a result"),
                    },
                })
            }
        };

        self.handle
            .apply(GoalCommand::FinishTurn(FinishTurn {
                goal_id: goal_id.clone(),
                lease_id: lease_id.clone(),
                turn_id: turn_id.clone(),
                reported: finalization.reported,
                signals: finalization.signals,
                usage: finalization.usage,
                outcome: finalization.outcome,
                at: now(),
            }))
            .await?;
        Ok(AdvanceOutcome::Advanced)
    }
}

/// Arguments for the finalizing `FinishTurn`.
struct Finalization {
    reported: bool,
    signals: Vec<ProgressSignal>,
    usage: UsageDelta,
    outcome: TurnFinishOutcome,
}

impl Finalization {
    /// A system-driven finalization (error/limit) with no worker report.
    fn system(outcome: TurnFinishOutcome) -> Self {
        Self {
            reported: false,
            signals: Vec::new(),
            usage: UsageDelta::default(),
            outcome,
        }
    }
}

fn now() -> Timestamp {
    Timestamp::from_millis(unix_millis())
}

fn now_plus(delta_ms: i64) -> Timestamp {
    Timestamp::from_millis(unix_millis().saturating_add(delta_ms))
}

fn unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

fn mint_turn() -> TurnId {
    TurnId::new(format!("goal-turn-{}", uuid::Uuid::new_v4()))
}

fn mint_lease() -> GoalLeaseId {
    GoalLeaseId::new(format!("lease-{}", uuid::Uuid::new_v4()))
}

fn mint_wake() -> WakeId {
    WakeId::new(format!("wake-{}", uuid::Uuid::new_v4()))
}

fn mint_attempt() -> VerificationAttemptId {
    VerificationAttemptId::new(format!("va-{}", uuid::Uuid::new_v4()))
}

#[cfg(test)]
#[path = "supervisor.test.rs"]
mod tests;
