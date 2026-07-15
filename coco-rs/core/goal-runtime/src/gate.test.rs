use super::*;
use crate::test_support::{
    evidence_record, evidence_ref, goal_id, queued_snapshot, running_snapshot, satisfied_coverage,
};
use coco_goals::{
    BoundedText, CandidateSource, CompletionCandidate, CompletionEvidenceSummary,
    CompletionOutcome, CompletionPolicy, CompletionRejectReason, VerificationOutcome,
};
use pretty_assertions::assert_eq;

fn candidate() -> CompletionCandidate {
    CompletionCandidate {
        source: CandidateSource::WorkerReport,
        coverage: satisfied_coverage(),
        evidence: vec![evidence_ref("e-1")],
        plan_observed: None,
    }
}

fn verified() -> VerificationOutcome {
    VerificationOutcome::Verified {
        summary: CompletionEvidenceSummary {
            summary: BoundedText::short("ok"),
            verified_requirements: Vec::new(),
            cited_evidence: Vec::new(),
        },
    }
}

#[test]
fn test_gate_authorizes_verified_candidate_with_owned_evidence() {
    let snapshot = running_snapshot(CompletionPolicy::CandidateWithEvidence, None);
    let records = [evidence_record("e-1", &goal_id())];
    let outcome = GoalCompletionGate::evaluate(&snapshot, &candidate(), &records, verified());
    assert!(matches!(outcome, CompletionOutcome::Authorized(_)));
}

#[test]
fn test_gate_rejects_when_no_running_lease() {
    // A queued (not running) goal cannot complete: there is no turn to bind.
    let snapshot = queued_snapshot(CompletionPolicy::CandidateWithEvidence);
    let records = [evidence_record("e-1", &goal_id())];
    let outcome = GoalCompletionGate::evaluate(&snapshot, &candidate(), &records, verified());
    match outcome {
        CompletionOutcome::Rejected(rejection) => {
            assert_eq!(rejection.reason, CompletionRejectReason::LeaseMismatch)
        }
        other => panic!("expected rejection, got {other:?}"),
    }
}

#[test]
fn test_gate_rejects_unowned_evidence_even_when_verified() {
    let snapshot = running_snapshot(CompletionPolicy::CandidateWithEvidence, None);
    // Evidence owned by a different goal fails ownership resolution.
    let records = [evidence_record("e-1", &coco_goals::GoalId::new("other"))];
    let outcome = GoalCompletionGate::evaluate(&snapshot, &candidate(), &records, verified());
    match outcome {
        CompletionOutcome::Rejected(rejection) => {
            assert_eq!(
                rejection.reason,
                CompletionRejectReason::EvidenceOwnershipFailed
            )
        }
        other => panic!("expected rejection, got {other:?}"),
    }
}
