use super::*;

use coco_types::AgentDefinition;

fn def_with_required_mcp(name: &str, required: &[&str]) -> AgentDefinition {
    AgentDefinition {
        name: name.to_string(),
        required_mcp_servers: required.iter().map(ToString::to_string).collect(),
        ..Default::default()
    }
}

#[test]
fn test_has_required_mcp_servers_empty_required_passes() {
    let def = def_with_required_mcp("agent", &[]);
    assert!(has_required_mcp_servers(&def, &[]));
    assert!(has_required_mcp_servers(&def, &["github".into()]));
}

#[test]
fn test_has_required_mcp_servers_all_present() {
    let def = def_with_required_mcp("agent", &["github", "slack"]);
    assert!(has_required_mcp_servers(
        &def,
        &["github".into(), "slack".into(), "linear".into()],
    ));
}

#[test]
fn test_has_required_mcp_servers_one_missing() {
    let def = def_with_required_mcp("agent", &["github", "slack"]);
    assert!(!has_required_mcp_servers(&def, &["github".into()]));
}

#[test]
fn test_has_required_mcp_servers_case_insensitive_substring() {
    // Case-insensitive substring match.
    let def = def_with_required_mcp("agent", &["GitHub"]);
    assert!(has_required_mcp_servers(&def, &["github-prod".into()],));
    let def_substr = def_with_required_mcp("agent", &["lack"]);
    assert!(has_required_mcp_servers(&def_substr, &["slack".into()]));
}
