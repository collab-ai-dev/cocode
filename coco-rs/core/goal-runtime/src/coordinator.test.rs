use super::*;
use crate::test_support::{
    evidence_record, evidence_ref, goal_id, running_snapshot, satisfied_coverage, turn_result,
};
use crate::verifier::{AlwaysRejected, AlwaysUnavailable, AlwaysVerified};
use crate::{CompletionVerifier, EvidenceStore, InMemoryEvidenceStore};
use coco_goals::{
    BoundedText, CheckExpectation, CheckKind, CompletionContract, CompletionPolicy,
    CompletionRejectReason, ContractItem, DeterministicCheck, EvidenceRef, GoalReminderKind,
    GoalTurnDisposition, PauseReason, ProgressSignal, SpecRevision, TurnFinishOutcome,
    WaitCondition,
};
use pretty_assertions::assert_eq;
use std::sync::Arc;

fn empty_evidence() -> Arc<dyn EvidenceStore> {
    Arc::new(InMemoryEvidenceStore::new())
}

fn evidence_with(ids: &[&str]) -> Arc<dyn EvidenceStore> {
    let store = InMemoryEvidenceStore::new();
    for id in ids {
        store.record(evidence_record(id, &goal_id())).unwrap();
    }
    Arc::new(store)
}

fn coordinator(
    verifier: Arc<dyn CompletionVerifier>,
    evidence: Arc<dyn EvidenceStore>,
) -> GoalCompletionCoordinator {
    GoalCompletionCoordinator::new(evidence, verifier)
}

fn completion(evidence: Vec<EvidenceRef>) -> GoalTurnDisposition {
    GoalTurnDisposition::CompletionCandidate {
        coverage: satisfied_coverage(),
        evidence,
    }
}

fn progress() -> GoalTurnDisposition {
    GoalTurnDisposition::Progress {
        summary: BoundedText::short("did work"),
        next_step: BoundedText::short("more work"),
        evidence: Vec::new(),
    }
}

#[tokio::test]
async fn test_progress_continues() {
    let snapshot = running_snapshot(CompletionPolicy::CandidateWithEvidence, None);
    let coord = coordinator(Arc::new(AlwaysVerified), empty_evidence());
    let out = coord
        .coordinate(
            &snapshot,
            turn_result(progress(), vec![ProgressSignal::ToolObservation]),
        )
        .await
        .unwrap();
    assert!(matches!(out.outcome, TurnFinishOutcome::Continue { .. }));
    assert!(out.reminders.is_empty());
}

#[tokio::test]
async fn test_unreported_continues_with_report_missing_reminder() {
    let snapshot = running_snapshot(CompletionPolicy::CandidateWithEvidence, None);
    let coord = coordinator(Arc::new(AlwaysVerified), empty_evidence());
    let out = coord
        .coordinate(
            &snapshot,
            turn_result(
                GoalTurnDisposition::Unreported,
                vec![ProgressSignal::WorkspaceChange],
            ),
        )
        .await
        .unwrap();
    assert!(matches!(out.outcome, TurnFinishOutcome::Continue { .. }));
    assert!(out.reminders.contains(&GoalReminderKind::ReportMissing));
}

#[tokio::test]
async fn test_waiting_disposition_produces_wait() {
    let snapshot = running_snapshot(CompletionPolicy::CandidateWithEvidence, None);
    let coord = coordinator(Arc::new(AlwaysVerified), empty_evidence());
    let out = coord
        .coordinate(
            &snapshot,
            turn_result(
                GoalTurnDisposition::Waiting {
                    condition: WaitCondition::External {
                        description: BoundedText::short("ci"),
                    },
                },
                Vec::new(),
            ),
        )
        .await
        .unwrap();
    assert!(matches!(out.outcome, TurnFinishOutcome::Wait { .. }));
}

#[tokio::test]
async fn test_completion_candidate_verified_completes() {
    let snapshot = running_snapshot(CompletionPolicy::CandidateWithEvidence, None);
    let coord = coordinator(Arc::new(AlwaysVerified), evidence_with(&["e-1"]));
    let out = coord
        .coordinate(
            &snapshot,
            turn_result(completion(vec![evidence_ref("e-1")]), Vec::new()),
        )
        .await
        .unwrap();
    assert!(matches!(out.outcome, TurnFinishOutcome::Completed { .. }));
}

#[tokio::test]
async fn test_completion_candidate_rejected_continues_with_reason() {
    let snapshot = running_snapshot(CompletionPolicy::CandidateWithEvidence, None);
    let coord = coordinator(Arc::new(AlwaysRejected), evidence_with(&["e-1"]));
    let out = coord
        .coordinate(
            &snapshot,
            turn_result(completion(vec![evidence_ref("e-1")]), Vec::new()),
        )
        .await
        .unwrap();
    match out.outcome {
        TurnFinishOutcome::Continue { rejection, .. } => assert!(rejection.is_some()),
        other => panic!("expected continue-with-rejection, got {other:?}"),
    }
}

#[tokio::test]
async fn test_completion_candidate_unavailable_pauses() {
    let snapshot = running_snapshot(CompletionPolicy::CandidateWithEvidence, None);
    let coord = coordinator(Arc::new(AlwaysUnavailable), evidence_with(&["e-1"]));
    let out = coord
        .coordinate(
            &snapshot,
            turn_result(completion(vec![evidence_ref("e-1")]), Vec::new()),
        )
        .await
        .unwrap();
    assert!(matches!(
        out.outcome,
        TurnFinishOutcome::Paused {
            reason: PauseReason::VerificationUnavailable
        }
    ));
}

#[tokio::test]
async fn test_unowned_evidence_rejected_despite_verified() {
    let snapshot = running_snapshot(CompletionPolicy::CandidateWithEvidence, None);
    // Cite e-1 but never record it: structural ownership check fails first.
    let coord = coordinator(Arc::new(AlwaysVerified), empty_evidence());
    let out = coord
        .coordinate(
            &snapshot,
            turn_result(completion(vec![evidence_ref("e-1")]), Vec::new()),
        )
        .await
        .unwrap();
    match out.outcome {
        TurnFinishOutcome::Continue { rejection, .. } => {
            assert_eq!(
                rejection.unwrap().reason,
                CompletionRejectReason::EvidenceOwnershipFailed
            )
        }
        other => panic!("expected continue-with-rejection, got {other:?}"),
    }
}

#[tokio::test]
async fn test_no_progress_boundary_pauses_when_uncheckable() {
    let mut snapshot = running_snapshot(CompletionPolicy::CandidateWithEvidence, None);
    snapshot.counters.no_progress_streak = 2;
    let coord = coordinator(Arc::new(AlwaysVerified), empty_evidence());
    // Signal-free third turn with no deterministic coverage → the boundary audit
    // cannot prove completion, so it pauses.
    let out = coord
        .coordinate(
            &snapshot,
            turn_result(GoalTurnDisposition::Unreported, Vec::new()),
        )
        .await
        .unwrap();
    assert!(matches!(
        out.outcome,
        TurnFinishOutcome::Paused {
            reason: PauseReason::NoProgress
        }
    ));
}

#[tokio::test]
async fn test_user_acceptance_parks_valid_candidate() {
    let snapshot = running_snapshot(CompletionPolicy::UserAcceptance, None);
    let coord = coordinator(Arc::new(AlwaysVerified), evidence_with(&["e-1"]));
    let out = coord
        .coordinate(
            &snapshot,
            turn_result(completion(vec![evidence_ref("e-1")]), Vec::new()),
        )
        .await
        .unwrap();
    assert!(matches!(
        out.outcome,
        TurnFinishOutcome::Wait {
            condition: WaitCondition::UserAcceptance,
            ..
        }
    ));
}

#[tokio::test]
async fn test_contract_checks_boundary_audit_can_complete() {
    let contract = CompletionContract {
        items: vec![ContractItem::Check(DeterministicCheck {
            description: BoundedText::short("tests pass"),
            kind: CheckKind::Command {
                command: BoundedText::short("just test"),
                expect: CheckExpectation::Success,
            },
        })],
        referenced_docs: Vec::new(),
        approved_at_spec: SpecRevision::INITIAL,
    };
    let mut snapshot = running_snapshot(CompletionPolicy::ContractChecks, Some(contract));
    snapshot.counters.no_progress_streak = 2;
    // Checks green (AlwaysVerified) → the boundary audit completes even without a
    // worker report.
    let coord = coordinator(Arc::new(AlwaysVerified), empty_evidence());
    let out = coord
        .coordinate(
            &snapshot,
            turn_result(GoalTurnDisposition::Unreported, Vec::new()),
        )
        .await
        .unwrap();
    assert!(matches!(out.outcome, TurnFinishOutcome::Completed { .. }));
}
