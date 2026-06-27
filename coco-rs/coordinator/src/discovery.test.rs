use super::*;

#[test]
fn test_teammate_run_status_as_str() {
    assert_eq!(TeammateRunStatus::Running.as_str(), "running");
    assert_eq!(TeammateRunStatus::Idle.as_str(), "idle");
    assert_eq!(TeammateRunStatus::Unknown.as_str(), "unknown");
}

#[test]
fn test_get_teammate_statuses_nonexistent() {
    let statuses = get_teammate_statuses("nonexistent-team-xyz-123");
    assert!(statuses.is_empty());
}

#[test]
fn test_list_teams_no_panic() {
    // Should not panic even if no teams exist
    let teams = list_teams();
    let _ = teams;
}
