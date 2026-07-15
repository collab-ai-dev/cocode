use super::*;
use coco_goals::{
    CompletionPolicy, CreateGoal, GoalBudget, GoalCommand, GoalLeaseId, GoalObjective, Timestamp,
    WakeId, decide,
};
use coco_session::InMemoryStore;
use pretty_assertions::assert_eq;

fn session_id() -> SessionId {
    match SessionId::try_new("goal-store-test") {
        Ok(id) => id,
        Err(_) => unreachable!("valid session id"),
    }
}

fn snapshot() -> GoalSnapshot {
    let cmd = GoalCommand::Create(CreateGoal {
        goal_id: GoalId::new("g-1"),
        session_id: session_id(),
        lease_id: GoalLeaseId::new("l0"),
        objective: GoalObjective::new("ship"),
        contract: None,
        policy: CompletionPolicy::CandidateWithEvidence,
        budget: GoalBudget::default(),
        plan: None,
        mode_gate: None,
        wake_id: WakeId::new("w0"),
        at: Timestamp::from_millis(0),
    });
    match decide(None, cmd) {
        Ok(decision) => decision.snapshot.expect("snapshot"),
        Err(e) => unreachable!("create should succeed: {e}"),
    }
}

#[test]
fn test_persist_load_clear_roundtrip() {
    let store: Arc<dyn SessionStore> = Arc::new(InMemoryStore::new());
    let goal_store = TranscriptGoalStore::new(store, session_id());

    assert_eq!(goal_store.load().unwrap(), None);

    let snapshot = snapshot();
    goal_store.persist(&snapshot).unwrap();
    let loaded = goal_store.load().unwrap().expect("loaded snapshot");
    assert_eq!(loaded, snapshot);

    goal_store.clear(&snapshot.goal_id).unwrap();
    assert_eq!(goal_store.load().unwrap(), None);
}

#[test]
fn test_load_returns_highest_state_version() {
    let store: Arc<dyn SessionStore> = Arc::new(InMemoryStore::new());
    let goal_store = TranscriptGoalStore::new(store, session_id());

    let mut early = snapshot();
    early.state_version = coco_goals::StateVersion::INITIAL;
    let mut late = snapshot();
    late.state_version = late.state_version.next().next();

    goal_store.persist(&early).unwrap();
    goal_store.persist(&late).unwrap();
    assert_eq!(
        goal_store.load().unwrap().unwrap().state_version,
        late.state_version
    );
}
