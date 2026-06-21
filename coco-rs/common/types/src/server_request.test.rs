use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

#[test]
fn ask_for_approval_wire_method() {
    let req = ServerRequest::AskForApproval(AskForApprovalParams {
        request_id: "req-1".into(),
        tool_name: "Bash".into(),
        input: json!({ "command": "ls" }),
        tool_use_id: "tu-1".into(),
        description: Some("list files".into()),
        title: None,
        display_name: None,
        blocked_path: None,
        decision_reason: None,
        agent_id: None,
        cwd: None,
        permission_suggestions: vec![],
    });
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "approval/askForApproval");
    assert_eq!(j["params"]["tool_name"], "Bash");
    assert_eq!(j["params"]["tool_use_id"], "tu-1");

    // Typed discriminator agrees with serde wire tag.
    assert_eq!(req.method(), ServerRequestMethod::AskForApproval);
    assert_eq!(
        ServerRequestMethod::AskForApproval.as_str(),
        "approval/askForApproval"
    );
}

#[test]
fn request_user_input_wire_method() {
    let req = ServerRequest::RequestUserInput(RequestUserInputParams {
        request_id: "req-2".into(),
        prompt: "Choose one:".into(),
        description: None,
        choices: vec!["a".into(), "b".into()],
        default: Some("a".into()),
    });
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "input/requestUserInput");
    assert_eq!(j["params"]["prompt"], "Choose one:");
    assert_eq!(j["params"]["choices"].as_array().unwrap().len(), 2);
}

#[test]
fn mcp_route_message_wire_method() {
    let req = ServerRequest::McpRouteMessage(McpRouteMessageParams {
        server_name: "github".into(),
        message: json!({ "jsonrpc": "2.0", "method": "tools/list" }),
    });
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "mcp/routeMessage");
    assert_eq!(j["params"]["server_name"], "github");
}

#[test]
fn hook_callback_wire_method() {
    let req = ServerRequest::HookCallback(HookCallbackParams {
        callback_id: "cb-1".into(),
        event_type: crate::HookEventType::PreToolUse,
        input: json!({ "tool_name": "Bash" }),
        tool_use_id: Some("tu-1".into()),
    });
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "hook/callback");
    assert_eq!(j["params"]["callback_id"], "cb-1");
}

#[test]
fn cancel_request_wire_method() {
    let req = ServerRequest::CancelRequest(ServerCancelRequestParams {
        request_id: "req-5".into(),
        reason: Some("timeout".into()),
    });
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "control/cancelRequest");
    assert_eq!(j["params"]["reason"], "timeout");
}

#[test]
fn mcp_status_result_roundtrip() {
    let r = McpStatusResult {
        mcp_servers: vec![McpServerStatus {
            name: "github".into(),
            status: McpConnectionStatus::Connected,
            tool_count: 5,
            error: None,
            skipped_tools: Vec::new(),
            tombstoned_tools: Vec::new(),
        }],
    };
    let j = serde_json::to_value(&r).unwrap();
    assert_eq!(j["mcpServers"][0]["name"], "github");
    assert_eq!(j["mcpServers"][0]["status"], "connected");
    assert_eq!(j["mcpServers"][0]["tool_count"], 5);
}

#[test]
fn mcp_connection_status_wire_format_matches_ts() {
    // TS enum: 'connected' | 'failed' | 'needs-auth' | 'pending' | 'disabled'
    // Note kebab-case `needs-auth` (not snake_case), matches TS
    // coreSchemas.ts:167-173.
    assert_eq!(
        serde_json::to_value(McpConnectionStatus::Connected).unwrap(),
        "connected"
    );
    assert_eq!(
        serde_json::to_value(McpConnectionStatus::Pending).unwrap(),
        "pending"
    );
    assert_eq!(
        serde_json::to_value(McpConnectionStatus::Failed).unwrap(),
        "failed"
    );
    assert_eq!(
        serde_json::to_value(McpConnectionStatus::NeedsAuth).unwrap(),
        "needs-auth"
    );
    assert_eq!(
        serde_json::to_value(McpConnectionStatus::Disabled).unwrap(),
        "disabled"
    );
    // coco-rs extension — not in TS.
    assert_eq!(
        serde_json::to_value(McpConnectionStatus::Disconnected).unwrap(),
        "disconnected"
    );
}

#[test]
fn context_usage_result_roundtrip() {
    let r = ContextUsageResult {
        total_tokens: 50_000,
        max_tokens: 200_000,
        raw_max_tokens: 200_000,
        percentage: 25.0,
        model: "claude-opus".into(),
        categories: vec![ContextUsageCategory {
            kind: crate::ContextCategoryKind::SystemPrompt,
            tokens: 5000,
        }],
        is_auto_compact_enabled: true,
        auto_compact_threshold: Some(180_000),
        message_breakdown: None,
        memory_files: Vec::new(),
        mcp_tools: Vec::new(),
        agents: Vec::new(),
        skills: Vec::new(),
        suggestions: Vec::new(),
    };
    let j = serde_json::to_value(&r).unwrap();
    assert_eq!(j["total_tokens"], 50_000);
    assert_eq!(j["percentage"], 25.0);
    assert_eq!(j["categories"][0]["kind"], "system_prompt");
}
