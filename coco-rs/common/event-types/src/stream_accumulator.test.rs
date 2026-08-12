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
fn response_attempt_publishes_only_on_matching_commit() {
    let mut acc = StreamAccumulator::new("turn-1");
    assert!(
        acc.process(AgentStreamEvent::ResponseAttemptStarted {
            turn_id: "turn-1".into(),
            attempt: 1,
        })
        .is_empty()
    );
    assert!(acc.process(text_delta("hello")).is_empty());
    assert!(
        acc.process(AgentStreamEvent::ResponseAttemptCommitted {
            turn_id: "turn-1".into(),
            attempt: 2,
        })
        .is_empty()
    );
    let published = acc.process(AgentStreamEvent::ResponseAttemptCommitted {
        turn_id: "turn-1".into(),
        attempt: 1,
    });
    assert_eq!(published.len(), 2);
}

#[test]
fn discarded_response_attempt_does_not_flush_content() {
    let mut acc = StreamAccumulator::new("turn-1");
    acc.process(AgentStreamEvent::ResponseAttemptStarted {
        turn_id: "turn-1".into(),
        attempt: 1,
    });
    acc.process(text_delta("malformed"));
    acc.process(AgentStreamEvent::ResponseAttemptDiscarded {
        turn_id: "turn-1".into(),
        attempt: 1,
    });
    assert!(acc.flush().is_empty());
}

/// A tool call proves the attempt is not a malformed terminal (that path
/// requires zero reconstructed tool calls), so the accumulator stops
/// withholding: the buffered text publishes in source order ahead of the tool
/// item, and everything after streams live instead of waiting for commit.
#[test]
fn a_tool_call_publishes_the_buffered_prefix_in_source_order() {
    let mut acc = StreamAccumulator::new("turn-1");
    acc.process(AgentStreamEvent::ResponseAttemptStarted {
        turn_id: "turn-1".into(),
        attempt: 1,
    });
    assert!(acc.process(text_delta("before tool")).is_empty());

    let published = acc.process(tool_queued("call-1", "Bash", json!({"command": "true"})));
    assert!(matches!(
        published.first(),
        Some(ServerNotification::ItemStarted { item, .. })
            if matches!(&item.details, ThreadItemDetails::AgentMessage { .. })
    ));
    assert!(matches!(
        published.last(),
        Some(ServerNotification::ItemStarted { item, .. })
            if matches!(&item.details, ThreadItemDetails::CommandExecution { .. })
    ));

    // Published, so the rest of the attempt no longer waits for commit.
    assert!(!acc.process(text_delta(" and after")).is_empty());
    assert!(
        acc.process(AgentStreamEvent::ResponseAttemptCommitted {
            turn_id: "turn-1".into(),
            attempt: 1,
        })
        .is_empty()
    );
}

/// Discarding a published attempt (only reachable on the stream-error paths,
/// which fire after tool execution may already have happened) replays
/// nothing: its events are already out. Text that preceded real tool
/// execution stays out too — that is the price of live streaming, and the
/// malformed-terminal path this transaction exists for can never get here.
#[test]
fn discarding_a_published_attempt_replays_nothing() {
    let mut acc = StreamAccumulator::new("turn-1");
    acc.process(AgentStreamEvent::ResponseAttemptStarted {
        turn_id: "turn-1".into(),
        attempt: 1,
    });
    acc.process(text_delta("streamed"));
    let published = acc.process(tool_queued("call-1", "Bash", json!({"command": "true"})));
    assert!(
        published.iter().any(|notification| matches!(
            notification,
            ServerNotification::ItemStarted { item, .. }
                if matches!(&item.details, ThreadItemDetails::CommandExecution { .. })
        )),
        "the tool call must publish before the discard: {published:?}"
    );

    assert!(
        acc.process(AgentStreamEvent::ResponseAttemptDiscarded {
            turn_id: "turn-1".into(),
            attempt: 1,
        })
        .is_empty()
    );
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
fn streamed_apply_patch_file_changes_upgrade_the_queued_item() {
    let mut acc = StreamAccumulator::new("turn-1");
    assert!(
        acc.process(AgentStreamEvent::ToolUseInputUpdated {
            call_id: "call-1".into(),
            file_changes: vec![FileChangeInfo {
                path: "src/lib.rs".into(),
                kind: FileChangeKind::Modify,
            }],
        })
        .is_empty()
    );

    let notifications = acc.process(tool_queued(
        "call-1",
        "apply_patch",
        json!({ "patch": "*** Begin Patch" }),
    ));

    let ServerNotification::ItemStarted { item } = &notifications[0] else {
        panic!("expected item start");
    };
    let ThreadItemDetails::FileChange { changes, status } = &item.details else {
        panic!("expected structured file change");
    };
    assert_eq!(*status, ItemStatus::InProgress);
    assert_eq!(changes[0].path, "src/lib.rs");
    assert_eq!(changes[0].kind, FileChangeKind::Modify);
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
