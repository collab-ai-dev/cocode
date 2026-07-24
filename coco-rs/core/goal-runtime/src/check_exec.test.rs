use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use coco_goals::{
    BoundedText, CandidateSource, CheckExpectation, CheckKind, CompletionCandidate,
    CompletionContract, CompletionPolicy, ContractItem, DeterministicCheck, SemanticCriterion,
    SpecRevision, VerificationAttemptId, VerificationOutcome,
};
use pretty_assertions::assert_eq;

use super::*;
use crate::test_support::{goal_id, satisfied_coverage};
use crate::verifier::{AlwaysUnavailable, AlwaysVerified};

/// Scripted executor double: commands/files/artifacts resolve from maps.
#[derive(Default)]
struct ScriptedExecutor {
    commands: HashMap<String, (bool, String)>,
    files: HashMap<String, String>,
    artifacts: HashSet<String>,
}

#[async_trait::async_trait]
impl CheckExecutor for ScriptedExecutor {
    async fn run_command(&self, command: &str) -> Result<CommandObservation, String> {
        self.commands
            .get(command)
            .map(|(exit_success, output)| CommandObservation {
                exit_success: *exit_success,
                output: output.clone(),
            })
            .ok_or_else(|| "spawn failed".to_string())
    }

    async fn read_file(&self, path: &str) -> Result<String, String> {
        self.files
            .get(path)
            .cloned()
            .ok_or_else(|| "no such file".to_string())
    }

    async fn artifact_exists(&self, locator: &str) -> bool {
        self.artifacts.contains(locator)
    }
}

fn command_check(command: &str, expect: CheckExpectation) -> ContractItem {
    ContractItem::Check(DeterministicCheck {
        description: BoundedText::short(format!("run {command}")),
        kind: CheckKind::Command {
            command: BoundedText::short(command),
            expect,
        },
    })
}

fn contract(items: Vec<ContractItem>) -> CompletionContract {
    CompletionContract {
        items,
        referenced_docs: Vec::new(),
        approved_at_spec: SpecRevision::INITIAL,
    }
}

fn request(contract: Option<CompletionContract>, policy: CompletionPolicy) -> VerificationRequest {
    VerificationRequest {
        goal_id: goal_id(),
        spec_revision: SpecRevision::INITIAL,
        objective: "ship it".to_string(),
        contract,
        policy,
        source: CandidateSource::WorkerReport,
        candidate: CompletionCandidate {
            source: CandidateSource::WorkerReport,
            coverage: satisfied_coverage(),
            evidence: Vec::new(),
            plan_observed: None,
        },
        attempt: VerificationAttemptId::new("va-1"),
    }
}

#[tokio::test]
async fn passing_command_check_verifies() {
    let mut executor = ScriptedExecutor::default();
    executor
        .commands
        .insert("just test".into(), (true, "ok".into()));
    let verifier = DeterministicCheckVerifier::new(Arc::new(executor), Arc::new(AlwaysUnavailable));
    let outcome = verifier
        .verify(request(
            Some(contract(vec![command_check(
                "just test",
                CheckExpectation::Success,
            )])),
            CompletionPolicy::ContractChecks,
        ))
        .await;
    let VerificationOutcome::Verified { summary } = outcome else {
        panic!("expected Verified, got a non-verified outcome");
    };
    assert_eq!(summary.verified_requirements.len(), 1);
}

#[tokio::test]
async fn failing_command_check_rejects_with_detail() {
    let mut executor = ScriptedExecutor::default();
    executor
        .commands
        .insert("just test".into(), (false, "3 failures".into()));
    let verifier = DeterministicCheckVerifier::new(Arc::new(executor), Arc::new(AlwaysVerified));
    let outcome = verifier
        .verify(request(
            Some(contract(vec![command_check(
                "just test",
                CheckExpectation::Success,
            )])),
            CompletionPolicy::ContractChecks,
        ))
        .await;
    let VerificationOutcome::Rejected(rejection) = outcome else {
        panic!("expected Rejected");
    };
    assert!(rejection.detail.as_str().contains("run just test"));
}

#[tokio::test]
async fn unrunnable_check_fails_closed() {
    // No scripted entry → run_command errors → the check must count as
    // unsatisfied, never as passed.
    let verifier = DeterministicCheckVerifier::new(
        Arc::new(ScriptedExecutor::default()),
        Arc::new(AlwaysVerified),
    );
    let outcome = verifier
        .verify(request(
            Some(contract(vec![command_check(
                "missing",
                CheckExpectation::Success,
            )])),
            CompletionPolicy::ContractChecks,
        ))
        .await;
    assert!(matches!(outcome, VerificationOutcome::Rejected(_)));
}

#[tokio::test]
async fn contains_and_equals_expectations_judge_output() {
    let mut executor = ScriptedExecutor::default();
    executor
        .commands
        .insert("cat version".into(), (true, " 1.2.3 ".into()));
    let verifier = DeterministicCheckVerifier::new(Arc::new(executor), Arc::new(AlwaysUnavailable));

    let ok = verifier
        .verify(request(
            Some(contract(vec![
                command_check(
                    "cat version",
                    CheckExpectation::Contains {
                        text: BoundedText::short("1.2"),
                    },
                ),
                command_check(
                    "cat version",
                    CheckExpectation::Equals {
                        text: BoundedText::short("1.2.3"),
                    },
                ),
            ])),
            CompletionPolicy::ContractChecks,
        ))
        .await;
    assert!(matches!(ok, VerificationOutcome::Verified { .. }));

    let miss = verifier
        .verify(request(
            Some(contract(vec![command_check(
                "cat version",
                CheckExpectation::Equals {
                    text: BoundedText::short("2.0.0"),
                },
            )])),
            CompletionPolicy::ContractChecks,
        ))
        .await;
    assert!(matches!(miss, VerificationOutcome::Rejected(_)));
}

#[tokio::test]
async fn file_and_artifact_checks_execute() {
    let mut executor = ScriptedExecutor::default();
    executor
        .files
        .insert("CHANGELOG.md".into(), "## v2 shipped".into());
    executor.artifacts.insert("dist/app.tar.gz".into());
    let verifier = DeterministicCheckVerifier::new(Arc::new(executor), Arc::new(AlwaysUnavailable));
    let outcome = verifier
        .verify(request(
            Some(contract(vec![
                ContractItem::Check(DeterministicCheck {
                    description: BoundedText::short("changelog mentions v2"),
                    kind: CheckKind::FileContent {
                        path: BoundedText::short("CHANGELOG.md"),
                        expect: CheckExpectation::Contains {
                            text: BoundedText::short("v2 shipped"),
                        },
                    },
                }),
                ContractItem::Check(DeterministicCheck {
                    description: BoundedText::short("tarball exists"),
                    kind: CheckKind::Artifact {
                        locator: BoundedText::short("dist/app.tar.gz"),
                    },
                }),
            ])),
            CompletionPolicy::ContractChecks,
        ))
        .await;
    assert!(matches!(outcome, VerificationOutcome::Verified { .. }));
}

#[tokio::test]
async fn external_state_check_fails_closed() {
    let verifier = DeterministicCheckVerifier::new(
        Arc::new(ScriptedExecutor::default()),
        Arc::new(AlwaysVerified),
    );
    let outcome = verifier
        .verify(request(
            Some(contract(vec![ContractItem::Check(DeterministicCheck {
                description: BoundedText::short("CI green"),
                kind: CheckKind::ExternalState {
                    description: BoundedText::short("CI pipeline passed"),
                },
            })])),
            CompletionPolicy::ContractChecks,
        ))
        .await;
    assert!(matches!(outcome, VerificationOutcome::Rejected(_)));
}

#[tokio::test]
async fn no_checks_delegates_to_fallback_unchanged() {
    let verifier = DeterministicCheckVerifier::new(
        Arc::new(ScriptedExecutor::default()),
        Arc::new(AlwaysUnavailable),
    );
    let outcome = verifier
        .verify(request(None, CompletionPolicy::CandidateWithEvidence))
        .await;
    assert!(matches!(outcome, VerificationOutcome::Unavailable));
}

#[tokio::test]
async fn passing_checks_with_criteria_run_semantic_fallback() {
    let mut executor = ScriptedExecutor::default();
    executor.commands.insert("true".into(), (true, "".into()));
    let verifier = DeterministicCheckVerifier::new(Arc::new(executor), Arc::new(AlwaysUnavailable));
    let outcome = verifier
        .verify(request(
            Some(contract(vec![
                command_check("true", CheckExpectation::Success),
                ContractItem::Criterion(SemanticCriterion {
                    claim: BoundedText::short("docs read well"),
                    anchor: None,
                }),
            ])),
            CompletionPolicy::ContractChecksAndVerifier,
        ))
        .await;
    // Checks passed but the semantic half has no verifier yet → the
    // fallback's Unavailable wins (fail closed, never fake-verified).
    assert!(matches!(outcome, VerificationOutcome::Unavailable));
}
