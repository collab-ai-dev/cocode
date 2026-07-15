use super::*;
use pretty_assertions::assert_eq;

#[test]
fn test_spec_revision_starts_at_one_and_advances() {
    assert_eq!(SpecRevision::INITIAL.get(), 1);
    assert_eq!(SpecRevision::INITIAL.next().get(), 2);
}

#[test]
fn test_state_version_starts_at_zero_and_advances() {
    assert_eq!(StateVersion::INITIAL.get(), 0);
    assert_eq!(StateVersion::INITIAL.next().get(), 1);
}

#[test]
fn test_plan_revision_advances() {
    assert_eq!(PlanRevision::INITIAL.next().get(), 1);
}

#[test]
fn test_string_id_transparent_serde_roundtrip() {
    let id = GoalId::new("g-123");
    let json = serde_json::to_string(&id).unwrap();
    assert_eq!(json, "\"g-123\"");
    let back: GoalId = serde_json::from_str(&json).unwrap();
    assert_eq!(back, id);
}

#[test]
fn test_timestamp_millis_roundtrip() {
    let ts = Timestamp::from_millis(1_700_000_000_000);
    assert_eq!(ts.millis(), 1_700_000_000_000);
    let json = serde_json::to_string(&ts).unwrap();
    assert_eq!(json, "1700000000000");
}
