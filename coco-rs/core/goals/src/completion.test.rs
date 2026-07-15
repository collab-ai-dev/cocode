use crate::*;
use coco_types::TurnId;
use pretty_assertions::assert_eq;

fn goal() -> GoalId {
    GoalId::new("g-1")
}

fn lease() -> GoalLeaseId {
    GoalLeaseId::new("l-1")
}

fn evidence_ref(id: &str) -> EvidenceRef {
    EvidenceRef {
        evidence_id: EvidenceId::new(id),
        summary: BoundedText::short("ran the test"),
    }
}

fn owned_record(id: &str, goal: &GoalId) -> GoalEvidenceRecord {
    GoalEvidenceRecord {
        evidence_id: EvidenceId::new(id),
        goal_id: goal.clone(),
        lease_id: lease(),
        turn_id: TurnId::new("t-1"),
        source: EvidenceSource::ToolResult {
            tool: "Bash".to_string(),
        },
        result_ref: DurableResultRef::new("loc"),
        content_digest: None,
        observed_at: Timestamp::from_millis(0),
    }
}

fn candidate(evidence: Vec<EvidenceRef>, asserts_complete: bool) -> CompletionCandidate {
    CompletionCandidate {
        source: CandidateSource::WorkerReport,
        coverage: RequirementCoverage {
            requirements: vec![RequirementResult {
                requirement: BoundedText::short("build passes"),
                satisfied: true,
                evidence: Vec::new(),
            }],
            asserts_complete,
        },
        evidence,
        plan_observed: None,
    }
}

#[test]
fn test_precheck_rejects_incomplete_coverage() {
    let result = precheck_candidate(&goal(), None, &candidate(Vec::new(), false), &[]);
    assert_eq!(
        result.unwrap_err().reason,
        CompletionRejectReason::CoverageIncomplete
    );
}

#[test]
fn test_precheck_rejects_unowned_evidence() {
    let cand = candidate(vec![evidence_ref("e-1")], true);
    // No resolved records at all.
    let result = precheck_candidate(&goal(), None, &cand, &[]);
    assert_eq!(
        result.unwrap_err().reason,
        CompletionRejectReason::EvidenceOwnershipFailed
    );
}

#[test]
fn test_precheck_rejects_evidence_owned_by_other_goal() {
    let cand = candidate(vec![evidence_ref("e-1")], true);
    let foreign = owned_record("e-1", &GoalId::new("other"));
    let result = precheck_candidate(&goal(), None, &cand, &[foreign]);
    assert_eq!(
        result.unwrap_err().reason,
        CompletionRejectReason::EvidenceOwnershipFailed
    );
}

#[test]
fn test_precheck_passes_with_owned_evidence() {
    let cand = candidate(vec![evidence_ref("e-1")], true);
    let record = owned_record("e-1", &goal());
    let summary = precheck_candidate(&goal(), None, &cand, &[record]).unwrap();
    assert_eq!(summary.cited_evidence, vec![EvidenceId::new("e-1")]);
}

#[test]
fn test_authorize_verified_mints_authorization() {
    let cand = candidate(Vec::new(), true);
    let outcome = authorize_completion(
        &goal(),
        SpecRevision::INITIAL,
        &lease(),
        None,
        &cand,
        &[],
        VerificationOutcome::Verified {
            summary: CompletionEvidenceSummary {
                summary: BoundedText::short("all good"),
                verified_requirements: Vec::new(),
                cited_evidence: Vec::new(),
            },
        },
    );
    match outcome {
        CompletionOutcome::Authorized(auth) => {
            assert_eq!(auth.goal_id(), &goal());
            assert_eq!(auth.spec_revision(), SpecRevision::INITIAL);
            assert_eq!(auth.lease_id(), &lease());
        }
        other => panic!("expected authorized, got {other:?}"),
    }
}

#[test]
fn test_authorize_rejected_verdict_passes_through() {
    let cand = candidate(Vec::new(), true);
    let outcome = authorize_completion(
        &goal(),
        SpecRevision::INITIAL,
        &lease(),
        None,
        &cand,
        &[],
        VerificationOutcome::Rejected(CompletionRejection::new(
            CompletionRejectReason::VerifierRejected,
            "not actually done",
        )),
    );
    assert!(matches!(outcome, CompletionOutcome::Rejected(_)));
}

#[test]
fn test_authorize_structural_failure_short_circuits_verdict() {
    // Coverage is incomplete, so even a Verified verdict cannot authorize.
    let cand = candidate(Vec::new(), false);
    let outcome = authorize_completion(
        &goal(),
        SpecRevision::INITIAL,
        &lease(),
        None,
        &cand,
        &[],
        VerificationOutcome::Verified {
            summary: CompletionEvidenceSummary {
                summary: BoundedText::short("x"),
                verified_requirements: Vec::new(),
                cited_evidence: Vec::new(),
            },
        },
    );
    assert!(matches!(outcome, CompletionOutcome::Rejected(_)));
}

#[test]
fn test_authorize_unavailable_verifier() {
    let cand = candidate(Vec::new(), true);
    let outcome = authorize_completion(
        &goal(),
        SpecRevision::INITIAL,
        &lease(),
        None,
        &cand,
        &[],
        VerificationOutcome::Unavailable,
    );
    assert!(matches!(outcome, CompletionOutcome::Unavailable));
}

#[test]
fn test_policy_can_judge_contract() {
    let criterion_contract = CompletionContract {
        items: vec![ContractItem::Criterion(SemanticCriterion {
            claim: BoundedText::short("matches design"),
            anchor: None,
        })],
        referenced_docs: Vec::new(),
        approved_at_spec: SpecRevision::INITIAL,
    };
    assert!(!policy_can_judge_contract(
        CompletionPolicy::ContractChecks,
        &criterion_contract
    ));
    assert!(policy_can_judge_contract(
        CompletionPolicy::ContractChecksAndVerifier,
        &criterion_contract
    ));
    assert!(policy_can_judge_contract(
        CompletionPolicy::UserAcceptance,
        &criterion_contract
    ));

    let checks_only = CompletionContract {
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
    assert!(policy_can_judge_contract(
        CompletionPolicy::ContractChecks,
        &checks_only
    ));
}

#[test]
fn test_probe_verdict_serde() {
    let verdict = ProbeVerdict::LikelyComplete {
        rationale: BoundedText::short("all requirements appear met"),
    };
    let json = serde_json::to_value(&verdict).unwrap();
    assert_eq!(json["verdict"], "likely_complete");
    let back: ProbeVerdict = serde_json::from_value(json).unwrap();
    assert_eq!(back, verdict);
}
