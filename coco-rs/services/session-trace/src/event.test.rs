use super::*;
use coco_types::AgentStreamEvent;
use coco_types::CoreEvent;
use coco_types::ServerNotification;
use coco_types::TurnId;
use coco_types::TurnStartedParams;
use pretty_assertions::assert_eq;

#[test]
fn test_tool_completed_projects_with_error_flag() {
    let event = CoreEvent::Stream(AgentStreamEvent::ToolUseCompleted {
        call_id: "c1".to_string(),
        name: "Bash".to_string(),
        output: "boom".to_string(),
        is_error: true,
    });
    assert_eq!(
        TraceEvent::from_core_event(&event),
        Some(TraceEvent::ToolCompleted {
            call_id: "c1".to_string(),
            name: "Bash".to_string(),
            is_error: true,
        })
    );
}

#[test]
fn test_mcp_call_projects_server_and_tool() {
    let event = CoreEvent::Stream(AgentStreamEvent::McpToolCallBegin {
        server: "fs".to_string(),
        tool: "read".to_string(),
        call_id: "m1".to_string(),
    });
    assert_eq!(
        TraceEvent::from_core_event(&event),
        Some(TraceEvent::McpToolBegin {
            server: "fs".to_string(),
            tool: "read".to_string(),
            call_id: "m1".to_string(),
        })
    );
}

#[test]
fn test_text_delta_is_not_durable() {
    let event = CoreEvent::Stream(AgentStreamEvent::TextDelta {
        turn_id: TurnId::from("t"),
        delta: "hi".to_string(),
    });
    assert_eq!(TraceEvent::from_core_event(&event), None);
}

#[test]
fn test_compaction_started_projects() {
    let event = CoreEvent::Protocol(ServerNotification::CompactionStarted);
    assert_eq!(
        TraceEvent::from_core_event(&event),
        Some(TraceEvent::CompactionStarted)
    );
}

#[test]
fn test_turn_started_projects_turn_id() {
    let event = CoreEvent::Protocol(ServerNotification::TurnStarted(TurnStartedParams {
        turn_id: TurnId::from("turn-1"),
    }));
    assert_eq!(
        TraceEvent::from_core_event(&event),
        Some(TraceEvent::TurnStarted {
            turn_id: TurnId::from("turn-1"),
        })
    );
}
