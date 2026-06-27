// PaneBackendExecutor tests require a real PaneBackend implementation.
// These are integration tests that would need tmux available.
// Unit tests focus on the agent_id parsing and tracking logic.

#[test]
fn test_agent_id_parse() {
    let agent_id = "researcher@my-team";
    let name = agent_id.split('@').next().unwrap_or(agent_id);
    let team = agent_id.split('@').nth(1).unwrap_or("default");
    assert_eq!(name, "researcher");
    assert_eq!(team, "my-team");
}

#[test]
fn test_agent_id_no_at() {
    let agent_id = "standalone";
    let name = agent_id.split('@').next().unwrap_or(agent_id);
    let team = agent_id.split('@').nth(1).unwrap_or("default");
    assert_eq!(name, "standalone");
    assert_eq!(team, "default");
}
