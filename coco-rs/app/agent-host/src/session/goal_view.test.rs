use std::sync::Arc;

use coco_goal_runtime::{GoalRuntimeHandle, InMemoryGoalStore};
use coco_goals::{
    CompletionPolicy, CreateGoal, GoalBudget, GoalCommand, GoalId, GoalLeaseId, GoalObjective,
    Timestamp, WakeId,
};

use super::*;

async fn active_snapshot() -> coco_goals::GoalSnapshot {
    let store = Arc::new(InMemoryGoalStore::new());
    let sid = coco_types::SessionId::try_new("goal-view-test").expect("session id");
    let runtime = GoalRuntimeHandle::new(sid.clone(), store, None);
    runtime
        .apply(GoalCommand::Create(CreateGoal {
            goal_id: GoalId::new("goal-1"),
            session_id: sid,
            lease_id: GoalLeaseId::new("lease-1"),
            objective: GoalObjective::new("finish migration"),
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
    runtime.snapshot().await.expect("snapshot")
}

#[tokio::test]
async fn maps_active_snapshot_fields_to_view() {
    let snapshot = active_snapshot().await;
    let view = goal_snapshot_view(&snapshot);

    assert_eq!(view.goal_id, "goal-1");
    assert_eq!(view.objective, "finish migration");
    assert_eq!(view.status, coco_types::GoalStatusKind::Active);
    // A plain active goal carries no stopped/waiting detail.
    assert!(view.status_detail.is_none());
    assert_eq!(
        view.max_autonomous_turns,
        GoalBudget::default().max_autonomous_turns.get() as i32
    );
}
