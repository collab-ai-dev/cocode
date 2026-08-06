//! StreamAccumulator — converts `AgentStreamEvent` sequences into semantic
//! `ServerNotification::ItemStarted/Updated/Completed` events with ThreadItem
//! tool mapping.
//!
//! Implemented as an explicit state machine per
//! `event-system-design.md` Section 6.
//!
//! ## State Machine
//!
//! ```text
//! AgentStreamEvent flow:
//!   AttemptStarted → (ThinkingDelta | TextDelta)* → Commit/Discard
//!                            ↓                       ↓
//!                   ordered provisional buffer   publish all / drop text
//!
//!   …unless a tool lifecycle event arrives first: that proves the attempt
//!   is not a malformed terminal, so the buffer publishes in source order
//!   and the rest of the attempt streams live.
//!
//!   Published flow:
//!   ThinkingDelta* → TextDelta* → ToolUseQueued → ToolUseStarted → ToolUseCompleted
//!        ↓               ↓              ↓                ↓                ↓
//!   ItemStarted     ItemStarted    ItemStarted     ItemUpdated      ItemCompleted
//!  (Reasoning)    (AgentMsg)    (tool-specific)
//!   ReasoningDelta  AgentMsgDelta
//! ```
//!
//! Once an attempt commits, text and thinking start a new item on first delta,
//! emit content deltas in source order, and flush to ItemCompleted when:
//! - A new item of a different type starts (text → thinking transition)
//! - A tool use begins
//! - The turn completes (`flush()`)

use std::collections::HashMap;
use std::str::FromStr;

use coco_types::{ToolId, ToolName, TurnId};

use crate::{
    AgentStreamEvent, ContentDeltaParams, FileChangeInfo, FileChangeKind, ItemStatus,
    ServerNotification, ThreadItem, ThreadItemDetails,
};

/// Stateful accumulator that converts `AgentStreamEvent` into
/// `ServerNotification` sequences suitable for SDK consumption.
pub struct StreamAccumulator {
    turn_id: TurnId,
    /// Active thread item ID for text content (if any).
    text_item_id: Option<String>,
    /// Accumulated text buffer for the current text item.
    text_buffer: String,
    /// Active thread item ID for thinking content (if any).
    thinking_item_id: Option<String>,
    /// Accumulated thinking buffer for the current thinking item.
    thinking_buffer: String,
    /// Active tool items keyed by call_id.
    active_items: HashMap<String, ThreadItem>,
    /// Monotonic counter for generating unique item IDs.
    item_counter: i64,
    /// Provider response events are transactional: a matching commit feeds
    /// them into semantic accumulation in source order. Discard keeps real
    /// tool lifecycle events but removes malformed assistant presentation.
    response_attempt: Option<ResponseAttempt>,
}

struct ResponseAttempt {
    attempt: i32,
    /// Events withheld so far, in source order. `None` once the attempt has
    /// published — see [`StreamAccumulator::is_buffering`].
    buffered: Option<Vec<AgentStreamEvent>>,
}

/// Tool lifecycle events describe work that actually ran. They are never
/// "malformed assistant presentation", and observing one proves the attempt
/// cannot take the malformed-terminal discard path (which requires zero
/// reconstructed tool calls).
fn is_tool_lifecycle_event(event: &AgentStreamEvent) -> bool {
    matches!(
        event,
        AgentStreamEvent::ToolUseQueued { .. }
            | AgentStreamEvent::ToolUseStarted { .. }
            | AgentStreamEvent::ToolUseCompleted { .. }
            | AgentStreamEvent::McpToolCallBegin { .. }
            | AgentStreamEvent::McpToolCallEnd { .. }
    )
}

impl StreamAccumulator {
    /// Create a new accumulator scoped to a turn.
    pub fn new(turn_id: impl Into<TurnId>) -> Self {
        Self {
            turn_id: turn_id.into(),
            text_item_id: None,
            text_buffer: String::new(),
            thinking_item_id: None,
            thinking_buffer: String::new(),
            active_items: HashMap::new(),
            item_counter: 0,
            response_attempt: None,
        }
    }

    /// Process a single stream event and return any notifications it produces.
    pub fn process(&mut self, event: AgentStreamEvent) -> Vec<ServerNotification> {
        match event {
            AgentStreamEvent::ResponseAttemptStarted { attempt, .. } => {
                debug_assert!(
                    self.response_attempt.is_none(),
                    "response attempt {attempt} started before the prior attempt reached a terminal boundary"
                );
                self.response_attempt = Some(ResponseAttempt {
                    attempt,
                    buffered: Some(Vec::new()),
                });
                Vec::new()
            }
            AgentStreamEvent::ResponseAttemptCommitted { attempt, .. } => {
                if self
                    .response_attempt
                    .as_ref()
                    .is_none_or(|response| response.attempt != attempt)
                {
                    return Vec::new();
                }
                let Some(response) = self.response_attempt.take() else {
                    return Vec::new();
                };
                response
                    .buffered
                    .unwrap_or_default()
                    .into_iter()
                    .flat_map(|event| self.process(event))
                    .collect()
            }
            AgentStreamEvent::ResponseAttemptDiscarded { attempt, .. } => {
                if self
                    .response_attempt
                    .as_ref()
                    .is_none_or(|response| response.attempt != attempt)
                {
                    return Vec::new();
                }
                let Some(response) = self.response_attempt.take() else {
                    return Vec::new();
                };
                // Tool execution may already have happened while the provider
                // stream was open. Preserve those lifecycle events, in order,
                // while dropping only the malformed assistant presentation.
                response
                    .buffered
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|event| {
                        !matches!(
                            event,
                            AgentStreamEvent::TextDelta { .. }
                                | AgentStreamEvent::ThinkingDelta { .. }
                        )
                    })
                    .flat_map(|event| self.process(event))
                    .collect()
            }
            event if self.is_buffering() => {
                if !is_tool_lifecycle_event(&event) {
                    if let Some(response) = self.response_attempt.as_mut()
                        && let Some(buffered) = response.buffered.as_mut()
                    {
                        buffered.push(event);
                    }
                    return Vec::new();
                }
                // The attempt is committable: stop withholding. Everything
                // buffered so far publishes here, in source order, so item
                // ordering still matches the provider stream — then the rest
                // of the attempt streams live. Without this, a whole response
                // (including tools that already ran) would land in one burst
                // at commit and SDK consumers would lose incremental output.
                let buffered = self
                    .response_attempt
                    .as_mut()
                    .and_then(|response| response.buffered.take())
                    .unwrap_or_default();
                let mut out: Vec<ServerNotification> = buffered
                    .into_iter()
                    .flat_map(|event| self.process(event))
                    .collect();
                out.extend(self.process(event));
                out
            }
            AgentStreamEvent::TextDelta { delta, .. } => self.handle_text_delta(delta),
            AgentStreamEvent::ThinkingDelta { delta, .. } => self.handle_thinking_delta(delta),
            AgentStreamEvent::ToolUseQueued {
                call_id,
                name,
                input,
            } => self.handle_tool_queued(call_id, name, input),
            AgentStreamEvent::ToolUseStarted { call_id, .. } => self.handle_tool_started(call_id),
            AgentStreamEvent::ToolUseCompleted {
                call_id,
                name: _,
                output,
                is_error,
            } => self.handle_tool_completed(call_id, output, is_error),
            AgentStreamEvent::McpToolCallBegin {
                server,
                tool,
                call_id,
            } => self.handle_mcp_begin(server, tool, call_id),
            AgentStreamEvent::McpToolCallEnd {
                server,
                tool,
                call_id,
                is_error,
            } => self.handle_mcp_end(server, tool, call_id, is_error),
        }
    }

    /// Whether an open attempt is still withholding its events.
    fn is_buffering(&self) -> bool {
        self.response_attempt
            .as_ref()
            .is_some_and(|response| response.buffered.is_some())
    }

    /// Flush any pending text/thinking items and return completion notifications.
    /// Call at turn end.
    pub fn flush(&mut self) -> Vec<ServerNotification> {
        self.response_attempt = None;
        let mut out = Vec::new();
        out.extend(self.flush_text());
        out.extend(self.flush_thinking());
        out
    }

    // ---------------------------------------------------------------
    // Text handling
    // ---------------------------------------------------------------

    fn handle_text_delta(&mut self, delta: String) -> Vec<ServerNotification> {
        let mut out = Vec::new();
        // Starting text → flush any active thinking item first.
        if self.thinking_item_id.is_some() {
            out.extend(self.flush_thinking());
        }

        // Start a new text item on first delta.
        if self.text_item_id.is_none() {
            let id = self.next_item_id("agent-msg");
            self.text_item_id = Some(id.clone());
            out.push(ServerNotification::ItemStarted {
                item: ThreadItem {
                    item_id: id,
                    turn_id: self.turn_id.clone(),
                    details: ThreadItemDetails::AgentMessage {
                        text: String::new(),
                    },
                },
            });
        }

        self.text_buffer.push_str(&delta);

        // Emit AgentMessageDelta for incremental text streaming.
        if let Some(id) = &self.text_item_id {
            out.push(ServerNotification::AgentMessageDelta(ContentDeltaParams {
                item_id: Some(id.clone()),
                turn_id: Some(self.turn_id.clone()),
                delta,
            }));
        }

        out
    }

    fn flush_text(&mut self) -> Vec<ServerNotification> {
        if let Some(id) = self.text_item_id.take() {
            let text = std::mem::take(&mut self.text_buffer);
            vec![ServerNotification::ItemCompleted {
                item: ThreadItem {
                    item_id: id,
                    turn_id: self.turn_id.clone(),
                    details: ThreadItemDetails::AgentMessage { text },
                },
            }]
        } else {
            Vec::new()
        }
    }

    // ---------------------------------------------------------------
    // Thinking handling
    // ---------------------------------------------------------------

    fn handle_thinking_delta(&mut self, delta: String) -> Vec<ServerNotification> {
        let mut out = Vec::new();
        // Starting thinking → flush any active text item first.
        if self.text_item_id.is_some() {
            out.extend(self.flush_text());
        }

        if self.thinking_item_id.is_none() {
            let id = self.next_item_id("reasoning");
            self.thinking_item_id = Some(id.clone());
            out.push(ServerNotification::ItemStarted {
                item: ThreadItem {
                    item_id: id,
                    turn_id: self.turn_id.clone(),
                    details: ThreadItemDetails::Reasoning {
                        text: String::new(),
                    },
                },
            });
        }

        self.thinking_buffer.push_str(&delta);

        if let Some(id) = &self.thinking_item_id {
            out.push(ServerNotification::ReasoningDelta(ContentDeltaParams {
                item_id: Some(id.clone()),
                turn_id: Some(self.turn_id.clone()),
                delta,
            }));
        }

        out
    }

    fn flush_thinking(&mut self) -> Vec<ServerNotification> {
        if let Some(id) = self.thinking_item_id.take() {
            let text = std::mem::take(&mut self.thinking_buffer);
            vec![ServerNotification::ItemCompleted {
                item: ThreadItem {
                    item_id: id,
                    turn_id: self.turn_id.clone(),
                    details: ThreadItemDetails::Reasoning { text },
                },
            }]
        } else {
            Vec::new()
        }
    }

    // ---------------------------------------------------------------
    // Tool handling
    // ---------------------------------------------------------------

    fn handle_tool_queued(
        &mut self,
        call_id: String,
        name: String,
        input: serde_json::Value,
    ) -> Vec<ServerNotification> {
        // Starting a tool → flush any active text/thinking first.
        let mut out = Vec::new();
        out.extend(self.flush_text());
        out.extend(self.flush_thinking());

        let details = build_tool_details(&name, &input, ItemStatus::InProgress);
        let item = ThreadItem {
            item_id: call_id.clone(),
            turn_id: self.turn_id.clone(),
            details,
        };
        self.active_items.insert(call_id, item.clone());
        out.push(ServerNotification::ItemStarted { item });
        out
    }

    fn handle_tool_started(&mut self, call_id: String) -> Vec<ServerNotification> {
        // Emit ItemUpdated if the tool is tracked.
        if let Some(item) = self.active_items.get(&call_id) {
            vec![ServerNotification::ItemUpdated { item: item.clone() }]
        } else {
            Vec::new()
        }
    }

    fn handle_tool_completed(
        &mut self,
        call_id: String,
        output: String,
        is_error: bool,
    ) -> Vec<ServerNotification> {
        if let Some(mut item) = self.active_items.remove(&call_id) {
            apply_tool_completion(&mut item.details, output, is_error);
            vec![ServerNotification::ItemCompleted { item }]
        } else {
            Vec::new()
        }
    }

    // ---------------------------------------------------------------
    // MCP handling
    // ---------------------------------------------------------------

    fn handle_mcp_begin(
        &mut self,
        server: String,
        tool: String,
        call_id: String,
    ) -> Vec<ServerNotification> {
        let mut out = Vec::new();
        out.extend(self.flush_text());
        out.extend(self.flush_thinking());

        let item = ThreadItem {
            item_id: call_id.clone(),
            turn_id: self.turn_id.clone(),
            details: ThreadItemDetails::McpToolCall {
                server,
                tool,
                arguments: serde_json::Value::Null,
                result: None,
                error: None,
                status: ItemStatus::InProgress,
            },
        };
        self.active_items.insert(call_id, item.clone());
        out.push(ServerNotification::ItemStarted { item });
        out
    }

    fn handle_mcp_end(
        &mut self,
        _server: String,
        _tool: String,
        call_id: String,
        is_error: bool,
    ) -> Vec<ServerNotification> {
        if let Some(mut item) = self.active_items.remove(&call_id) {
            if let ThreadItemDetails::McpToolCall { status, .. } = &mut item.details {
                *status = if is_error {
                    ItemStatus::Failed
                } else {
                    ItemStatus::Completed
                };
            }
            vec![ServerNotification::ItemCompleted { item }]
        } else {
            Vec::new()
        }
    }

    // ---------------------------------------------------------------

    fn next_item_id(&mut self, prefix: &str) -> String {
        self.item_counter += 1;
        format!("{}-{}-{}", self.turn_id, prefix, self.item_counter)
    }
}

// ---------------------------------------------------------------
// Tool mapping (see event-system-design.md Section 6.2)
// ---------------------------------------------------------------

/// Build the initial `ThreadItemDetails` for a tool call.
///
/// Mapping:
/// - Bash → `CommandExecution`
/// - Edit/Write/NotebookEdit → `FileChange`
/// - WebSearch → `WebSearch`
/// - mcp__* → `McpToolCall`
/// - Agent/Task → `Subagent`
/// - all others → `ToolCall`
fn build_tool_details(
    tool_name: &str,
    input: &serde_json::Value,
    status: ItemStatus,
) -> ThreadItemDetails {
    // ToolId::from_str is infallible and handles MCP parsing + builtin lookup.
    let tool_id = ToolId::from_str(tool_name).expect("ToolId::from_str is infallible");

    if let ToolId::Mcp { server, tool } = tool_id {
        return ThreadItemDetails::McpToolCall {
            server,
            tool,
            arguments: input.clone(),
            result: None,
            error: None,
            status,
        };
    }

    let str_field = |key: &str| -> String {
        input
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };

    let builtin = match &tool_id {
        ToolId::Builtin(name) => Some(*name),
        _ => None,
    };

    match builtin {
        Some(ToolName::Bash | ToolName::PowerShell) => ThreadItemDetails::CommandExecution {
            command: str_field("command"),
            output: String::new(),
            exit_code: None,
            status,
        },
        Some(name @ (ToolName::Edit | ToolName::Write | ToolName::NotebookEdit)) => {
            let kind = if name == ToolName::Write {
                FileChangeKind::Create
            } else {
                FileChangeKind::Modify
            };
            ThreadItemDetails::FileChange {
                changes: vec![FileChangeInfo {
                    path: str_field("file_path"),
                    kind,
                }],
                status,
            }
        }
        Some(ToolName::WebSearch) => ThreadItemDetails::WebSearch {
            query: str_field("query"),
            status,
        },
        Some(ToolName::Agent) => ThreadItemDetails::Subagent {
            agent_id: None,
            agent_type: input
                .get("subagent_type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("general")
                .to_string(),
            description: str_field("description"),
            is_background: input
                .get("background")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            result: None,
            status,
        },
        _ => ThreadItemDetails::ToolCall {
            tool: tool_name.to_string(),
            input: input.clone(),
            output: None,
            is_error: false,
            status,
        },
    }
}

/// Apply tool completion output to the item details.
fn apply_tool_completion(details: &mut ThreadItemDetails, output: String, is_error: bool) {
    let final_status = if is_error {
        ItemStatus::Failed
    } else {
        ItemStatus::Completed
    };

    match details {
        ThreadItemDetails::CommandExecution {
            output: out,
            status,
            ..
        } => {
            *out = output;
            *status = final_status;
        }
        ThreadItemDetails::FileChange { status, .. } => {
            *status = final_status;
        }
        ThreadItemDetails::WebSearch { status, .. } => {
            *status = final_status;
        }
        ThreadItemDetails::McpToolCall {
            result,
            error,
            status,
            ..
        } => {
            if is_error {
                *error = Some(output);
            } else {
                *result = Some(output);
            }
            *status = final_status;
        }
        ThreadItemDetails::Subagent { result, status, .. } => {
            *result = Some(output);
            *status = final_status;
        }
        ThreadItemDetails::ToolCall {
            output: out,
            is_error: err,
            status,
            ..
        } => {
            *out = Some(output);
            *err = is_error;
            *status = final_status;
        }
        ThreadItemDetails::AgentMessage { .. }
        | ThreadItemDetails::Reasoning { .. }
        | ThreadItemDetails::Error { .. } => {
            // Non-tool items never receive completion from tool events.
        }
    }
}

#[cfg(test)]
#[path = "stream_accumulator.test.rs"]
mod tests;
