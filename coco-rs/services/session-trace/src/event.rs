//! [`TraceEvent`] — the semantic projection of a [`coco_types::CoreEvent`].
//!
//! A session trace keeps only the execution facts worth replaying: which tools
//! ran (and whether they errored), MCP calls, turn boundaries, and compaction
//! edges. Volatile display detail (text/thinking deltas, TUI overlays) is
//! dropped — those reconstruct from the transcript, and keeping them would make
//! traces enormous and non-deterministic.

use coco_types::AgentStreamEvent;
use coco_types::CoreEvent;
use coco_types::ServerNotification;
use serde::Deserialize;
use serde::Serialize;

/// A durable, replayable execution fact. Internally tagged on `kind` so a
/// [`crate::TraceRecord`] line reads `{"seq":N,"kind":"tool_completed",…}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TraceEvent {
    /// A user-prompt cycle began.
    TurnStarted { turn_id: String },
    /// A user-prompt cycle ended (outcome detail intentionally omitted — the
    /// boundary is the durable fact; outcome lives in the transcript).
    TurnEnded { turn_id: String },
    /// A tool call was received from the model (input complete).
    ToolQueued { call_id: String, name: String },
    /// A tool began executing (post permission check).
    ToolStarted { call_id: String, name: String },
    /// A tool finished.
    ToolCompleted {
        call_id: String,
        name: String,
        is_error: bool,
    },
    /// An MCP tool call started.
    McpToolBegin {
        server: String,
        tool: String,
        call_id: String,
    },
    /// An MCP tool call finished.
    McpToolEnd {
        server: String,
        tool: String,
        call_id: String,
        is_error: bool,
    },
    /// Context compaction began.
    CompactionStarted,
    /// Context was compacted (a compaction edge).
    ContextCompacted,
    /// Context compaction failed.
    CompactionFailed,
}

impl TraceEvent {
    /// Project a [`CoreEvent`] to its durable trace fact, or `None` if the
    /// event carries no semantic meaning worth persisting.
    pub fn from_core_event(event: &CoreEvent) -> Option<Self> {
        match event {
            CoreEvent::Stream(stream) => Self::from_stream(stream),
            CoreEvent::Protocol(notification) => Self::from_protocol(notification),
            // TUI-only events (overlays, toasts, display deltas) are never durable.
            CoreEvent::Tui(_) => None,
        }
    }

    fn from_stream(stream: &AgentStreamEvent) -> Option<Self> {
        match stream {
            AgentStreamEvent::ToolUseQueued { call_id, name, .. } => Some(Self::ToolQueued {
                call_id: call_id.clone(),
                name: name.clone(),
            }),
            AgentStreamEvent::ToolUseStarted { call_id, name, .. } => Some(Self::ToolStarted {
                call_id: call_id.clone(),
                name: name.clone(),
            }),
            AgentStreamEvent::ToolUseCompleted {
                call_id,
                name,
                is_error,
                ..
            } => Some(Self::ToolCompleted {
                call_id: call_id.clone(),
                name: name.clone(),
                is_error: *is_error,
            }),
            AgentStreamEvent::McpToolCallBegin {
                server,
                tool,
                call_id,
            } => Some(Self::McpToolBegin {
                server: server.clone(),
                tool: tool.clone(),
                call_id: call_id.clone(),
            }),
            AgentStreamEvent::McpToolCallEnd {
                server,
                tool,
                call_id,
                is_error,
            } => Some(Self::McpToolEnd {
                server: server.clone(),
                tool: tool.clone(),
                call_id: call_id.clone(),
                is_error: *is_error,
            }),
            // Content deltas reconstruct from the transcript, not the trace.
            AgentStreamEvent::TextDelta { .. } | AgentStreamEvent::ThinkingDelta { .. } => None,
        }
    }

    fn from_protocol(notification: &ServerNotification) -> Option<Self> {
        match notification {
            ServerNotification::TurnStarted(params) => Some(Self::TurnStarted {
                turn_id: params.turn_id.to_string(),
            }),
            ServerNotification::TurnEnded(params) => Some(Self::TurnEnded {
                turn_id: params.turn_id.to_string(),
            }),
            ServerNotification::CompactionStarted => Some(Self::CompactionStarted),
            ServerNotification::ContextCompacted(_) => Some(Self::ContextCompacted),
            ServerNotification::CompactionFailed(_) => Some(Self::CompactionFailed),
            // `ServerNotification` is a large, evolving wire enum; a session trace
            // deliberately keeps only the lifecycle/compaction subset above. A
            // catch-all is correct here — new wire variants are non-durable by
            // default until explicitly projected.
            _ => None,
        }
    }
}

#[cfg(test)]
#[path = "event.test.rs"]
mod tests;
