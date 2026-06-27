use super::*;
use pretty_assertions::assert_eq;

fn request(tool_name: &str) -> ToolPermissionRequest {
    ToolPermissionRequest {
        id: "req-1".to_string(),
        tool_use_id: "toolu_1".to_string(),
        agent_id: "agent-1".to_string(),
        tool_name: tool_name.to_string(),
        description: String::new(),
        input: serde_json::Value::Null,
        cwd: None,
        suggestions: Vec::new(),
        choices: None,
        detail: None,
        worker_badge: None,
    }
}

#[test]
fn test_tool_registry_builder_registers_selected_tools() {
    let registry = ToolRegistryBuilder::new().with_bash().with_read().build();
    assert_eq!(registry.len(), 2);
    assert!(registry.get_by_name("Bash").is_some());
    assert!(registry.get_by_name("Read").is_some());
    assert!(registry.get_by_name("Write").is_none());
}

#[test]
fn test_tool_registry_builder_with_core_registers_all_six() {
    let registry = ToolRegistryBuilder::new().with_core().build();
    for name in ["Bash", "Read", "Write", "Edit", "Glob", "Grep"] {
        assert!(
            registry.get_by_name(name).is_some(),
            "core registry must include {name}"
        );
    }
}

#[tokio::test]
async fn test_permission_bridge_allow_all_approves_everything() {
    let bridge = PermissionBridgeBuilder::allow_all();
    let res = bridge.request_permission(request("Bash")).await.unwrap();
    assert_eq!(res.decision, ToolPermissionDecision::Approved);
}

#[tokio::test]
async fn test_permission_bridge_deny_all_rejects_everything() {
    let bridge = PermissionBridgeBuilder::deny_all();
    let res = bridge.request_permission(request("Read")).await.unwrap();
    assert_eq!(res.decision, ToolPermissionDecision::Rejected);
}

#[tokio::test]
async fn test_permission_bridge_allow_tool_is_an_allow_list() {
    let bridge = PermissionBridgeBuilder::new()
        .allow_tool("Read")
        .allow_tool("Glob")
        .build();

    let allowed = bridge.request_permission(request("Read")).await.unwrap();
    assert_eq!(allowed.decision, ToolPermissionDecision::Approved);

    let rejected = bridge.request_permission(request("Bash")).await.unwrap();
    assert_eq!(rejected.decision, ToolPermissionDecision::Rejected);
}
