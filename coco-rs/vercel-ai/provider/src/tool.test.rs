use super::*;
use serde_json::json;

#[test]
fn test_tool_invocation() {
    let invocation = ToolInvocation::pending("call_123", "search", json!({"query": "test"}));

    assert_eq!(invocation.tool_call_id, "call_123");
    assert_eq!(invocation.tool_name, "search");
    assert!(invocation.output.is_none());
    assert!(!invocation.is_error);
}

#[test]
fn test_tool_invocation_with_output() {
    let invocation =
        ToolInvocation::pending("call_1", "test", json!({})).with_output(json!({"status": "ok"}));

    assert!(invocation.output.is_some());
    assert!(!invocation.is_error);
}

#[test]
fn test_tool_invocation_with_error() {
    let invocation =
        ToolInvocation::pending("call_1", "test", json!({})).with_error(json!({"error": "failed"}));

    assert!(invocation.output.is_some());
    assert!(invocation.is_error);
}
