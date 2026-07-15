use super::*;
use pretty_assertions::assert_eq;

fn requirement(satisfied: bool) -> RequirementResult {
    RequirementResult {
        requirement: BoundedText::short("req"),
        satisfied,
        evidence: Vec::new(),
    }
}

#[test]
fn test_coverage_all_satisfied_requires_assertion_and_every_requirement() {
    let ok = RequirementCoverage {
        requirements: vec![requirement(true), requirement(true)],
        asserts_complete: true,
    };
    assert!(ok.all_satisfied());

    let missing_assert = RequirementCoverage {
        requirements: vec![requirement(true)],
        asserts_complete: false,
    };
    assert!(!missing_assert.all_satisfied());

    let one_unsatisfied = RequirementCoverage {
        requirements: vec![requirement(true), requirement(false)],
        asserts_complete: true,
    };
    assert!(!one_unsatisfied.all_satisfied());
}

#[test]
fn test_disposition_report_flags() {
    let progress = GoalTurnDisposition::Progress {
        summary: BoundedText::short("s"),
        next_step: BoundedText::short("n"),
        evidence: Vec::new(),
    };
    assert!(progress.is_reported());
    assert!(!progress.is_completion_candidate());
    assert!(!GoalTurnDisposition::Unreported.is_reported());

    let candidate = GoalTurnDisposition::CompletionCandidate {
        coverage: RequirementCoverage {
            requirements: Vec::new(),
            asserts_complete: true,
        },
        evidence: Vec::new(),
    };
    assert!(candidate.is_completion_candidate());
}

#[test]
fn test_wait_condition_tagged_serde() {
    let condition = WaitCondition::ModeGate {
        mode: ModeGate::Plan,
    };
    let json = serde_json::to_value(&condition).unwrap();
    assert_eq!(json["kind"], "mode_gate");
    assert_eq!(json["mode"], "plan");
    let back: WaitCondition = serde_json::from_value(json).unwrap();
    assert_eq!(back, condition);
}

#[test]
fn test_progress_signal_snake_case() {
    let json = serde_json::to_value(ProgressSignal::WorkspaceChange).unwrap();
    assert_eq!(json, "workspace_change");
}
