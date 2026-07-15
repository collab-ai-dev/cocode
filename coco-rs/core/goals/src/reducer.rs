//! The pure goal reducer.
//!
//! `decide(snapshot, command)` validates a command against the current snapshot and
//! returns the next snapshot plus typed effects. It performs no I/O, holds no locks,
//! reads no clock, and mints no ids — every non-deterministic input arrives through
//! the command. This makes the state machine exhaustively testable and lets the host
//! commit durably *before* publishing live state (§10.1).
//!
//! Two invariants are enforced structurally here and cannot regress silently:
//!
//! * a `create`/`resume`/`wake`/budget-resume that yields `active` always commits a
//!   queued lease in the same decision (§9.1 invariant 9) — never ownerless active;
//! * a `wait` transition always carries a durable [`GoalWake`], and always emits a
//!   `RegisterWake` effect.

use crate::budget::{GoalCounters, GoalUsage};
use crate::command::{
    AcceptCompletion, Clear, CreateGoal, Edit, FinishTurn, GoalCommand, Pause, RejectCompletion,
    Resume, StartTurn, TurnFinishOutcome, Wake,
};
use crate::completion::policy_can_judge_contract;
use crate::decision::{
    GoalAuditKind, GoalDecision, GoalEffect, GoalReminderKind, GoalTransitionEvent,
};
use crate::disposition::WaitCondition;
use crate::error::GoalTransitionError;
use crate::id::{GoalId, SpecRevision, StateVersion, Timestamp};
use crate::snapshot::{GoalSnapshot, SCHEMA_VERSION};
use crate::status::{BudgetKind, GoalLease, GoalLifecycle, GoalWake};

/// Apply one command to the current snapshot.
pub fn decide(
    snapshot: Option<&GoalSnapshot>,
    command: GoalCommand,
) -> Result<GoalDecision, GoalTransitionError> {
    match command {
        GoalCommand::Create(cmd) => create(snapshot, &cmd),
        GoalCommand::StartTurn(cmd) => start_turn(snapshot, &cmd),
        GoalCommand::FinishTurn(cmd) => finish_turn(snapshot, &cmd),
        GoalCommand::Wake(cmd) => wake(snapshot, &cmd),
        GoalCommand::Pause(cmd) => pause(snapshot, &cmd),
        GoalCommand::Resume(cmd) => resume(snapshot, &cmd),
        GoalCommand::Edit(cmd) => edit(snapshot, &cmd),
        GoalCommand::Clear(cmd) => clear(snapshot, &cmd),
        GoalCommand::AcceptCompletion(cmd) => accept_completion(snapshot, &cmd),
        GoalCommand::RejectCompletion(cmd) => reject_completion(snapshot, &cmd),
    }
}

/// Resolve the current snapshot and reject a command targeting a different goal.
fn ensure_goal<'a>(
    snapshot: Option<&'a GoalSnapshot>,
    goal_id: &GoalId,
) -> Result<&'a GoalSnapshot, GoalTransitionError> {
    let snapshot = snapshot.ok_or(GoalTransitionError::NoCurrentGoal)?;
    if &snapshot.goal_id != goal_id {
        return Err(GoalTransitionError::StaleGoalId {
            expected: snapshot.goal_id.to_string(),
            actual: goal_id.to_string(),
        });
    }
    Ok(snapshot)
}

/// Stamp a mutated snapshot as the next committed version.
fn commit(mut snapshot: GoalSnapshot, at: Timestamp) -> GoalSnapshot {
    snapshot.state_version = snapshot.state_version.next();
    snapshot.updated_at = at;
    snapshot
}

/// A queued lease for the given id at attempt 0.
fn queued(lease_id: crate::id::GoalLeaseId) -> GoalLease {
    GoalLease::Queued {
        lease_id,
        attempt: 0,
    }
}

fn create(
    snapshot: Option<&GoalSnapshot>,
    cmd: &CreateGoal,
) -> Result<GoalDecision, GoalTransitionError> {
    if let Some(existing) = snapshot
        && !existing.is_terminal()
    {
        return Err(GoalTransitionError::GoalAlreadyActive);
    }
    if let Some(contract) = &cmd.contract
        && !policy_can_judge_contract(cmd.policy, contract)
    {
        return Err(GoalTransitionError::InvalidPolicyForContract);
    }

    let (lifecycle, effects) = match cmd.mode_gate {
        Some(mode) => {
            let wake = GoalWake {
                wake_id: cmd.wake_id.clone(),
                condition: WaitCondition::ModeGate { mode },
            };
            (
                GoalLifecycle::Waiting { wake: wake.clone() },
                vec![GoalEffect::RegisterWake { wake }],
            )
        }
        None => (
            GoalLifecycle::Active {
                lease: queued(cmd.lease_id.clone()),
            },
            vec![GoalEffect::ScheduleTurn {
                lease_id: cmd.lease_id.clone(),
            }],
        ),
    };

    let snapshot = GoalSnapshot {
        schema_version: SCHEMA_VERSION,
        goal_id: cmd.goal_id.clone(),
        session_id: cmd.session_id.clone(),
        spec_revision: SpecRevision::INITIAL,
        state_version: StateVersion::INITIAL,
        objective: cmd.objective.clone(),
        contract: cmd.contract.clone(),
        policy: cmd.policy,
        lifecycle,
        plan: cmd.plan.clone(),
        budget: cmd.budget,
        usage: GoalUsage::default(),
        counters: GoalCounters::default(),
        progress: None,
        last_rejection: None,
        last_blocker: None,
        wait_resolution: None,
        created_at: cmd.at,
        updated_at: cmd.at,
    };

    Ok(GoalDecision {
        snapshot: Some(snapshot),
        effects,
        event: GoalTransitionEvent::Created,
    })
}

fn start_turn(
    snapshot: Option<&GoalSnapshot>,
    cmd: &StartTurn,
) -> Result<GoalDecision, GoalTransitionError> {
    let snapshot = ensure_goal(snapshot, &cmd.goal_id)?;
    let GoalLifecycle::Active {
        lease: GoalLease::Queued { lease_id, .. },
    } = &snapshot.lifecycle
    else {
        return Err(GoalTransitionError::InvalidTransition {
            from: snapshot.status(),
        });
    };
    if lease_id != &cmd.lease_id {
        return Err(GoalTransitionError::LeaseMismatch);
    }

    let mut next = snapshot.clone();

    // An autonomous start beyond the cap stops rather than running (§11.1).
    if cmd.trigger.spends_autonomous_quota() && next.counters.autonomous_exhausted(&next.budget) {
        next.lifecycle = GoalLifecycle::BudgetLimited {
            kind: BudgetKind::Turns,
            usage: next.usage,
        };
        return Ok(GoalDecision {
            snapshot: Some(commit(next, cmd.at)),
            effects: vec![GoalEffect::ReleaseLease {
                lease_id: cmd.lease_id.clone(),
            }],
            event: GoalTransitionEvent::BudgetLimited,
        });
    }

    next.counters.total_turns = next.counters.total_turns.saturating_add(1);
    if cmd.trigger.spends_autonomous_quota() {
        next.counters.autonomous_turns = next.counters.autonomous_turns.saturating_add(1);
        next.counters.continuations_since_user_turn = next
            .counters
            .continuations_since_user_turn
            .saturating_add(1);
    }
    if cmd.trigger.resets_probe_cadence() {
        next.counters.continuations_since_user_turn = 0;
        next.counters.probe_cooldown = false;
    }
    next.lifecycle = GoalLifecycle::Active {
        lease: GoalLease::Running {
            lease_id: cmd.lease_id.clone(),
            turn_id: cmd.turn_id.clone(),
        },
    };

    Ok(GoalDecision {
        snapshot: Some(commit(next, cmd.at)),
        effects: Vec::new(),
        event: GoalTransitionEvent::TurnStarted,
    })
}

fn finish_turn(
    snapshot: Option<&GoalSnapshot>,
    cmd: &FinishTurn,
) -> Result<GoalDecision, GoalTransitionError> {
    let snapshot = ensure_goal(snapshot, &cmd.goal_id)?;
    let GoalLifecycle::Active {
        lease: GoalLease::Running { lease_id, turn_id },
    } = &snapshot.lifecycle
    else {
        return Err(GoalTransitionError::InvalidTransition {
            from: snapshot.status(),
        });
    };
    if lease_id != &cmd.lease_id {
        return Err(GoalTransitionError::LeaseMismatch);
    }
    if turn_id != &cmd.turn_id {
        return Err(GoalTransitionError::TurnMismatch);
    }

    let mut next = snapshot.clone();
    next.usage.apply(cmd.usage);

    // Progress and report are separate counters (§12.2): prose is not a signal.
    if cmd.signals.is_empty() {
        next.counters.no_progress_streak = next.counters.no_progress_streak.saturating_add(1);
    } else {
        next.counters.no_progress_streak = 0;
    }
    if cmd.reported {
        next.counters.unreported_streak = 0;
    } else {
        next.counters.unreported_streak = next.counters.unreported_streak.saturating_add(1);
    }

    let token_exhausted = next
        .budget
        .max_tokens
        .is_some_and(|max| next.usage.total_tokens() > max.get());

    let release = GoalEffect::ReleaseLease {
        lease_id: cmd.lease_id.clone(),
    };
    let mut effects = Vec::new();

    let (lifecycle, event) = match &cmd.outcome {
        TurnFinishOutcome::Continue {
            next_lease_id,
            checkpoint,
            rejection,
        } => {
            if let Some(checkpoint) = checkpoint {
                next.progress = Some(checkpoint.clone());
            }
            if let Some(rejection) = rejection {
                next.last_rejection = Some(rejection.clone());
            }
            if token_exhausted {
                effects.push(release);
                (
                    GoalLifecycle::BudgetLimited {
                        kind: BudgetKind::Tokens,
                        usage: next.usage,
                    },
                    GoalTransitionEvent::BudgetLimited,
                )
            } else if next.counters.no_progress_tripped() {
                // Safety net: the coordinator runs the boundary audit before
                // pausing, but the no-progress invariant holds even if it does not.
                effects.push(release);
                (
                    GoalLifecycle::Paused {
                        reason: crate::status::PauseReason::NoProgress,
                    },
                    GoalTransitionEvent::Paused,
                )
            } else {
                effects.push(GoalEffect::ScheduleTurn {
                    lease_id: next_lease_id.clone(),
                });
                (
                    GoalLifecycle::Active {
                        lease: queued(next_lease_id.clone()),
                    },
                    GoalTransitionEvent::Continued,
                )
            }
        }
        TurnFinishOutcome::Wait { wake_id, condition } => {
            let wake = GoalWake {
                wake_id: wake_id.clone(),
                condition: condition.clone(),
            };
            next.counters.probe_cooldown = true;
            effects.push(release);
            effects.push(GoalEffect::RegisterWake { wake: wake.clone() });
            (
                GoalLifecycle::Waiting { wake },
                GoalTransitionEvent::EnteredWaiting,
            )
        }
        TurnFinishOutcome::Blocked { evidence } => {
            next.last_blocker = Some(evidence.clone());
            effects.push(release);
            (
                GoalLifecycle::Blocked {
                    evidence: evidence.clone(),
                },
                GoalTransitionEvent::Blocked,
            )
        }
        TurnFinishOutcome::Paused { reason } => {
            effects.push(release);
            (
                GoalLifecycle::Paused { reason: *reason },
                GoalTransitionEvent::Paused,
            )
        }
        TurnFinishOutcome::UsageLimited { reason } => {
            effects.push(release);
            (
                GoalLifecycle::UsageLimited {
                    reason: reason.clone(),
                },
                GoalTransitionEvent::UsageLimited,
            )
        }
        TurnFinishOutcome::BudgetLimited { kind } => {
            effects.push(release);
            (
                GoalLifecycle::BudgetLimited {
                    kind: *kind,
                    usage: next.usage,
                },
                GoalTransitionEvent::BudgetLimited,
            )
        }
        TurnFinishOutcome::Completed { authorization } => {
            if authorization.goal_id() != &next.goal_id
                || authorization.spec_revision() != next.spec_revision
                || authorization.lease_id() != &cmd.lease_id
            {
                return Err(GoalTransitionError::CompletionAuthorizationMismatch);
            }
            (
                GoalLifecycle::Completed {
                    evidence: authorization.evidence_summary().clone(),
                },
                GoalTransitionEvent::Completed,
            )
        }
    };

    next.lifecycle = lifecycle;
    Ok(GoalDecision {
        snapshot: Some(commit(next, cmd.at)),
        effects,
        event,
    })
}

fn wake(snapshot: Option<&GoalSnapshot>, cmd: &Wake) -> Result<GoalDecision, GoalTransitionError> {
    let snapshot = ensure_goal(snapshot, &cmd.goal_id)?;
    let GoalLifecycle::Waiting { wake } = &snapshot.lifecycle else {
        return Err(GoalTransitionError::InvalidTransition {
            from: snapshot.status(),
        });
    };
    if wake.wake_id != cmd.wake_id {
        return Err(GoalTransitionError::WakeNotFound);
    }

    let mut next = snapshot.clone();
    next.wait_resolution = cmd.resolution.clone();
    next.lifecycle = GoalLifecycle::Active {
        lease: queued(cmd.next_lease_id.clone()),
    };

    let mut effects = vec![GoalEffect::ScheduleTurn {
        lease_id: cmd.next_lease_id.clone(),
    }];
    if cmd.resolution.is_some() {
        effects.push(GoalEffect::EmitReminder(GoalReminderKind::WaitResolved));
    }

    Ok(GoalDecision {
        snapshot: Some(commit(next, cmd.at)),
        effects,
        event: GoalTransitionEvent::Woken,
    })
}

fn pause(
    snapshot: Option<&GoalSnapshot>,
    cmd: &Pause,
) -> Result<GoalDecision, GoalTransitionError> {
    let snapshot = ensure_goal(snapshot, &cmd.goal_id)?;
    let mut effects = Vec::new();
    match &snapshot.lifecycle {
        GoalLifecycle::Active { lease } => effects.push(GoalEffect::ReleaseLease {
            lease_id: lease.lease_id().clone(),
        }),
        GoalLifecycle::Waiting { wake } => effects.push(GoalEffect::CancelWake {
            wake_id: wake.wake_id.clone(),
        }),
        _ => {
            return Err(GoalTransitionError::InvalidTransition {
                from: snapshot.status(),
            });
        }
    }

    let mut next = snapshot.clone();
    next.lifecycle = GoalLifecycle::Paused { reason: cmd.reason };
    Ok(GoalDecision {
        snapshot: Some(commit(next, cmd.at)),
        effects,
        event: GoalTransitionEvent::Paused,
    })
}

fn resume(
    snapshot: Option<&GoalSnapshot>,
    cmd: &Resume,
) -> Result<GoalDecision, GoalTransitionError> {
    let snapshot = ensure_goal(snapshot, &cmd.goal_id)?;
    let mut effects = Vec::new();
    match &snapshot.lifecycle {
        GoalLifecycle::Paused { .. }
        | GoalLifecycle::Blocked { .. }
        | GoalLifecycle::UsageLimited { .. } => {}
        GoalLifecycle::Waiting { wake } => effects.push(GoalEffect::CancelWake {
            wake_id: wake.wake_id.clone(),
        }),
        GoalLifecycle::BudgetLimited { .. } => {
            return Err(GoalTransitionError::BudgetRaiseRequired);
        }
        GoalLifecycle::Active { .. } | GoalLifecycle::Completed { .. } => {
            return Err(GoalTransitionError::InvalidTransition {
                from: snapshot.status(),
            });
        }
    }

    let mut next = snapshot.clone();
    reset_resume_counters(&mut next.counters);
    next.lifecycle = GoalLifecycle::Active {
        lease: queued(cmd.next_lease_id.clone()),
    };
    effects.push(GoalEffect::ScheduleTurn {
        lease_id: cmd.next_lease_id.clone(),
    });

    Ok(GoalDecision {
        snapshot: Some(commit(next, cmd.at)),
        effects,
        event: GoalTransitionEvent::Resumed,
    })
}

/// Resume resets the no-progress/unreported/scheduler retry counters and probe
/// cooldown, but never the lifetime usage/turn budgets (§11.3 rule 4).
fn reset_resume_counters(counters: &mut GoalCounters) {
    counters.no_progress_streak = 0;
    counters.unreported_streak = 0;
    counters.scheduler_retries = 0;
    counters.probe_cooldown = false;
}

fn edit(snapshot: Option<&GoalSnapshot>, cmd: &Edit) -> Result<GoalDecision, GoalTransitionError> {
    let snapshot = ensure_goal(snapshot, &cmd.goal_id)?;
    if snapshot.spec_revision != cmd.expected_spec_revision {
        return Err(GoalTransitionError::SpecRevisionMismatch {
            expected: cmd.expected_spec_revision,
            actual: snapshot.spec_revision,
        });
    }

    let mut next = snapshot.clone();
    let mut effects = Vec::new();

    if let Some(objective) = &cmd.objective {
        if next.objective != *objective {
            effects.push(GoalEffect::EmitReminder(GoalReminderKind::ObjectiveChanged));
        }
        next.objective = objective.clone();
    }
    if cmd.clear_contract {
        next.contract = None;
    } else if let Some(contract) = &cmd.contract {
        next.contract = Some(contract.clone());
    }
    if let Some(policy) = cmd.policy {
        next.policy = policy;
    }
    if let Some(budget) = cmd.budget {
        next.budget = budget;
    }
    if let Some(plan) = &cmd.plan_binding {
        next.plan = Some(plan.clone());
        effects.push(GoalEffect::EmitReminder(GoalReminderKind::PlanActivated));
    }
    if let Some(contract) = &next.contract
        && !policy_can_judge_contract(next.policy, contract)
    {
        return Err(GoalTransitionError::InvalidPolicyForContract);
    }

    next.spec_revision = next.spec_revision.next();

    // Atomic budget-edit-and-resume from budget_limited (§11.3).
    if let (GoalLifecycle::BudgetLimited { kind, .. }, Some(next_lease_id)) =
        (&snapshot.lifecycle, &cmd.next_lease_id)
    {
        let sufficient = match kind {
            BudgetKind::Tokens => next
                .budget
                .max_tokens
                .is_some_and(|max| max.get() > next.usage.total_tokens()),
            BudgetKind::Turns => {
                next.budget.max_autonomous_turns.get() > next.counters.autonomous_turns
            }
        };
        if !sufficient {
            return Err(GoalTransitionError::InvalidBudgetEdit);
        }
        reset_resume_counters(&mut next.counters);
        next.lifecycle = GoalLifecycle::Active {
            lease: queued(next_lease_id.clone()),
        };
        effects.push(GoalEffect::ScheduleTurn {
            lease_id: next_lease_id.clone(),
        });
    }

    Ok(GoalDecision {
        snapshot: Some(commit(next, cmd.at)),
        effects,
        event: GoalTransitionEvent::Edited,
    })
}

fn clear(
    snapshot: Option<&GoalSnapshot>,
    cmd: &Clear,
) -> Result<GoalDecision, GoalTransitionError> {
    let snapshot = ensure_goal(snapshot, &cmd.goal_id)?;
    let mut effects = vec![GoalEffect::RecordAudit(GoalAuditKind::Cleared)];
    match &snapshot.lifecycle {
        GoalLifecycle::Active { lease } => effects.push(GoalEffect::ReleaseLease {
            lease_id: lease.lease_id().clone(),
        }),
        GoalLifecycle::Waiting { wake } => effects.push(GoalEffect::CancelWake {
            wake_id: wake.wake_id.clone(),
        }),
        _ => {}
    }
    Ok(GoalDecision {
        snapshot: None,
        effects,
        event: GoalTransitionEvent::Cleared,
    })
}

fn accept_completion(
    snapshot: Option<&GoalSnapshot>,
    cmd: &AcceptCompletion,
) -> Result<GoalDecision, GoalTransitionError> {
    let snapshot = snapshot.ok_or(GoalTransitionError::NoCurrentGoal)?;
    let GoalLifecycle::Waiting { wake } = &snapshot.lifecycle else {
        return Err(GoalTransitionError::InvalidTransition {
            from: snapshot.status(),
        });
    };
    if !matches!(wake.condition, WaitCondition::UserAcceptance) {
        return Err(GoalTransitionError::InvalidTransition {
            from: snapshot.status(),
        });
    }
    if cmd.authorization.goal_id() != &snapshot.goal_id
        || cmd.authorization.spec_revision() != snapshot.spec_revision
    {
        return Err(GoalTransitionError::CompletionAuthorizationMismatch);
    }

    let wake_id = wake.wake_id.clone();
    let mut next = snapshot.clone();
    next.lifecycle = GoalLifecycle::Completed {
        evidence: cmd.authorization.evidence_summary().clone(),
    };
    Ok(GoalDecision {
        snapshot: Some(commit(next, cmd.at)),
        effects: vec![GoalEffect::CancelWake { wake_id }],
        event: GoalTransitionEvent::Completed,
    })
}

fn reject_completion(
    snapshot: Option<&GoalSnapshot>,
    cmd: &RejectCompletion,
) -> Result<GoalDecision, GoalTransitionError> {
    let snapshot = ensure_goal(snapshot, &cmd.goal_id)?;
    let GoalLifecycle::Waiting { wake } = &snapshot.lifecycle else {
        return Err(GoalTransitionError::InvalidTransition {
            from: snapshot.status(),
        });
    };
    if !matches!(wake.condition, WaitCondition::UserAcceptance) {
        return Err(GoalTransitionError::InvalidTransition {
            from: snapshot.status(),
        });
    }

    let wake_id = wake.wake_id.clone();
    let mut next = snapshot.clone();
    next.last_rejection = Some(cmd.rejection.clone());
    next.lifecycle = GoalLifecycle::Active {
        lease: queued(cmd.next_lease_id.clone()),
    };
    Ok(GoalDecision {
        snapshot: Some(commit(next, cmd.at)),
        effects: vec![
            GoalEffect::CancelWake { wake_id },
            GoalEffect::ScheduleTurn {
                lease_id: cmd.next_lease_id.clone(),
            },
        ],
        event: GoalTransitionEvent::CompletionRejected,
    })
}

#[cfg(test)]
#[path = "reducer.test.rs"]
mod tests;
