use super::*;
use crate::test_support::{evidence_record, goal_id};
use coco_goals::EvidenceId;
use pretty_assertions::assert_eq;

#[test]
fn test_record_and_resolve_found() {
    let store = InMemoryEvidenceStore::new();
    store.record(evidence_record("e-1", &goal_id())).unwrap();
    let resolved = store.resolve(&[EvidenceId::new("e-1")]).unwrap();
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].evidence_id, EvidenceId::new("e-1"));
}

#[test]
fn test_resolve_omits_unknown_ids() {
    let store = InMemoryEvidenceStore::new();
    store.record(evidence_record("e-1", &goal_id())).unwrap();
    let resolved = store
        .resolve(&[EvidenceId::new("e-1"), EvidenceId::new("missing")])
        .unwrap();
    // Unknown ids are omitted so the gate's ownership check fails closed on them.
    assert_eq!(resolved.len(), 1);
}

#[test]
fn test_recent_for_goal_is_newest_first_capped_and_owned() {
    let store = InMemoryEvidenceStore::new();
    let mine = goal_id();
    let other = coco_goals::GoalId::new("other-goal");
    store.record(evidence_record("e-1", &mine)).unwrap();
    store.record(evidence_record("e-2", &other)).unwrap();
    store.record(evidence_record("e-3", &mine)).unwrap();
    store.record(evidence_record("e-4", &mine)).unwrap();

    let recent = store.recent_for_goal(&mine, 2).unwrap();
    // Newest-first, capped at the limit, and only records owned by this goal.
    assert_eq!(recent.len(), 2);
    assert_eq!(recent[0].evidence_id, EvidenceId::new("e-4"));
    assert_eq!(recent[1].evidence_id, EvidenceId::new("e-3"));
}
