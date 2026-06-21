//! Stream-layer handler.
//!
//! Handles [`AgentStreamEvent`] — raw agent-loop stream deltas (text,
//! thinking, tool use lifecycle, MCP tool call lifecycle). The TUI consumes
//! these directly for real-time display; the SDK dispatcher routes them
//! through `StreamAccumulator` separately.
//!
//! Per-layer defensive invariant: if a stream delta arrives before
//! `TurnStarted` has created `state.ui.streaming`, the handler lazily
//! creates the streaming state so deltas are never silently dropped. See
//! `server_notification_handler.test.rs` for the regression test.

use coco_types::AgentStreamEvent;
use coco_types::MCP_TOOL_PREFIX;
use coco_types::MCP_TOOL_SEPARATOR;

use super::projection::flush_streaming_to_messages;
use crate::state::AppState;

pub(super) fn handle(state: &mut AppState, event: AgentStreamEvent) -> bool {
    match event {
        AgentStreamEvent::TextDelta { delta, .. } => {
            state.ui.ephemeral.add_output_delta(&delta);
            // Defensive: in normal flow TurnStarted creates the streaming
            // state before any delta arrives. If a delta lands first (e.g.
            // channel reordering between engine emit sites, or a unit test
            // driving this handler directly), fall back to lazy creation
            // so the content isn't silently dropped.
            state
                .ui
                .streaming
                .get_or_insert_with(crate::state::ui::StreamingState::new)
                .append_text(&delta);
            true
        }
        AgentStreamEvent::ThinkingDelta { delta, .. } => {
            state
                .ui
                .streaming
                .get_or_insert_with(crate::state::ui::StreamingState::new)
                .append_thinking(&delta);
            true
        }
        AgentStreamEvent::ToolUseQueued {
            call_id,
            name,
            input,
        } => {
            // Clear the live streaming overlay; the engine's
            // Message::Assistant push (with ToolCall content blocks)
            // will appear via transcript when the turn finalizes.
            flush_streaming_to_messages(state);
            state.session.start_tool(call_id, name, &input);
            true
        }
        AgentStreamEvent::ToolUseStarted { call_id, .. } => {
            state.session.run_tool(&call_id);
            true
        }
        AgentStreamEvent::ToolUseCompleted {
            call_id,
            name: _,
            output: _,
            is_error,
        } => {
            state.session.complete_tool(&call_id, is_error);
            // Engine pushes Message::ToolResult → MessageAppended →
            // transcript; the renderer surfaces it from there.
            true
        }
        AgentStreamEvent::McpToolCallBegin {
            server,
            tool,
            call_id,
        } => {
            // Only create if not already tracked via ToolUseQueued (avoids
            // duplicate for MCP tools that go through the regular pipeline).
            let already_tracked = state
                .session
                .tool_executions
                .iter()
                .any(|t| t.call_id == call_id);
            if !already_tracked {
                // `McpToolCallBegin` carries no arguments (those ride the
                // regular `ToolUseQueued` path); fall back to a name-only row.
                state.session.start_tool(
                    call_id,
                    format!("{MCP_TOOL_PREFIX}{server}{MCP_TOOL_SEPARATOR}{tool}"),
                    &serde_json::Value::Null,
                );
            }
            true
        }
        AgentStreamEvent::McpToolCallEnd {
            call_id, is_error, ..
        } => {
            state.session.complete_tool(&call_id, is_error);
            true
        }
    }
}
