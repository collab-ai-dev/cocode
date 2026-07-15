//! Completion policy, contract compilation, and the **sealed** completion
//! authority.
//!
//! The central safety property (§10.2, §12): `completed` cannot be minted by prose,
//! by the worker, or by any other crate. [`CompletionAuthorization`] has no public
//! constructor and is produced **only** by [`authorize_completion`], which runs the
//! full structural gate (identity, lease, plan observation, coverage, evidence
//! ownership) and requires a `Verified` verdict. The reducer accepts a completed
//! transition only when handed one of these tokens, and re-checks it against the
//! live snapshot.

use serde::{Deserialize, Serialize};

use crate::disposition::RequirementCoverage;
use crate::evidence::{EvidenceRef, GoalEvidenceRecord};
use crate::id::{ContentDigest, EvidenceId, GoalId, GoalLeaseId, SpecRevision};
use crate::plan::GoalPlanRef;
use crate::text::BoundedText;

/// How completion is judged for a goal, selected at creation and persisted (§12.3).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionPolicy {
    /// Default free-form path: structural validation plus one evidence-grounded
    /// review per candidate when there is no deterministic coverage.
    #[default]
    CandidateWithEvidence,
    /// All user-approved deterministic checks must pass; checks are also sufficient.
    ContractChecks,
    /// Deterministic checks, then one tool-capable semantic verifier.
    ContractChecksAndVerifier,
    /// Gate validates the candidate, then the goal awaits an explicit user accept.
    UserAcceptance,
}

/// Predicate a deterministic check evaluates. Executing it never involves a model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CheckKind {
    /// Run a command; the expectation judges its result.
    Command {
        command: BoundedText,
        expect: CheckExpectation,
    },
    /// Inspect file content.
    FileContent {
        path: BoundedText,
        expect: CheckExpectation,
    },
    /// Assert an artifact exists.
    Artifact { locator: BoundedText },
    /// A registered external-state predicate (CI/PR/service).
    ExternalState { description: BoundedText },
}

/// Expected outcome of a deterministic check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "expect", rename_all = "snake_case")]
pub enum CheckExpectation {
    /// The command/predicate succeeds (exit 0 / present).
    Success,
    /// Output/content contains the given text.
    Contains { text: BoundedText },
    /// Output/content equals the given text.
    Equals { text: BoundedText },
}

/// A command/file/artifact/external predicate with an expected result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeterministicCheck {
    pub description: BoundedText,
    pub kind: CheckKind,
}

/// A bounded natural-language requirement, satisfiable only by the verifier or an
/// explicit user acceptance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticCriterion {
    pub claim: BoundedText,
    /// Document section this claim is anchored to, when decomposed from a spec.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<BoundedText>,
}

/// One compiled contract item (§12.3). Compilation splits user text into typed
/// items once, at approval time; raw text is never interpreted at judgment time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "item", rename_all = "snake_case")]
pub enum ContractItem {
    Check(DeterministicCheck),
    Criterion(SemanticCriterion),
}

impl ContractItem {
    pub fn is_check(&self) -> bool {
        matches!(self, Self::Check(_))
    }

    pub fn is_criterion(&self) -> bool {
        matches!(self, Self::Criterion(_))
    }
}

/// A user-authored spec the contract references. Binds by digest, like the plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReferencedDoc {
    pub locator: BoundedText,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<ContentDigest>,
}

/// A user-approved completion contract. Authoritative only when the user supplied or
/// approved it (§9.3); a model-drafted plan can never silently redefine success.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionContract {
    pub items: Vec<ContractItem>,
    #[serde(default)]
    pub referenced_docs: Vec<ReferencedDoc>,
    /// Spec revision at which the user approved this contract.
    pub approved_at_spec: SpecRevision,
}

impl CompletionContract {
    pub fn has_checks(&self) -> bool {
        self.items.iter().any(ContractItem::is_check)
    }

    pub fn has_criteria(&self) -> bool {
        self.items.iter().any(ContractItem::is_criterion)
    }
}

/// Whether `policy` can actually judge `contract`. A checks-only policy with semantic
/// criteria would silently drop them, so goal creation must reject it (§12.3).
pub fn policy_can_judge_contract(policy: CompletionPolicy, contract: &CompletionContract) -> bool {
    if contract.has_criteria() {
        matches!(
            policy,
            CompletionPolicy::ContractChecksAndVerifier | CompletionPolicy::UserAcceptance
        )
    } else {
        true
    }
}

/// Bounded per-requirement audit report produced on successful completion (§12.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionEvidenceSummary {
    pub summary: BoundedText,
    #[serde(default)]
    pub verified_requirements: Vec<BoundedText>,
    #[serde(default)]
    pub cited_evidence: Vec<EvidenceId>,
}

/// Why a completion candidate was rejected. The typed detail lets the next reminder
/// name exactly which item failed (§12.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionRejectReason {
    StaleIdentity,
    SpecMismatch,
    LeaseMismatch,
    PlanDrift,
    CoverageIncomplete,
    EvidenceOwnershipFailed,
    CheckFailed,
    CriterionFailed,
    VerifierRejected,
    NewerGuidance,
}

/// Typed rejection carried back into `active` so the worker addresses the gap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionRejection {
    pub reason: CompletionRejectReason,
    pub detail: BoundedText,
    #[serde(default)]
    pub failed_items: Vec<BoundedText>,
}

impl CompletionRejection {
    pub fn new(reason: CompletionRejectReason, detail: impl AsRef<str>) -> Self {
        Self {
            reason,
            detail: BoundedText::short(detail),
            failed_items: Vec::new(),
        }
    }
}

/// Where a completion candidate came from (§12.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateSource {
    /// The worker submitted a `CompletionCandidate` report.
    WorkerReport,
    /// Deterministic contract checks whose evidence changed this turn.
    DeterministicContract,
    /// A mandatory boundary audit before a stop transition.
    BoundaryAudit,
    /// A probe-escalated audit after an unanswered `LikelyComplete` nudge.
    ProbeAudit,
}

/// The candidate the gate validates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionCandidate {
    pub source: CandidateSource,
    pub coverage: RequirementCoverage,
    #[serde(default)]
    pub evidence: Vec<EvidenceRef>,
    /// Plan revision/digest the worker observed, compared against current for drift.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_observed: Option<GoalPlanRef>,
}

/// Verdict from the host's policy execution (deterministic checks, evidence review,
/// or contract verifier). The gate owns the transition, not the verifier (§12.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerificationOutcome {
    Verified { summary: CompletionEvidenceSummary },
    Rejected(CompletionRejection),
    Unavailable,
}

/// Outcome of the sealed authorization path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionOutcome {
    /// Structure and verification passed: an authorization capability was minted.
    Authorized(CompletionAuthorization),
    /// Structure or verification failed: return to `active` with detail.
    Rejected(CompletionRejection),
    /// Required verification could not run.
    Unavailable,
}

/// **Sealed** proof that a completion candidate passed the full gate. Constructed
/// only by [`authorize_completion`] (private `seal`), it is a non-serializable
/// in-memory capability: it is never persisted or deserialized, so it cannot be
/// forged by round-tripping JSON. The reducer requires one to persist `completed`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionAuthorization {
    goal_id: GoalId,
    spec_revision: SpecRevision,
    lease_id: GoalLeaseId,
    evidence_summary: CompletionEvidenceSummary,
}

impl CompletionAuthorization {
    pub fn goal_id(&self) -> &GoalId {
        &self.goal_id
    }

    pub fn spec_revision(&self) -> SpecRevision {
        self.spec_revision
    }

    pub fn lease_id(&self) -> &GoalLeaseId {
        &self.lease_id
    }

    pub fn evidence_summary(&self) -> &CompletionEvidenceSummary {
        &self.evidence_summary
    }

    pub fn into_evidence_summary(self) -> CompletionEvidenceSummary {
        self.evidence_summary
    }
}

/// Private mint. Only reachable from within this module, so [`authorize_completion`]
/// is the sole producer of the capability token.
fn seal(
    goal_id: GoalId,
    spec_revision: SpecRevision,
    lease_id: GoalLeaseId,
    evidence_summary: CompletionEvidenceSummary,
) -> CompletionAuthorization {
    CompletionAuthorization {
        goal_id,
        spec_revision,
        lease_id,
        evidence_summary,
    }
}

/// Pure structural validation shared by candidate parking and final authorization.
/// Validates coverage, plan-drift, and evidence ownership against the live goal.
/// Returns the derived evidence summary; never a capability.
pub fn precheck_candidate(
    goal_id: &GoalId,
    current_plan: Option<&GoalPlanRef>,
    candidate: &CompletionCandidate,
    resolved_evidence: &[GoalEvidenceRecord],
) -> Result<CompletionEvidenceSummary, CompletionRejection> {
    if !candidate.coverage.all_satisfied() {
        return Err(CompletionRejection::new(
            CompletionRejectReason::CoverageIncomplete,
            "not every requirement is satisfied or completion is not asserted",
        ));
    }

    // A stale plan excerpt fails the digest comparison (§12.2).
    if let (Some(current), Some(observed)) = (current_plan, candidate.plan_observed.as_ref())
        && current.drifted_from(observed)
    {
        return Err(CompletionRejection::new(
            CompletionRejectReason::PlanDrift,
            "plan artifact changed since the observed revision/digest",
        ));
    }

    // Every cited reference must resolve to a durable record owned by this goal.
    let mut cited = Vec::new();
    for reference in &candidate.evidence {
        let owned = resolved_evidence
            .iter()
            .find(|record| record.evidence_id == reference.evidence_id)
            .is_some_and(|record| record.owned_by(goal_id));
        if !owned {
            let mut rejection = CompletionRejection::new(
                CompletionRejectReason::EvidenceOwnershipFailed,
                "cited evidence does not resolve to a record produced under this goal",
            );
            rejection
                .failed_items
                .push(BoundedText::short(reference.evidence_id.as_str()));
            return Err(rejection);
        }
        cited.push(reference.evidence_id.clone());
    }

    let verified_requirements = candidate
        .coverage
        .requirements
        .iter()
        .map(|r| r.requirement.clone())
        .collect();

    Ok(CompletionEvidenceSummary {
        summary: BoundedText::short("all requirements satisfied with owned evidence"),
        verified_requirements,
        cited_evidence: cited,
    })
}

/// The sole sealed authorization path. Runs the full structural gate, then applies
/// the host's verification verdict. On success it mints the capability from a fresh
/// summary derived here (never from caller-supplied data), keeping the seal intact.
pub fn authorize_completion(
    goal_id: &GoalId,
    spec_revision: SpecRevision,
    running_lease: &GoalLeaseId,
    current_plan: Option<&GoalPlanRef>,
    candidate: &CompletionCandidate,
    resolved_evidence: &[GoalEvidenceRecord],
    verification: VerificationOutcome,
) -> CompletionOutcome {
    let structural = match precheck_candidate(goal_id, current_plan, candidate, resolved_evidence) {
        Ok(summary) => summary,
        Err(rejection) => return CompletionOutcome::Rejected(rejection),
    };

    match verification {
        VerificationOutcome::Verified { summary } => {
            // Prefer the verifier's per-claim audit report; fall back to the
            // structurally-derived summary.
            let evidence_summary = if summary.summary.is_empty() {
                structural
            } else {
                summary
            };
            CompletionOutcome::Authorized(seal(
                goal_id.clone(),
                spec_revision,
                running_lease.clone(),
                evidence_summary,
            ))
        }
        VerificationOutcome::Rejected(rejection) => CompletionOutcome::Rejected(rejection),
        VerificationOutcome::Unavailable => CompletionOutcome::Unavailable,
    }
}

/// Steering-only verdict from the bounded periodic completion probe (§12.5). It has
/// no terminal authority; its failure is a safe no-op.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum ProbeVerdict {
    LikelyComplete { rationale: BoundedText },
    OnTrack,
    Circling { rationale: BoundedText },
}

#[cfg(test)]
#[path = "completion.test.rs"]
mod tests;
