use crate::*;
use coco_goals::{
    CreateGoal, FinishTurn, GoalBudget, GoalCommand, GoalId, GoalLeaseId, GoalObjective,
    GoalSnapshot, GoalStatus, GoalTurnTrigger, Pause, PauseReason, StartTurn, Timestamp,
    TurnFinishOutcome, UsageDelta, WakeId,
};
use coco_types::{SessionId, TurnId};
use pretty_assertions::assert_eq;
use std::sync::Arc;

fn sid() -> SessionId {
    SessionId::try_new("sess-1").unwrap()
}

fn create_cmd() -> GoalCommand {
    GoalCommand::Create(CreateGoal {
        goal_id: GoalId::new("g-1"),
        session_id: sid(),
        lease_id: GoalLeaseId::new("l0"),
        objective: GoalObjective::new("ship the feature"),
        contract: None,
        policy: coco_goals::CompletionPolicy::CandidateWithEvidence,
        budget: GoalBudget::default(),
        plan: None,
        mode_gate: None,
        wake_id: WakeId::new("w0"),
        at: Timestamp::from_millis(0),
    })
}

fn handle() -> (GoalRuntimeHandle, Arc<InMemoryGoalStore>) {
    let store = Arc::new(InMemoryGoalStore::new());
    let handle = GoalRuntimeHandle::new(sid(), store.clone(), None);
    (handle, store)
}

#[tokio::test]
async fn test_create_commits_and_projects() {
    let (handle, store) = handle();
    let decision = handle.apply(create_cmd()).await.unwrap();
    assert_eq!(decision.event, coco_goals::GoalTransitionEvent::Created);
    assert!(decision.schedules_turn());
    assert_eq!(store.append_count(), 1);
    assert_eq!(handle.status().await, Some(GoalStatus::Active));
    assert!(handle.has_live_goal().await);
}

#[tokio::test]
async fn test_full_turn_cycle_appends_each_commit() {
    let (handle, store) = handle();
    handle.apply(create_cmd()).await.unwrap();
    handle
        .apply(GoalCommand::StartTurn(StartTurn {
            goal_id: GoalId::new("g-1"),
            lease_id: GoalLeaseId::new("l0"),
            turn_id: TurnId::new("t0"),
            trigger: GoalTurnTrigger::Creation,
            at: Timestamp::from_millis(1),
        }))
        .await
        .unwrap();
    handle
        .apply(GoalCommand::FinishTurn(FinishTurn {
            goal_id: GoalId::new("g-1"),
            lease_id: GoalLeaseId::new("l0"),
            turn_id: TurnId::new("t0"),
            reported: true,
            signals: vec![coco_goals::ProgressSignal::ToolObservation],
            usage: UsageDelta::default(),
            outcome: TurnFinishOutcome::Continue {
                next_lease_id: GoalLeaseId::new("l1"),
                checkpoint: None,
                rejection: None,
            },
            at: Timestamp::from_millis(2),
        }))
        .await
        .unwrap();
    assert_eq!(store.append_count(), 3);
    let snapshot = handle.snapshot().await.unwrap();
    assert_eq!(snapshot.status(), GoalStatus::Active);
    assert_eq!(snapshot.counters.total_turns, 1);
}

#[tokio::test]
async fn test_clear_removes_projection_and_durable_state() {
    let (handle, store) = handle();
    handle.apply(create_cmd()).await.unwrap();
    let decision = handle
        .apply(GoalCommand::Clear(coco_goals::Clear {
            goal_id: GoalId::new("g-1"),
            at: Timestamp::from_millis(5),
        }))
        .await
        .unwrap();
    assert!(decision.snapshot.is_none());
    assert_eq!(handle.snapshot().await, None);
    assert_eq!(store.load().unwrap(), None);
    assert!(!handle.has_live_goal().await);
}

#[tokio::test]
async fn test_restore_recovers_latest_snapshot() {
    let store = Arc::new(InMemoryGoalStore::new());
    let handle = GoalRuntimeHandle::new(sid(), store.clone(), None);
    handle.apply(create_cmd()).await.unwrap();

    // A fresh handle over the same store recovers the projection on resume.
    let restored = GoalRuntimeHandle::restore(sid(), store).unwrap();
    assert_eq!(restored.status().await, Some(GoalStatus::Active));
}

#[tokio::test]
async fn test_transition_error_leaves_projection_unchanged() {
    let (handle, _store) = handle();
    handle.apply(create_cmd()).await.unwrap();
    // A second create is rejected; projection stays on the first goal.
    let err = handle.apply(create_cmd()).await.unwrap_err();
    assert!(matches!(
        err,
        GoalRuntimeError::Transition {
            source: coco_goals::GoalTransitionError::GoalAlreadyActive
        }
    ));
    assert_eq!(handle.status().await, Some(GoalStatus::Active));
}

#[tokio::test]
async fn test_stale_goal_id_is_conflict() {
    let (handle, _store) = handle();
    handle.apply(create_cmd()).await.unwrap();
    let err = handle
        .apply(GoalCommand::Pause(Pause {
            goal_id: GoalId::new("other-goal"),
            reason: PauseReason::UserInterrupt,
            at: Timestamp::from_millis(9),
        }))
        .await
        .unwrap_err();
    assert!(err.is_conflict());
}

#[tokio::test]
async fn test_store_failure_does_not_advance_projection() {
    struct FailingStore;
    impl GoalStore for FailingStore {
        fn persist(&self, _snapshot: &GoalSnapshot) -> Result<()> {
            Err(GoalRuntimeError::store("disk full"))
        }
        fn clear(&self, _goal_id: &GoalId) -> Result<()> {
            Ok(())
        }
        fn load(&self) -> Result<Option<GoalSnapshot>> {
            Ok(None)
        }
    }

    let handle = GoalRuntimeHandle::new(sid(), Arc::new(FailingStore), None);
    let err = handle.apply(create_cmd()).await.unwrap_err();
    assert!(matches!(err, GoalRuntimeError::Store { .. }));
    // Durable-before-visible: the failed persist leaves the projection empty.
    assert_eq!(handle.snapshot().await, None);
}
