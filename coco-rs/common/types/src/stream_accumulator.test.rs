use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

fn text_delta(delta: &str) -> AgentStreamEvent {
    AgentStreamEvent::TextDelta {
        turn_id: "turn-1".into(),
        delta: delta.into(),
    }
}

fn thinking_delta(delta: &str) -> AgentStreamEvent {
    AgentStreamEvent::ThinkingDelta {
        turn_id: "turn-1".into(),
        delta: delta.into(),
    }
}

fn tool_queued(call_id: &str, name: &str, input: serde_json::Value) -> AgentStreamEvent {
    AgentStreamEvent::ToolUseQueued {
        call_id: call_id.into(),
        name: name.into(),
        input,
    }
}

fn tool_completed(call_id: &str, output: &str, is_error: bool) -> AgentStreamEvent {
    AgentStreamEvent::ToolUseCompleted {
        call_id: call_id.into(),
        name: "Bash".into(),
        output: output.into(),
        is_error,
    }
}

#[test]
fn text_delta_starts_item_and_emits_delta() {
    let mut acc = StreamAccumulator::new("turn-1");
    let notifs = acc.process(text_delta("hello"));
    assert_eq!(notifs.len(), 2);
    assert!(matches!(&notifs[0], ServerNotification::ItemStarted { .. }));
    assert!(matches!(
        &notifs[1],
        ServerNotification::AgentMessageDelta(_)
    ));
}

#[test]
fn consecutive_text_deltas_share_item() {
    let mut acc = StreamAccumulator::new("turn-1");
    let _ = acc.process(text_delta("hello "));
    let second = acc.process(text_delta("world"));
    // Second delta should only produce AgentMessageDelta, no new ItemStarted.
    assert_eq!(second.len(), 1);
    assert!(matches!(
        &second[0],
        ServerNotification::AgentMessageDelta(_)
    ));
}

#[test]
fn flush_emits_item_completed_for_text() {
    let mut acc = StreamAccumulator::new("turn-1");
    let _ = acc.process(text_delta("hello"));
    let done = acc.flush();
    assert_eq!(done.len(), 1);
    match &done[0] {
        ServerNotification::ItemCompleted { item } => match &item.details {
            ThreadItemDetails::AgentMessage { text } => assert_eq!(text, "hello"),
            _ => panic!("expected AgentMessage"),
        },
        _ => panic!("expected ItemCompleted"),
    }
}

#[test]
fn text_to_thinking_transition_flushes_text() {
    let mut acc = StreamAccumulator::new("turn-1");
    let _ = acc.process(text_delta("hi"));
    let notifs = acc.process(thinking_delta("thinking..."));
    // Should flush text (ItemCompleted) then start thinking (ItemStarted + delta).
    assert_eq!(notifs.len(), 3);
    assert!(matches!(
        &notifs[0],
        ServerNotification::ItemCompleted { .. }
    ));
    assert!(matches!(&notifs[1], ServerNotification::ItemStarted { .. }));
    assert!(matches!(&notifs[2], ServerNotification::ReasoningDelta(_)));
}

#[test]
fn bash_tool_maps_to_command_execution() {
    let mut acc = StreamAccumulator::new("turn-1");
    let notifs = acc.process(tool_queued(
        "call-1",
        "Bash",
        json!({ "command": "ls -la" }),
    ));
    assert_eq!(notifs.len(), 1);
    match &notifs[0] {
        ServerNotification::ItemStarted { item } => match &item.details {
            ThreadItemDetails::CommandExecution {
                command, status, ..
            } => {
                assert_eq!(command, "ls -la");
                assert_eq!(*status, ItemStatus::InProgress);
            }
            _ => panic!("expected CommandExecution"),
        },
        _ => panic!("expected ItemStarted"),
    }
}

#[test]
fn bash_completion_fills_output() {
    let mut acc = StreamAccumulator::new("turn-1");
    let _ = acc.process(tool_queued("call-1", "Bash", json!({ "command": "ls" })));
    let notifs = acc.process(tool_completed("call-1", "file1\nfile2", false));
    assert_eq!(notifs.len(), 1);
    match &notifs[0] {
        ServerNotification::ItemCompleted { item } => match &item.details {
            ThreadItemDetails::CommandExecution { output, status, .. } => {
                assert_eq!(output, "file1\nfile2");
                assert_eq!(*status, ItemStatus::Completed);
            }
            _ => panic!("expected CommandExecution"),
        },
        _ => panic!("expected ItemCompleted"),
    }
}

#[test]
fn edit_tool_maps_to_file_change() {
    let mut acc = StreamAccumulator::new("turn-1");
    let notifs = acc.process(tool_queued(
        "call-1",
        "Edit",
        json!({ "file_path": "src/main.rs" }),
    ));
    match &notifs[0] {
        ServerNotification::ItemStarted { item } => match &item.details {
            ThreadItemDetails::FileChange { changes, .. } => {
                assert_eq!(changes[0].path, "src/main.rs");
                assert_eq!(changes[0].kind, FileChangeKind::Modify);
            }
            _ => panic!("expected FileChange"),
        },
        _ => panic!("expected ItemStarted"),
    }
}

#[test]
fn write_tool_uses_create_kind() {
    let mut acc = StreamAccumulator::new("turn-1");
    let notifs = acc.process(tool_queued(
        "call-1",
        "Write",
        json!({ "file_path": "new.rs" }),
    ));
    match &notifs[0] {
        ServerNotification::ItemStarted { item } => match &item.details {
            ThreadItemDetails::FileChange { changes, .. } => {
                assert_eq!(changes[0].kind, FileChangeKind::Create);
            }
            _ => panic!("expected FileChange"),
        },
        _ => panic!("expected ItemStarted"),
    }
}

#[test]
fn web_search_tool_maps_correctly() {
    let mut acc = StreamAccumulator::new("turn-1");
    let notifs = acc.process(tool_queued(
        "call-1",
        "WebSearch",
        json!({ "query": "rust async" }),
    ));
    match &notifs[0] {
        ServerNotification::ItemStarted { item } => match &item.details {
            ThreadItemDetails::WebSearch { query, .. } => {
                assert_eq!(query, "rust async");
            }
            _ => panic!("expected WebSearch"),
        },
        _ => panic!("expected ItemStarted"),
    }
}

#[test]
fn mcp_tool_name_parses_server_and_tool() {
    let mut acc = StreamAccumulator::new("turn-1");
    let notifs = acc.process(tool_queued(
        "call-1",
        "mcp__github__create_pr",
        json!({ "title": "fix" }),
    ));
    match &notifs[0] {
        ServerNotification::ItemStarted { item } => match &item.details {
            ThreadItemDetails::McpToolCall { server, tool, .. } => {
                assert_eq!(server, "github");
                assert_eq!(tool, "create_pr");
            }
            _ => panic!("expected McpToolCall"),
        },
        _ => panic!("expected ItemStarted"),
    }
}

#[test]
fn agent_tool_maps_to_subagent() {
    let mut acc = StreamAccumulator::new("turn-1");
    let notifs = acc.process(tool_queued(
        "call-1",
        "Agent",
        json!({ "description": "do something", "subagent_type": "researcher" }),
    ));
    match &notifs[0] {
        ServerNotification::ItemStarted { item } => match &item.details {
            ThreadItemDetails::Subagent {
                description,
                agent_type,
                ..
            } => {
                assert_eq!(description, "do something");
                assert_eq!(agent_type, "researcher");
            }
            _ => panic!("expected Subagent"),
        },
        _ => panic!("expected ItemStarted"),
    }
}

#[test]
fn unknown_tool_maps_to_tool_call() {
    let mut acc = StreamAccumulator::new("turn-1");
    let notifs = acc.process(tool_queued(
        "call-1",
        "Read",
        json!({ "file_path": "README.md" }),
    ));
    match &notifs[0] {
        ServerNotification::ItemStarted { item } => match &item.details {
            ThreadItemDetails::ToolCall { tool, .. } => {
                assert_eq!(tool, "Read");
            }
            _ => panic!("expected ToolCall"),
        },
        _ => panic!("expected ItemStarted"),
    }
}

#[test]
fn tool_queued_flushes_pending_text() {
    let mut acc = StreamAccumulator::new("turn-1");
    let _ = acc.process(text_delta("running tool"));
    let notifs = acc.process(tool_queued("call-1", "Bash", json!({ "command": "ls" })));
    // Should flush text (ItemCompleted) then start tool (ItemStarted).
    assert_eq!(notifs.len(), 2);
    assert!(matches!(
        &notifs[0],
        ServerNotification::ItemCompleted { .. }
    ));
    assert!(matches!(&notifs[1], ServerNotification::ItemStarted { .. }));
}

#[test]
fn tool_error_marks_failed_status() {
    let mut acc = StreamAccumulator::new("turn-1");
    let _ = acc.process(tool_queued("call-1", "Bash", json!({ "command": "false" })));
    let notifs = acc.process(tool_completed("call-1", "exit 1", true));
    match &notifs[0] {
        ServerNotification::ItemCompleted { item } => match &item.details {
            ThreadItemDetails::CommandExecution { status, .. } => {
                assert_eq!(*status, ItemStatus::Failed);
            }
            _ => panic!("expected CommandExecution"),
        },
        _ => panic!("expected ItemCompleted"),
    }
}

#[test]
fn flush_with_no_active_items_returns_empty() {
    let mut acc = StreamAccumulator::new("turn-1");
    assert!(acc.flush().is_empty());
}
