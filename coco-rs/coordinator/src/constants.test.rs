use super::*;

#[test]
fn test_team_lead_name() {
    assert_eq!(TEAM_LEAD_NAME, "team-lead");
}

#[test]
fn test_env_var_names() {
    assert_eq!(TEAMMATE_COMMAND_ENV_VAR.as_str(), "COCO_TEAMMATE_COMMAND");
    assert_eq!(TEAMMATE_COLOR_ENV_VAR.as_str(), "COCO_AGENT_COLOR");
    assert_eq!(
        PLAN_MODE_REQUIRED_ENV_VAR.as_str(),
        "COCO_PLAN_MODE_REQUIRED"
    );
    assert_eq!(AGENT_ID_ENV_VAR.as_str(), "COCO_AGENT_ID");
    assert_eq!(AGENT_NAME_ENV_VAR.as_str(), "COCO_AGENT_NAME");
    assert_eq!(TEAM_NAME_ENV_VAR.as_str(), "COCO_TEAM_NAME");
    assert_eq!(PARENT_SESSION_ID_ENV_VAR.as_str(), "COCO_PARENT_SESSION_ID");
    assert_eq!(VERIFY_PLAN_ENV_VAR.as_str(), "COCO_VERIFY_PLAN");
}

#[test]
fn test_swarm_socket_name_format() {
    let name = swarm_socket_name();
    assert!(name.starts_with("claude-swarm-"));
}

// `AgentColorName` tests live with the canonical type in `coco-types`
// (see `common/types/src/agent.test.rs`). The re-export at
// `crate::constants::AgentColorName` is just a path alias.
