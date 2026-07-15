use crate::test_support::*;
use crate::*;
use pretty_assertions::assert_eq;

#[test]
fn test_created_snapshot_has_continuation_owner() {
    let snapshot = created_snapshot();
    assert!(snapshot.is_active());
    assert!(snapshot.has_continuation_owner());
    assert_eq!(snapshot.schema_version, SCHEMA_VERSION);
    assert_eq!(snapshot.spec_revision, SpecRevision::INITIAL);
    assert_eq!(snapshot.state_version, StateVersion::INITIAL);
}

#[test]
fn test_objective_bounds_and_defaults() {
    let objective = GoalObjective::new("do the thing");
    assert_eq!(objective.text.as_str(), "do the thing");
    assert!(objective.attachments.is_empty());
    assert!(objective.amendments.is_empty());
}

#[test]
fn test_full_snapshot_serde_roundtrip() {
    let snapshot = running_snapshot();
    let json = serde_json::to_string(&snapshot).unwrap();
    let back: GoalSnapshot = serde_json::from_str(&json).unwrap();
    assert_eq!(back, snapshot);
}

#[test]
fn test_status_helpers_track_lifecycle() {
    let snapshot = running_snapshot();
    assert_eq!(snapshot.status(), GoalStatus::Active);
    assert!(!snapshot.is_terminal());
}
