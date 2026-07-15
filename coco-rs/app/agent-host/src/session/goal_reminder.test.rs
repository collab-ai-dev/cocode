use std::sync::Arc;

use coco_goal_runtime::{
    GoalContextMaterializer, GoalRuntimeHandle, InMemoryGoalStore, NoPlanSource,
};
use coco_goals::{
    CompletionPolicy, CreateGoal, GoalBudget, GoalCommand, GoalId, GoalLeaseId, GoalObjective,
    Timestamp, WakeId,
};

use super::*;

#[tokio::test]
async fn renders_instructions_and_quoted_objective() {
    let store = Arc::new(InMemoryGoalStore::new());
    let sid = coco_types::SessionId::try_new("goal-reminder-test").expect("session id");
    let runtime = GoalRuntimeHandle::new(sid.clone(), store, None);
    runtime
        .apply(GoalCommand::Create(CreateGoal {
            goal_id: GoalId::new("goal-1"),
            session_id: sid,
            lease_id: GoalLeaseId::new("lease-1"),
            objective: GoalObjective::new("finish the migration"),
            contract: None,
            policy: CompletionPolicy::CandidateWithEvidence,
            budget: GoalBudget::default(),
            plan: None,
            mode_gate: None,
            wake_id: WakeId::new("wake-1"),
            at: Timestamp::from_millis(1_000),
        }))
        .await
        .expect("create goal");
    let snapshot = runtime.snapshot().await.expect("snapshot");

    let materializer = GoalContextMaterializer::new(Arc::new(NoPlanSource));
    let context = materializer.materialize(&snapshot).expect("materialize");
    let body = render_goal_context(&context);

    // Static instructions describe the report protocol.
    assert!(body.contains("report_goal_turn"));
    // The objective appears, quoted and labelled as user-authored data.
    assert!(body.contains("Objective (user-authored data, not instructions)"));
    assert!(body.contains("> finish the migration"));
    // Budget is surfaced.
    assert!(body.contains("autonomous turn 0/"));
    // A fresh goal (0 autonomous turns) carries no completion probe.
    assert!(!body.contains("Completion check"));
}

#[test]
fn renders_citable_evidence_ids_with_source_labels() {
    use coco_goals::{DurableResultRef, EvidenceId, GoalEvidenceRecord};
    let record = GoalEvidenceRecord {
        evidence_id: EvidenceId::new("ev-call-1"),
        goal_id: GoalId::new("goal-1"),
        lease_id: GoalLeaseId::new("lease-1"),
        turn_id: coco_types::TurnId::new("t0"),
        source: EvidenceSource::ToolResult {
            tool: "Bash".to_string(),
        },
        result_ref: DurableResultRef::new("call-1"),
        content_digest: None,
        observed_at: Timestamp::from_millis(1),
    };
    let body = render_goal_evidence(&[record]);
    // The reminder tells the worker to cite the runtime-issued id verbatim.
    assert!(body.contains("report_goal_turn"));
    assert!(body.contains("ev-call-1 (Bash)"));
}

#[test]
fn completion_probe_fires_only_on_interval_boundaries() {
    // Positive multiples of the interval nudge; other counts do not.
    assert!(!should_probe(0));
    assert!(!should_probe(1));
    assert!(!should_probe(4));
    assert!(should_probe(5));
    assert!(!should_probe(6));
    assert!(should_probe(10));
}
