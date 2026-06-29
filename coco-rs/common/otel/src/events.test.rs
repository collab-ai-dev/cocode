use super::*;
use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().expect("env test lock")
}

fn clear_assistant_response_env() {
    unsafe {
        std::env::remove_var(EnvKey::OtelLogAssistantResponses);
        std::env::remove_var(EnvKey::OtelLogUserPrompts);
    }
}

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
    assert_eq!(
        AppEventType::AssistantResponse.as_str(),
        "assistant_response"
    );
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
    emit_assistant_response(
        "done",
        "claude-sonnet-4-6",
        Some("req-1"),
        "repl_main_thread",
    );
}

#[test]
fn assistant_response_payload_skips_empty_text() {
    let _guard = lock_env();
    clear_assistant_response_env();

    assert_eq!(build_assistant_response_payload("", true), None);
}

#[test]
fn assistant_response_logging_inherits_prompt_logging_when_unset() {
    let _guard = lock_env();
    clear_assistant_response_env();

    let payload =
        build_assistant_response_payload("visible response", true).expect("payload emitted");

    assert_eq!(
        payload,
        AssistantResponsePayload {
            response_length: 16,
            response: "visible response".to_string(),
        }
    );
}

#[test]
fn assistant_response_logging_explicit_false_overrides_prompt_logging() {
    let _guard = lock_env();
    clear_assistant_response_env();
    unsafe {
        std::env::set_var(EnvKey::OtelLogAssistantResponses, "0");
    }

    let payload =
        build_assistant_response_payload("visible response", true).expect("payload emitted");

    assert_eq!(
        payload,
        AssistantResponsePayload {
            response_length: 16,
            response: REDACTED.to_string(),
        }
    );
    clear_assistant_response_env();
}

#[test]
fn assistant_response_logging_explicit_true_overrides_prompt_redaction() {
    let _guard = lock_env();
    clear_assistant_response_env();
    unsafe {
        std::env::set_var(EnvKey::OtelLogAssistantResponses, "1");
    }

    let payload =
        build_assistant_response_payload("visible response", false).expect("payload emitted");

    assert_eq!(payload.response, "visible response");
    clear_assistant_response_env();
}

#[test]
fn assistant_response_length_matches_js_utf16_length() {
    let _guard = lock_env();
    clear_assistant_response_env();
    unsafe {
        std::env::set_var(EnvKey::OtelLogAssistantResponses, "1");
    }

    let payload = build_assistant_response_payload("a😀", false).expect("payload emitted");

    assert_eq!(payload.response_length, 3);
    clear_assistant_response_env();
}

#[test]
fn assistant_response_logging_truncates_at_utf8_boundary() {
    let _guard = lock_env();
    clear_assistant_response_env();
    unsafe {
        std::env::set_var(EnvKey::OtelLogAssistantResponses, "1");
    }
    let content = format!("{}é", "a".repeat(TELEMETRY_CONTENT_LIMIT_BYTES - 1));

    let payload = build_assistant_response_payload(&content, false).expect("payload emitted");

    assert!(payload.response.ends_with(TELEMETRY_TRUNCATION_MARKER));
    assert!(
        payload
            .response
            .starts_with(&"a".repeat(TELEMETRY_CONTENT_LIMIT_BYTES - 1))
    );
    assert!(!payload.response.contains('é'));
    clear_assistant_response_env();
}

#[test]
fn emit_assistant_response_uses_env_inheritance() {
    let _guard = lock_env();
    clear_assistant_response_env();
    unsafe {
        std::env::set_var(EnvKey::OtelLogUserPrompts, "1");
    }

    let payload = build_assistant_response_payload(
        "visible response",
        is_env_truthy(EnvKey::OtelLogUserPrompts),
    )
    .expect("payload emitted");

    assert_eq!(payload.response, "visible response");
    clear_assistant_response_env();
}
