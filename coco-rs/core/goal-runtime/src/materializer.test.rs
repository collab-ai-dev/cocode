use super::*;
use crate::error::GoalRuntimeError;
use crate::test_support::{goal_id, running_snapshot, ts};
use coco_goals::{
    CompletionPolicy, ContentDigest, GoalCommand, GoalPlanRef, Pause, PauseReason, PlanArtifactId,
    PlanRevision, decide,
};
use pretty_assertions::assert_eq;
use std::sync::Arc;

struct MockPlanSource {
    view: Option<GoalPlanView>,
}

impl PlanSource for MockPlanSource {
    fn plan_view(&self, _plan_ref: &GoalPlanRef) -> crate::error::Result<Option<GoalPlanView>> {
        Ok(self.view.clone())
    }
}

fn plan_ref() -> GoalPlanRef {
    GoalPlanRef {
        artifact_id: PlanArtifactId::new("plan-1"),
        revision: PlanRevision::INITIAL,
        content_digest: Some(ContentDigest::new("abc")),
        observed_at: ts(0),
    }
}

fn plan_view() -> GoalPlanView {
    GoalPlanView {
        artifact_id: PlanArtifactId::new("plan-1"),
        revision: PlanRevision::INITIAL,
        display_path: "plans/session.md".to_string(),
        digest: Some(ContentDigest::new("abc")),
        headings: vec!["Approach".to_string()],
        active_steps: vec!["wire the handle".to_string()],
        drifted: false,
    }
}

#[test]
fn test_materialize_active_goal_without_plan() {
    let snapshot = running_snapshot(CompletionPolicy::CandidateWithEvidence, None);
    let materializer = GoalContextMaterializer::new(Arc::new(NoPlanSource));
    let context = materializer.materialize(&snapshot).unwrap();
    assert_eq!(context.objective, "ship the feature");
    assert_eq!(context.budget.autonomous_turns_max, 20);
    assert_eq!(context.budget.total_turns, 1);
    assert!(context.plan.is_none());
}

#[test]
fn test_materialize_resolves_plan_view() {
    let mut snapshot = running_snapshot(CompletionPolicy::CandidateWithEvidence, None);
    snapshot.plan = Some(plan_ref());
    let materializer = GoalContextMaterializer::new(Arc::new(MockPlanSource {
        view: Some(plan_view()),
    }));
    let context = materializer.materialize(&snapshot).unwrap();
    assert_eq!(context.plan.unwrap().display_path, "plans/session.md");
}

#[test]
fn test_missing_plan_is_context_unavailable() {
    let mut snapshot = running_snapshot(CompletionPolicy::CandidateWithEvidence, None);
    snapshot.plan = Some(plan_ref());
    let materializer = GoalContextMaterializer::new(Arc::new(MockPlanSource { view: None }));
    let err = materializer.materialize(&snapshot).unwrap_err();
    assert!(matches!(err, GoalRuntimeError::ContextUnavailable { .. }));
}

#[test]
fn test_non_active_goal_is_context_unavailable() {
    let running = running_snapshot(CompletionPolicy::CandidateWithEvidence, None);
    let paused = decide(
        Some(&running),
        GoalCommand::Pause(Pause {
            goal_id: goal_id(),
            reason: PauseReason::UserInterrupt,
            at: ts(5),
        }),
    )
    .unwrap()
    .snapshot
    .unwrap();
    let materializer = GoalContextMaterializer::new(Arc::new(NoPlanSource));
    let err = materializer.materialize(&paused).unwrap_err();
    assert!(matches!(err, GoalRuntimeError::ContextUnavailable { .. }));
}
