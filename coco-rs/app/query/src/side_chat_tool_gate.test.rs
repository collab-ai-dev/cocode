use std::sync::Arc;

use coco_tool_runtime::{CanUseToolCallContext, CanUseToolDecision, TurnAbortSignal};
use serde_json::json;

use super::*;

fn ctx() -> CanUseToolCallContext {
    CanUseToolCallContext {
        tool_use_id: "t".into(),
        cwd: std::path::PathBuf::from("/"),
        abort: TurnAbortSignal::from_token(tokio_util::sync::CancellationToken::new()),
        require_can_use_tool: true,
        messages: Arc::new(Vec::new()),
    }
}

async fn decide(tool: &str, input: serde_json::Value) -> CanUseToolDecision {
    let tool_id: ToolId = tool.parse().expect("valid tool id");
    SideChatReadOnlyHandle
        .check(&tool_id, tool, &input, &ctx())
        .await
}

fn is_ask(d: &CanUseToolDecision) -> bool {
    matches!(d, CanUseToolDecision::Ask { .. })
}

fn is_deny(d: &CanUseToolDecision) -> bool {
    matches!(d, CanUseToolDecision::Deny { .. })
}

#[tokio::test]
async fn read_tools_return_ask_so_permissions_still_run() {
    // Ask (not Allow) is deliberate: the tool's own permission evaluator must
    // still apply explicit denies / sensitive-path checks / approval prompts.
    for tool in ["Read", "Glob", "Grep"] {
        assert!(
            is_ask(&decide(tool, json!({})).await),
            "{tool} should return Ask"
        );
    }
}

#[tokio::test]
async fn read_only_bash_is_asked_but_mutating_bash_is_denied() {
    assert!(is_ask(&decide("Bash", json!({"command": "ls -la"})).await));
    assert!(is_ask(
        &decide("Bash", json!({"command": "git status"})).await
    ));
    assert!(is_deny(
        &decide("Bash", json!({"command": "rm -rf /tmp/x"})).await
    ));
    assert!(is_deny(
        &decide("Bash", json!({"command": "echo hi > f"})).await
    ));
    // Missing/empty command is not a read-only command → denied, not a panic.
    assert!(is_deny(&decide("Bash", json!({})).await));
}

#[tokio::test]
async fn every_mutating_or_non_builtin_tool_is_denied() {
    for tool in ["Write", "Edit", "NotebookEdit", "WebFetch", "Task"] {
        assert!(
            is_deny(&decide(tool, json!({})).await),
            "{tool} should be denied"
        );
    }
    // MCP and custom tools are denied.
    assert!(is_deny(&decide("mcp__slack__send", json!({})).await));
    assert!(is_deny(&decide("my_plugin_tool", json!({})).await));
}
