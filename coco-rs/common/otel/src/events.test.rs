use super::*;

#[test]
fn test_app_event_new() {
    let event = AppEvent::new(AppEventType::SessionStart);
    assert_eq!(event.event_type, AppEventType::SessionStart);
    assert!(event.timestamp_ms > 0);
    assert!(event.attributes.is_empty());
}

#[test]
fn test_app_event_with_attributes() {
    let event = AppEvent::new(AppEventType::ToolUse)
        .with_str("tool_name", "BashTool")
        .with_int("duration_ms", 1500)
        .with_bool("success", true)
        .with_float("cost_usd", 0.003);

    assert_eq!(event.attributes.len(), 4);
    assert_eq!(
        event.attributes.get("tool_name").and_then(|v| v.as_str()),
        Some("BashTool")
    );
    assert_eq!(
        event
            .attributes
            .get("duration_ms")
            .and_then(serde_json::Value::as_i64),
        Some(1500)
    );
    assert_eq!(
        event
            .attributes
            .get("success")
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );
}

#[test]
fn test_app_event_type_as_str() {
    assert_eq!(AppEventType::SessionStart.as_str(), "session_start");
    assert_eq!(AppEventType::ToolUse.as_str(), "tool_use");
    assert_eq!(AppEventType::ApiRetry.as_str(), "api_retry");
    assert_eq!(AppEventType::McpToolCall.as_str(), "mcp_tool_call");
}

#[test]
fn test_event_serialization() {
    let event = AppEvent::new(AppEventType::ApiResponse)
        .with_str("model", "claude-opus-4-6")
        .with_int("input_tokens", 1000);

    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["event_type"], "api_response");
    assert_eq!(json["model"], "claude-opus-4-6");
    assert_eq!(json["input_tokens"], 1000);
}

#[test]
fn test_emit_event_does_not_panic() {
    // Just verify emit doesn't panic without a tracing subscriber
    emit_session_start("test-session", "claude-opus-4-6");
    emit_tool_use("BashTool", 500, true);
    emit_api_request("claude-sonnet-4-6", 100, 50, 0.001);
    emit_slash_command("compact");
    emit_subagent_spawn("agent-1", "general", "claude-sonnet-4-6");
}
