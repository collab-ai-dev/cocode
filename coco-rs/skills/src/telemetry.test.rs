use super::*;
use pretty_assertions::assert_eq;
use tempfile::tempdir;

#[test]
fn records_success_and_failure_separately() {
    let temp = tempdir().unwrap();
    let home = temp.path();
    record_invocation(home, "s", SkillOutcome::Success);
    record_invocation(home, "s", SkillOutcome::Failure);
    record_invocation(home, "s", SkillOutcome::Failure);
    let stats = load_all(home);
    let s = stats.get("s").unwrap();
    assert_eq!(s.success_count, 1);
    assert_eq!(s.failure_count, 2);
    assert_eq!(s.total_invocations(), 3);
    assert_eq!(s.last_status, Some(SkillOutcome::Failure));
}

#[test]
fn no_debounce_every_event_counts() {
    // Unlike the autocomplete store, two rapid records both land.
    let temp = tempdir().unwrap();
    let home = temp.path();
    record_invocation(home, "s", SkillOutcome::Success);
    record_invocation(home, "s", SkillOutcome::Success);
    assert_eq!(load_all(home).get("s").unwrap().success_count, 2);
}

#[test]
fn patch_counter_accumulates() {
    let temp = tempdir().unwrap();
    let home = temp.path();
    record_patch(home, "s");
    record_patch(home, "s");
    let stats = load_all(home);
    let s = stats.get("s").unwrap();
    assert_eq!(s.patch_count, 2);
    assert!(s.last_patched_at_ms > 0);
}

#[test]
fn success_rate_is_one_for_unused_skill() {
    let s = SkillTelemetryStats::default();
    assert_eq!(s.success_rate(), 1.0);
    assert_eq!(s.total_invocations(), 0);
}

#[test]
fn success_rate_reflects_failures() {
    let s = SkillTelemetryStats {
        success_count: 1,
        failure_count: 3,
        ..Default::default()
    };
    assert_eq!(s.success_rate(), 0.25);
}

#[test]
fn missing_file_loads_empty() {
    let temp = tempdir().unwrap();
    assert!(load_all(temp.path()).is_empty());
}
