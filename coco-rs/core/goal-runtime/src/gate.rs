//! `GoalCompletionGate` — the only component that may request a `completed`
//! transition (§10.2, §12).
//!
//! It is a thin host wrapper over the domain's sealed [`authorize_completion`]:
//! it locates the running lease and forwards identity, coverage, plan, evidence,
//! and the verification verdict. The capability token can only be minted inside
//! the domain crate, so no host code (here or elsewhere) can fabricate a
//! completion.

use coco_goals::{
    CompletionCandidate, CompletionOutcome, CompletionRejectReason, CompletionRejection,
    GoalEvidenceRecord, GoalSnapshot, VerificationOutcome, authorize_completion,
};

/// Stateless completion authority.
pub struct GoalCompletionGate;

impl GoalCompletionGate {
    /// Validate a candidate against the live goal and the verification verdict.
    /// Requires a running goal turn; otherwise the candidate is rejected.
    pub fn evaluate(
        snapshot: &GoalSnapshot,
        candidate: &CompletionCandidate,
        resolved_evidence: &[GoalEvidenceRecord],
        verification: VerificationOutcome,
    ) -> CompletionOutcome {
        let Some(running_lease) = snapshot.lifecycle.running_lease_id() else {
            return CompletionOutcome::Rejected(CompletionRejection::new(
                CompletionRejectReason::LeaseMismatch,
                "completion requires a running goal turn",
            ));
        };
        authorize_completion(
            &snapshot.goal_id,
            snapshot.spec_revision,
            running_lease,
            snapshot.plan.as_ref(),
            candidate,
            resolved_evidence,
            verification,
        )
    }
}

#[cfg(test)]
#[path = "gate.test.rs"]
mod tests;
