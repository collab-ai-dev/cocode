//! `GoalCompletionCoordinator` — the mandatory, deterministic after-turn boundary
//! (§6, §12). It runs on *every* goal-owned turn result, cannot fail open, and
//! contains no LLM judge of its own. It normalizes the worker disposition,
//! evaluates completion candidates through the gate, runs the mandatory boundary
//! audit before a no-progress/blocked stop, and returns the `TurnFinishOutcome`
//! the runtime hands to the reducer.

use std::sync::Arc;

use coco_goals::{
    CandidateSource, CompletionCandidate, CompletionOutcome, CompletionPolicy, ContractItem,
    GoalLeaseId, GoalReminderKind, GoalSnapshot, GoalTurnDisposition, NO_PROGRESS_LIMIT,
    PauseReason, ProgressCheckpoint, ProgressSignal, RequirementCoverage, RequirementResult,
    Timestamp, TurnFinishOutcome, VerificationAttemptId, WaitCondition, WakeId, precheck_candidate,
};

use crate::error::Result;
use crate::evidence::EvidenceStore;
use crate::gate::GoalCompletionGate;
use crate::verifier::{CompletionVerifier, VerificationRequest};

/// The worker's turn outcome plus the ids the runtime pre-minted for a possible
/// continuation or wait. Identity (goal/lease/turn) lives on the surrounding
/// `FinishTurn` the caller builds from [`CoordinatorOutcome`].
pub struct GoalTurnResult {
    pub disposition: GoalTurnDisposition,
    /// Accepted runtime progress signals for the turn (§9.5).
    pub signals: Vec<ProgressSignal>,
    /// Queued lease id for a continuation.
    pub next_lease_id: GoalLeaseId,
    /// Wake id for a wait/acceptance transition.
    pub wake_id: WakeId,
    /// Durable verification-attempt id.
    pub verification_attempt: VerificationAttemptId,
    pub at: Timestamp,
}

/// The coordinator's decision: the transition to enact plus one-shot reminders.
pub struct CoordinatorOutcome {
    pub outcome: TurnFinishOutcome,
    pub reminders: Vec<GoalReminderKind>,
}

/// Deterministic after-turn coordinator.
pub struct GoalCompletionCoordinator {
    evidence: Arc<dyn EvidenceStore>,
    verifier: Arc<dyn CompletionVerifier>,
}

impl GoalCompletionCoordinator {
    pub fn new(evidence: Arc<dyn EvidenceStore>, verifier: Arc<dyn CompletionVerifier>) -> Self {
        Self { evidence, verifier }
    }

    /// Coordinate one goal-owned turn result against the current running snapshot.
    pub async fn coordinate(
        &self,
        snapshot: &GoalSnapshot,
        result: GoalTurnResult,
    ) -> Result<CoordinatorOutcome> {
        let mut reminders = Vec::new();
        if !result.disposition.is_reported() {
            reminders.push(GoalReminderKind::ReportMissing);
        }

        let outcome = match &result.disposition {
            GoalTurnDisposition::Waiting { condition } => TurnFinishOutcome::Wait {
                wake_id: result.wake_id.clone(),
                condition: condition.clone(),
            },
            GoalTurnDisposition::BlockedCandidate { evidence } => {
                // A mandatory boundary audit runs before a stop so a forgotten
                // completion is not silently lost (§12.2).
                match self.boundary_audit(snapshot, &result).await? {
                    Some(completed) => completed,
                    None => TurnFinishOutcome::Blocked {
                        evidence: evidence.clone(),
                    },
                }
            }
            GoalTurnDisposition::CompletionCandidate { coverage, evidence } => {
                let candidate = CompletionCandidate {
                    source: CandidateSource::WorkerReport,
                    coverage: coverage.clone(),
                    evidence: evidence.clone(),
                    plan_observed: snapshot.plan.clone(),
                };
                self.judge_candidate(snapshot, &result, candidate, CandidateSource::WorkerReport)
                    .await?
            }
            GoalTurnDisposition::Progress {
                summary, next_step, ..
            } => {
                let checkpoint = Some(ProgressCheckpoint {
                    summary: summary.clone(),
                    next_step: next_step.clone(),
                    at: result.at,
                });
                self.continue_or_boundary(snapshot, &result, checkpoint)
                    .await?
            }
            GoalTurnDisposition::Unreported => {
                self.continue_or_boundary(snapshot, &result, None).await?
            }
        };

        Ok(CoordinatorOutcome { outcome, reminders })
    }

    /// Resolve, verify, and gate a completion candidate.
    async fn judge_candidate(
        &self,
        snapshot: &GoalSnapshot,
        result: &GoalTurnResult,
        candidate: CompletionCandidate,
        source: CandidateSource,
    ) -> Result<TurnFinishOutcome> {
        let ids: Vec<_> = candidate
            .evidence
            .iter()
            .map(|reference| reference.evidence_id.clone())
            .collect();
        let resolved = self.evidence.resolve(&ids)?;

        // Under user-acceptance the gate validates structure, then the goal parks
        // for an explicit accept — it does not complete here (§12.3).
        if snapshot.policy == CompletionPolicy::UserAcceptance {
            return Ok(
                match precheck_candidate(
                    &snapshot.goal_id,
                    snapshot.plan.as_ref(),
                    &candidate,
                    &resolved,
                ) {
                    Ok(_summary) => TurnFinishOutcome::Wait {
                        wake_id: result.wake_id.clone(),
                        condition: WaitCondition::UserAcceptance,
                    },
                    Err(rejection) => continue_with_rejection(result, rejection),
                },
            );
        }

        let verification = self
            .verifier
            .verify(VerificationRequest {
                goal_id: snapshot.goal_id.clone(),
                spec_revision: snapshot.spec_revision,
                objective: snapshot.objective.text.to_string(),
                contract: snapshot.contract.clone(),
                policy: snapshot.policy,
                source,
                candidate: candidate.clone(),
                attempt: result.verification_attempt.clone(),
            })
            .await;

        Ok(
            match GoalCompletionGate::evaluate(snapshot, &candidate, &resolved, verification) {
                CompletionOutcome::Authorized(authorization) => {
                    TurnFinishOutcome::Completed { authorization }
                }
                CompletionOutcome::Rejected(rejection) => {
                    continue_with_rejection(result, rejection)
                }
                CompletionOutcome::Unavailable => TurnFinishOutcome::Paused {
                    reason: PauseReason::VerificationUnavailable,
                },
            },
        )
    }

    /// A progress/unreported turn continues, unless it trips the no-progress
    /// boundary — where the mandatory audit runs before pausing (§9.5, §12.2).
    async fn continue_or_boundary(
        &self,
        snapshot: &GoalSnapshot,
        result: &GoalTurnResult,
        checkpoint: Option<ProgressCheckpoint>,
    ) -> Result<TurnFinishOutcome> {
        let trips_no_progress = result.signals.is_empty()
            && snapshot.counters.no_progress_streak + 1 >= NO_PROGRESS_LIMIT;
        if trips_no_progress {
            return Ok(match self.boundary_audit(snapshot, result).await? {
                Some(completed) => completed,
                None => TurnFinishOutcome::Paused {
                    reason: PauseReason::NoProgress,
                },
            });
        }

        Ok(TurnFinishOutcome::Continue {
            next_lease_id: result.next_lease_id.clone(),
            checkpoint,
            rejection: None,
        })
    }

    /// Derive a system candidate from deterministic contract coverage and gate it.
    /// For a free-form goal with no deterministic coverage there is nothing to
    /// prove, so it returns `None` and the caller commits the original stop.
    async fn boundary_audit(
        &self,
        snapshot: &GoalSnapshot,
        result: &GoalTurnResult,
    ) -> Result<Option<TurnFinishOutcome>> {
        let Some(candidate) = system_candidate_from_contract(snapshot) else {
            return Ok(None);
        };
        let outcome = self
            .judge_candidate(snapshot, result, candidate, CandidateSource::BoundaryAudit)
            .await?;
        Ok(match outcome {
            completed @ TurnFinishOutcome::Completed { .. } => Some(completed),
            _ => None,
        })
    }
}

/// Continue to the next queued lease while recording why a candidate was rejected.
fn continue_with_rejection(
    result: &GoalTurnResult,
    rejection: coco_goals::CompletionRejection,
) -> TurnFinishOutcome {
    TurnFinishOutcome::Continue {
        next_lease_id: result.next_lease_id.clone(),
        checkpoint: None,
        rejection: Some(rejection),
    }
}

/// Build a boundary-audit candidate from a contract that has deterministic
/// coverage. Returns `None` for a contract-less or checks-less goal, whose
/// completion cannot be proven at a boundary (§12.3).
fn system_candidate_from_contract(snapshot: &GoalSnapshot) -> Option<CompletionCandidate> {
    let contract = snapshot.contract.as_ref()?;
    if !contract.has_checks() {
        return None;
    }
    let requirements = contract
        .items
        .iter()
        .map(|item| {
            let requirement = match item {
                ContractItem::Check(check) => check.description.clone(),
                ContractItem::Criterion(criterion) => criterion.claim.clone(),
            };
            RequirementResult {
                requirement,
                satisfied: true,
                evidence: Vec::new(),
            }
        })
        .collect();
    Some(CompletionCandidate {
        source: CandidateSource::BoundaryAudit,
        coverage: RequirementCoverage {
            requirements,
            asserts_complete: true,
        },
        evidence: Vec::new(),
        plan_observed: snapshot.plan.clone(),
    })
}

#[cfg(test)]
#[path = "coordinator.test.rs"]
mod tests;
