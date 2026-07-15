//! `GoalContextMaterializer` — bounded typed context for a goal-owned turn (§5.5,
//! §10.2).
//!
//! Before every goal-owned turn the runtime re-materializes the authoritative
//! objective, budget, plan reference, progress, and wait resolution from durable
//! state — never trusting compaction to preserve them. The result is a *typed*
//! value, not a pre-authorized string: the reminder adapter escapes the untrusted
//! goal fields and renders static runtime instructions separately, so an
//! objective or plan cannot gain system authority (prompt-injection safety).

use std::sync::Arc;

use coco_goals::{
    CompletionContract, ContentDigest, GoalLeaseId, GoalPlanRef, GoalSnapshot, PlanArtifactId,
    PlanRevision, ProgressCheckpoint, SpecRevision, StateVersion, WaitResolution,
};

use crate::error::{GoalRuntimeError, Result};

/// Bounded budget projection for the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalBudgetView {
    pub autonomous_turns_used: u32,
    pub autonomous_turns_max: u32,
    pub total_turns: u32,
    pub tokens_used: u64,
    pub tokens_max: Option<u64>,
}

/// Bounded plan projection: path/digest/active steps, never the full body (§5.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalPlanView {
    pub artifact_id: PlanArtifactId,
    pub revision: PlanRevision,
    pub display_path: String,
    pub digest: Option<ContentDigest>,
    /// Bounded plan headings.
    pub headings: Vec<String>,
    /// Bounded active/unchecked steps.
    pub active_steps: Vec<String>,
    /// The file digest changed outside the current worker turn.
    pub drifted: bool,
}

/// The bounded, typed goal context injected before a goal-owned turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalTurnContext {
    pub goal_id: coco_goals::GoalId,
    pub spec_revision: SpecRevision,
    pub state_version: StateVersion,
    pub lease_id: GoalLeaseId,
    /// Untrusted user-authored objective text (escaped by the reminder adapter).
    pub objective: String,
    pub budget: GoalBudgetView,
    pub plan: Option<GoalPlanView>,
    pub progress: Option<ProgressCheckpoint>,
    pub wait_resolution: Option<WaitResolution>,
    pub completion_contract: Option<CompletionContract>,
}

/// Resolves a goal's plan reference to a bounded view. The concrete
/// `PlanArtifactService` over the session plan file lives in the session runtime;
/// this seam keeps the materializer testable and free of filesystem access.
pub trait PlanSource: Send + Sync {
    /// Resolve `plan_ref` to a bounded view, or `None` if the artifact is missing.
    fn plan_view(&self, plan_ref: &GoalPlanRef) -> Result<Option<GoalPlanView>>;
}

/// A [`PlanSource`] that resolves nothing — for goals with no plan binding.
pub struct NoPlanSource;

impl PlanSource for NoPlanSource {
    fn plan_view(&self, _plan_ref: &GoalPlanRef) -> Result<Option<GoalPlanView>> {
        Ok(None)
    }
}

/// Builds a [`GoalTurnContext`] from durable state plus the current plan.
pub struct GoalContextMaterializer {
    plan_source: Arc<dyn PlanSource>,
}

impl GoalContextMaterializer {
    pub fn new(plan_source: Arc<dyn PlanSource>) -> Self {
        Self { plan_source }
    }

    /// Materialize context for a goal-owned turn. Requires the goal to be active
    /// (it has a lease); a missing/unreadable plan yields
    /// `ContextUnavailable` so the supervisor pauses rather than starting an
    /// unanchored turn (§5.5).
    pub fn materialize(&self, snapshot: &GoalSnapshot) -> Result<GoalTurnContext> {
        let lease = snapshot.lifecycle.lease().ok_or_else(|| {
            GoalRuntimeError::context_unavailable("goal is not active; nothing to materialize")
        })?;

        let plan = match &snapshot.plan {
            Some(plan_ref) => match self.plan_source.plan_view(plan_ref)? {
                Some(view) => Some(view),
                None => {
                    return Err(GoalRuntimeError::context_unavailable(
                        "referenced plan artifact is missing or unreadable",
                    ));
                }
            },
            None => None,
        };

        Ok(GoalTurnContext {
            goal_id: snapshot.goal_id.clone(),
            spec_revision: snapshot.spec_revision,
            state_version: snapshot.state_version,
            lease_id: lease.lease_id().clone(),
            objective: snapshot.objective.text.to_string(),
            budget: GoalBudgetView {
                autonomous_turns_used: snapshot.counters.autonomous_turns,
                autonomous_turns_max: snapshot.budget.max_autonomous_turns.get(),
                total_turns: snapshot.counters.total_turns,
                tokens_used: snapshot.usage.total_tokens(),
                tokens_max: snapshot.budget.max_tokens.map(std::num::NonZeroU64::get),
            },
            plan,
            progress: snapshot.progress.clone(),
            wait_resolution: snapshot.wait_resolution.clone(),
            completion_contract: snapshot.contract.clone(),
        })
    }
}

#[cfg(test)]
#[path = "materializer.test.rs"]
mod tests;
