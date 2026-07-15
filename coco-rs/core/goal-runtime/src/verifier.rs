//! Completion-policy execution seam (§12.3).
//!
//! The verifier runs the effective policy — deterministic contract checks, an
//! evidence-grounded review, or a tool-capable semantic verifier — and returns a
//! verdict. It never owns the transition; the gate does. The production impl
//! (tool-capable, model-backed) lives in the session runtime; this crate defines
//! the trait plus deterministic doubles.

use async_trait::async_trait;
use coco_goals::{
    BoundedText, CandidateSource, CompletionCandidate, CompletionContract,
    CompletionEvidenceSummary, CompletionPolicy, CompletionRejectReason, CompletionRejection,
    GoalId, SpecRevision, VerificationAttemptId, VerificationOutcome,
};

/// Everything a verifier needs to judge one candidate.
pub struct VerificationRequest {
    pub goal_id: GoalId,
    pub spec_revision: SpecRevision,
    pub objective: String,
    pub contract: Option<CompletionContract>,
    pub policy: CompletionPolicy,
    pub source: CandidateSource,
    pub candidate: CompletionCandidate,
    /// Durable attempt id so a crash after the call but before persistence can
    /// safely retry (§10.2).
    pub attempt: VerificationAttemptId,
}

/// Runs the effective completion policy for a candidate.
#[async_trait]
pub trait CompletionVerifier: Send + Sync {
    async fn verify(&self, request: VerificationRequest) -> VerificationOutcome;
}

/// Approves every candidate — the double for a policy whose checks all pass, or
/// for tests exercising the completed path.
pub struct AlwaysVerified;

#[async_trait]
impl CompletionVerifier for AlwaysVerified {
    async fn verify(&self, _request: VerificationRequest) -> VerificationOutcome {
        VerificationOutcome::Verified {
            summary: CompletionEvidenceSummary {
                summary: BoundedText::short("verified"),
                verified_requirements: Vec::new(),
                cited_evidence: Vec::new(),
            },
        }
    }
}

/// Rejects every candidate.
pub struct AlwaysRejected;

#[async_trait]
impl CompletionVerifier for AlwaysRejected {
    async fn verify(&self, _request: VerificationRequest) -> VerificationOutcome {
        VerificationOutcome::Rejected(CompletionRejection::new(
            CompletionRejectReason::VerifierRejected,
            "verifier rejected the candidate",
        ))
    }
}

/// Reports the verifier could not run.
pub struct AlwaysUnavailable;

#[async_trait]
impl CompletionVerifier for AlwaysUnavailable {
    async fn verify(&self, _request: VerificationRequest) -> VerificationOutcome {
        VerificationOutcome::Unavailable
    }
}
