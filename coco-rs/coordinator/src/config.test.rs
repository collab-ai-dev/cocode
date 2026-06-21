use super::*;

#[test]
fn test_teammate_mode_default() {
    assert_eq!(TeammateMode::default(), TeammateMode::Auto);
}

#[test]
fn test_teammate_mode_as_str() {
    assert_eq!(TeammateMode::Auto.as_str(), "auto");
    assert_eq!(TeammateMode::Tmux.as_str(), "tmux");
    assert_eq!(TeammateMode::InProcess.as_str(), "in-process");
}

#[test]
fn test_teammate_mode_serde() {
    let json = serde_json::to_string(&TeammateMode::InProcess).unwrap();
    assert_eq!(json, "\"in-process\"");
    let parsed: TeammateMode = serde_json::from_str("\"tmux\"").unwrap();
    assert_eq!(parsed, TeammateMode::Tmux);
}

#[test]
fn test_team_config_default() {
    let config = TeamConfig::default();
    assert_eq!(config.teammate_mode, TeammateMode::Auto);
    assert_eq!(config.default_model_role, coco_types::ModelRole::Main);
    assert!(config.agent_type_model_roles.is_empty());
    assert!(config.show_spinner_tree);
    assert_eq!(config.max_agents, 8);
}
