//! CLI output formatting for non-TUI mode.

use coco_messages::AssistantContent;
use coco_messages::LlmMessage;
use coco_messages::Message;
use coco_types::TokenUsage;

/// Output mode for CLI rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    /// Human-readable text output (default).
    Text,
    /// NDJSON structured output (SDK mode).
    Json,
}

/// Render a message for CLI output.
pub fn render_message(msg: &Message, mode: OutputMode) -> String {
    match mode {
        OutputMode::Json => render_json(msg),
        OutputMode::Text => render_text(msg),
    }
}

/// Render as human-readable text.
fn render_text(msg: &Message) -> String {
    match msg {
        Message::User(u) => {
            // Post-Phase-2: all `Message::User` are genuine human input.
            // Meta / reminder text lives in `Message::Attachment`.
            // Slash-command echo/result rows carry command tags — strip
            // them so text mode shows `> /cmd` / the result, not raw XML.
            let text = extract_text(&u.message);
            if coco_messages::is_command_input(&text) {
                let name = coco_messages::extract_tag(&text, coco_messages::COMMAND_NAME_TAG)
                    .unwrap_or("");
                let args = coco_messages::extract_tag(&text, coco_messages::COMMAND_ARGS_TAG)
                    .unwrap_or("");
                let echo = if args.is_empty() {
                    name.to_string()
                } else {
                    format!("{name} {args}")
                };
                return format!("\x1b[1;36m> {echo}\x1b[0m\n");
            }
            if coco_messages::is_local_command_output(&text) {
                let body =
                    coco_messages::extract_tag(&text, coco_messages::LOCAL_COMMAND_STDOUT_TAG)
                        .or_else(|| {
                            coco_messages::extract_tag(
                                &text,
                                coco_messages::LOCAL_COMMAND_STDERR_TAG,
                            )
                        })
                        .unwrap_or("");
                return format!("\x1b[2m  {body}\x1b[0m\n");
            }
            format!("\x1b[1;36m> {text}\x1b[0m\n")
        }
        Message::Assistant(a) => {
            let text = extract_text(&a.message);
            let tool_calls = extract_tool_calls(&a.message);
            let mut output = text;
            for (name, _input) in &tool_calls {
                output.push_str(&format!("\n\x1b[2m⏳ {name}...\x1b[0m"));
            }
            output
        }
        Message::ToolResult(t) => {
            let text = extract_text(&t.message);
            if t.is_error {
                format!("\x1b[31m✗ Error: {text}\x1b[0m\n")
            } else if text.len() > 500 {
                format!("{}...\n", &text[..500])
            } else {
                format!("{text}\n")
            }
        }
        Message::System(coco_messages::SystemMessage::Informational(m)) => {
            format!("\x1b[2m{}\x1b[0m\n", m.message)
        }
        _ => String::new(),
    }
}

/// Render as NDJSON for SDK protocol.
fn render_json(msg: &Message) -> String {
    serde_json::to_string(msg).unwrap_or_default()
}

/// Extract text content from an LlmMessage.
fn extract_text(msg: &LlmMessage) -> String {
    match msg {
        LlmMessage::User { content, .. } => content
            .iter()
            .filter_map(|c| match c {
                coco_messages::UserContent::Text(t) => Some(t.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        LlmMessage::Assistant { content, .. } => content
            .iter()
            .filter_map(|c| match c {
                AssistantContent::Text(t) => Some(t.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        LlmMessage::System { content, .. } | LlmMessage::Developer { content, .. } => content
            .iter()
            .filter_map(|c| match c {
                coco_messages::UserContent::Text(t) => Some(t.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        LlmMessage::Tool { .. } => String::new(),
    }
}

/// Extract tool calls from an assistant message.
fn extract_tool_calls(msg: &LlmMessage) -> Vec<(String, serde_json::Value)> {
    match msg {
        LlmMessage::Assistant { content, .. } => content
            .iter()
            .filter_map(|c| match c {
                AssistantContent::ToolCall(tc) => Some((tc.tool_name.clone(), tc.input.clone())),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Format token usage as a status line.
pub fn format_usage(usage: &TokenUsage, cost_usd: f64) -> String {
    format!(
        "Tokens: {}↓ {}↑ | Cost: ${cost_usd:.4}",
        usage.input_tokens.total, usage.output_tokens.total
    )
}

/// Format a turn summary line.
pub fn format_turn_summary(turn: i32, tool_count: i32, duration_ms: i64) -> String {
    let secs = duration_ms as f64 / 1000.0;
    if tool_count > 0 {
        format!("Turn {turn} ({tool_count} tools, {secs:.1}s)")
    } else {
        format!("Turn {turn} ({secs:.1}s)")
    }
}

/// NDJSON message types for SDK protocol.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SdkMessage {
    /// Assistant text response.
    AssistantMessage { text: String },
    /// Tool execution started.
    ToolUseStart {
        tool_use_id: String,
        tool_name: String,
    },
    /// Tool execution completed.
    ToolUseEnd {
        tool_use_id: String,
        tool_name: String,
        is_error: bool,
    },
    /// System information.
    SystemInfo { message: String },
    /// Session metadata.
    SessionMeta {
        session_id: String,
        model: String,
        cwd: String,
    },
    /// Usage statistics.
    Usage {
        input_tokens: i64,
        output_tokens: i64,
        cost_usd: f64,
    },
    /// Result (final response).
    Result { text: String, turns: i32 },
}

/// Write an SDK message to stdout as NDJSON.
pub fn write_sdk_message(msg: &SdkMessage) {
    if let Ok(json) = serde_json::to_string(msg) {
        println!("{json}");
    }
}

#[cfg(test)]
#[path = "output.test.rs"]
mod tests;
