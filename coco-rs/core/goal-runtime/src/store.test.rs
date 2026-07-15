use super::*;
use coco_goals::{
    CreateGoal, GoalBudget, GoalCommand, GoalId, GoalLeaseId, GoalObjective, GoalSnapshot,
    Timestamp, WakeId, decide,
};
use coco_types::SessionId;
use pretty_assertions::assert_eq;

fn snapshot() -> GoalSnapshot {
    let cmd = GoalCommand::Create(CreateGoal {
        goal_id: GoalId::new("g-1"),
        session_id: SessionId::try_new("sess-1").unwrap(),
        lease_id: GoalLeaseId::new("l0"),
        objective: GoalObjective::new("ship"),
        contract: None,
        policy: coco_goals::CompletionPolicy::CandidateWithEvidence,
        budget: GoalBudget::default(),
        plan: None,
        mode_gate: None,
        wake_id: WakeId::new("w0"),
        at: Timestamp::from_millis(0),
    });
    decide(None, cmd).unwrap().snapshot.unwrap()
}

#[test]
fn test_persist_then_load_returns_latest() {
    let store = InMemoryGoalStore::new();
    assert_eq!(store.load().unwrap(), None);
    let snap = snapshot();
    store.persist(&snap).unwrap();
    assert_eq!(store.append_count(), 1);
    assert_eq!(store.load().unwrap(), Some(snap));
}

#[test]
fn test_load_prefers_highest_state_version() {
    let store = InMemoryGoalStore::new();
    let mut early = snapshot();
    let mut late = snapshot();
    late.state_version = late.state_version.next();
    // Persist out of order; load must still pick the highest state version.
    store.persist(&late).unwrap();
    early.state_version = coco_goals::StateVersion::INITIAL;
    store.persist(&early).unwrap();
    assert_eq!(
        store.load().unwrap().unwrap().state_version,
        late.state_version
    );
}

#[test]
fn test_clear_hides_prior_snapshot() {
    let store = InMemoryGoalStore::new();
    store.persist(&snapshot()).unwrap();
    store.clear(&GoalId::new("g-1")).unwrap();
    assert_eq!(store.load().unwrap(), None);
}

#[test]
fn test_persist_after_clear_is_visible_again() {
    let store = InMemoryGoalStore::new();
    store.persist(&snapshot()).unwrap();
    store.clear(&GoalId::new("g-1")).unwrap();
    store.persist(&snapshot()).unwrap();
    assert!(store.load().unwrap().is_some());
}
