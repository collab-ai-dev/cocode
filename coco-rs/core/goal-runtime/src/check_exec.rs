//! Deterministic contract-check execution (§12.3).
//!
//! Closes the "gate can pass on vibes" hole: the boundary audit and worker
//! completion candidates previously reached the gate with every requirement
//! marked satisfied while [`coco_goals::CheckKind`] predicates were never
//! run. [`DeterministicCheckVerifier`] is a [`CompletionVerifier`] that
//! executes the contract's deterministic checks for real through the
//! host-provided [`CheckExecutor`] seam (this crate does no I/O itself) and
//! fails closed: any check that cannot run counts as unsatisfied.

use std::sync::Arc;

use async_trait::async_trait;
use coco_goals::{
    BoundedText, CheckExpectation, CheckKind, CompletionEvidenceSummary, CompletionPolicy,
    CompletionRejectReason, CompletionRejection, ContractItem, DeterministicCheck,
    VerificationOutcome,
};

use crate::verifier::{CompletionVerifier, VerificationRequest};

/// What a command execution observed. `output` is stdout+stderr combined,
/// bounded by the host.
pub struct CommandObservation {
    pub exit_success: bool,
    pub output: String,
}

/// Host-side execution seam for deterministic checks. The production impl
/// (process spawn, filesystem) lives in the session runtime; errors are
/// plain strings because every failure maps to the same fail-closed
/// "check did not pass" outcome.
#[async_trait]
pub trait CheckExecutor: Send + Sync {
    async fn run_command(&self, command: &str) -> Result<CommandObservation, String>;
    async fn read_file(&self, path: &str) -> Result<String, String>;
    async fn artifact_exists(&self, locator: &str) -> bool;
}

/// Executes deterministic contract checks; delegates everything it cannot
/// judge deterministically to `fallback`.
///
/// - No deterministic checks in the contract → `fallback.verify(request)`
///   unchanged (free-form goals keep their existing policy path).
/// - Any check fails (or cannot run) → `Rejected` naming the failing
///   checks.
/// - All checks pass under `ContractChecksAndVerifier` with semantic
///   criteria present → the semantic half runs via `fallback`.
/// - All checks pass otherwise → `Verified` with per-check evidence.
pub struct DeterministicCheckVerifier {
    executor: Arc<dyn CheckExecutor>,
    fallback: Arc<dyn CompletionVerifier>,
}

impl DeterministicCheckVerifier {
    pub fn new(executor: Arc<dyn CheckExecutor>, fallback: Arc<dyn CompletionVerifier>) -> Self {
        Self { executor, fallback }
    }

    async fn execute_check(&self, check: &DeterministicCheck) -> CheckResult {
        let (satisfied, detail) = match &check.kind {
            CheckKind::Command { command, expect } => {
                match self.executor.run_command(command.as_str()).await {
                    Ok(observation) => match expect {
                        CheckExpectation::Success => (
                            observation.exit_success,
                            format!(
                                "command `{}` {}",
                                command.as_str(),
                                if observation.exit_success {
                                    "succeeded"
                                } else {
                                    "exited non-zero"
                                }
                            ),
                        ),
                        CheckExpectation::Contains { text } => (
                            observation.output.contains(text.as_str()),
                            format!("command `{}` output contains check", command.as_str()),
                        ),
                        CheckExpectation::Equals { text } => (
                            observation.output.trim() == text.as_str(),
                            format!("command `{}` output equality check", command.as_str()),
                        ),
                    },
                    Err(error) => (
                        false,
                        format!("command `{}` failed to run: {error}", command.as_str()),
                    ),
                }
            }
            CheckKind::FileContent { path, expect } => {
                match self.executor.read_file(path.as_str()).await {
                    Ok(content) => match expect {
                        CheckExpectation::Success => {
                            (true, format!("file `{}` readable", path.as_str()))
                        }
                        CheckExpectation::Contains { text } => (
                            content.contains(text.as_str()),
                            format!("file `{}` content contains check", path.as_str()),
                        ),
                        CheckExpectation::Equals { text } => (
                            content.trim() == text.as_str(),
                            format!("file `{}` content equality check", path.as_str()),
                        ),
                    },
                    Err(error) => (
                        false,
                        format!("file `{}` could not be read: {error}", path.as_str()),
                    ),
                }
            }
            CheckKind::Artifact { locator } => (
                self.executor.artifact_exists(locator.as_str()).await,
                format!("artifact `{}` existence check", locator.as_str()),
            ),
            // No registered external-state predicates exist yet; fail
            // closed rather than pass on an unexecuted claim.
            CheckKind::ExternalState { description } => (
                false,
                format!(
                    "external-state check `{}` is not executable yet",
                    description.as_str()
                ),
            ),
        };
        CheckResult {
            description: check.description.as_str().to_string(),
            satisfied,
            detail,
        }
    }
}

struct CheckResult {
    description: String,
    satisfied: bool,
    detail: String,
}

#[async_trait]
impl CompletionVerifier for DeterministicCheckVerifier {
    async fn verify(&self, request: VerificationRequest) -> VerificationOutcome {
        let checks: Vec<DeterministicCheck> = request
            .contract
            .as_ref()
            .map(|contract| {
                contract
                    .items
                    .iter()
                    .filter_map(|item| match item {
                        ContractItem::Check(check) => Some(check.clone()),
                        ContractItem::Criterion(_) => None,
                    })
                    .collect()
            })
            .unwrap_or_default();

        if checks.is_empty() {
            return self.fallback.verify(request).await;
        }

        let mut verified_requirements = Vec::new();
        let mut failures = Vec::new();
        for check in &checks {
            let result = self.execute_check(check).await;
            if result.satisfied {
                verified_requirements.push(BoundedText::short(&result.description));
            } else {
                failures.push(format!("{} ({})", result.description, result.detail));
            }
        }

        if !failures.is_empty() {
            return VerificationOutcome::Rejected(CompletionRejection::new(
                CompletionRejectReason::VerifierRejected,
                format!("deterministic checks failed: {}", failures.join("; ")),
            ));
        }

        // The semantic half (criteria) still needs the fallback verifier
        // under the combined policy; checks alone cannot prove claims.
        let has_criteria = request
            .contract
            .as_ref()
            .is_some_and(coco_goals::CompletionContract::has_criteria);
        if request.policy == CompletionPolicy::ContractChecksAndVerifier && has_criteria {
            return self.fallback.verify(request).await;
        }

        VerificationOutcome::Verified {
            summary: CompletionEvidenceSummary {
                summary: BoundedText::short(format!(
                    "{} deterministic check(s) executed and passed",
                    checks.len()
                )),
                verified_requirements,
                cited_evidence: Vec::new(),
            },
        }
    }
}

#[cfg(test)]
#[path = "check_exec.test.rs"]
mod tests;
