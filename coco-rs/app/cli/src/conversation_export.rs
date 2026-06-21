//! `/export` conversation rendering — turn the live `MessageHistory` into a
//! Markdown / JSON / plain-text document. Mirrors TS `utils/exportRenderer.tsx`
//! (`renderMessagesToPlainText`): the full transcript including tool activity,
//! since an agentic session is mostly tool use. The file-writing + cwd
//! resolution lives in `tui_runner::run_export` (it needs the runtime).

use std::sync::Arc;

/// Export output format, inferred from the target filename's extension.
#[derive(Clone, Copy)]
pub enum ExportFormat {
    Markdown,
    Json,
    Text,
}

impl ExportFormat {
    /// Infer from a filename extension; unknown/absent → plain text.
    pub fn from_filename(name: &str) -> Self {
        match name
            .rsplit_once('.')
            .map(|(_, ext)| ext.to_ascii_lowercase())
        {
            Some(ext) if ext == "md" || ext == "markdown" => Self::Markdown,
            Some(ext) if ext == "json" => Self::Json,
            _ => Self::Text,
        }
    }

    /// The bare format keywords the no-arg modal dispatches.
    pub fn from_keyword(arg: &str) -> Option<Self> {
        match arg {
            "markdown" => Some(Self::Markdown),
            "json" => Some(Self::Json),
            "text" => Some(Self::Text),
            _ => None,
        }
    }

    pub fn ext(self) -> &'static str {
        match self {
            Self::Markdown => "md",
            Self::Json => "json",
            Self::Text => "txt",
        }
    }

    pub fn render(self, messages: &[Arc<coco_messages::Message>]) -> String {
        match self {
            Self::Markdown => render_conversation_markdown(messages),
            Self::Json => render_conversation_json(messages),
            Self::Text => render_conversation_text(messages),
        }
    }
}

/// One rendered entry of the conversation export: a role/label and its body.
struct ExportEntry {
    label: String,
    body: String,
}

/// Walk the conversation into export entries — user/assistant/system text,
/// assistant tool CALLS (name + input), and tool RESULTS — mirroring TS
/// export, which renders the full transcript including tool activity.
/// Progress/tombstone messages and empty bodies are skipped.
fn conversation_entries(messages: &[Arc<coco_messages::Message>]) -> Vec<ExportEntry> {
    let mut out = Vec::new();
    let mut push = |label: String, body: &str| {
        let body = body.trim();
        if !body.is_empty() {
            out.push(ExportEntry {
                label,
                body: body.to_string(),
            });
        }
    };
    for m in messages {
        match m.as_ref() {
            coco_messages::Message::User(_) => {
                push(
                    "User".to_string(),
                    &coco_messages::wrapping::extract_text_from_message(m),
                );
            }
            coco_messages::Message::Assistant(a) => {
                push(
                    "Assistant".to_string(),
                    &coco_messages::wrapping::extract_text_from_message(m),
                );
                if let coco_messages::LlmMessage::Assistant { content, .. } = &a.message {
                    for part in content {
                        if let coco_messages::AssistantContent::ToolCall(tc) = part {
                            push(
                                format!("Tool call · {}", tc.tool_name),
                                &tc.input.to_string(),
                            );
                        }
                    }
                }
            }
            coco_messages::Message::ToolResult(_) => {
                if let Some((tool_name, output)) = tool_result_text(m) {
                    push(format!("Tool result · {tool_name}"), &output);
                }
            }
            coco_messages::Message::System(_) => {
                push(
                    "System".to_string(),
                    &coco_messages::wrapping::extract_text_from_message(m),
                );
            }
            _ => {}
        }
    }
    out
}

/// Concatenate a `Message::ToolResult`'s output to plain text + its tool name.
/// Mirrors `coco_tui::transcript::derive::tool_result_output` (which is
/// crate-private to the TUI) over the `ToolResultOutput` variants.
fn tool_result_text(msg: &Arc<coco_messages::Message>) -> Option<(String, String)> {
    use coco_messages::ToolContent;
    use coco_messages::ToolResultContentPart;
    use coco_messages::ToolResultOutput;
    let coco_messages::Message::ToolResult(tr) = msg.as_ref() else {
        return None;
    };
    let coco_messages::LlmMessage::Tool { content, .. } = &tr.message else {
        return None;
    };
    let part = content.iter().find_map(|p| match p {
        ToolContent::ToolResult(part) => Some(part),
        _ => None,
    })?;
    let output = match &part.output {
        ToolResultOutput::Text { value, .. } => value.clone(),
        ToolResultOutput::Json { value, .. } => value.to_string(),
        ToolResultOutput::Content { value, .. } => value
            .iter()
            .filter_map(|p| match p {
                ToolResultContentPart::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        ToolResultOutput::ErrorText { value, .. } => value.clone(),
        ToolResultOutput::ErrorJson { value, .. } => value.to_string(),
        ToolResultOutput::ExecutionDenied { reason, .. } => reason.clone().unwrap_or_default(),
    };
    Some((part.tool_name.clone(), output))
}

fn render_conversation_markdown(messages: &[Arc<coco_messages::Message>]) -> String {
    let mut out = String::from("# Conversation Export\n");
    for e in conversation_entries(messages) {
        out.push_str(&format!("\n## {}\n\n{}\n", e.label, e.body));
    }
    out
}

fn render_conversation_text(messages: &[Arc<coco_messages::Message>]) -> String {
    let mut out = String::new();
    for e in conversation_entries(messages) {
        out.push_str(&format!("{}:\n{}\n\n", e.label, e.body));
    }
    out
}

fn render_conversation_json(messages: &[Arc<coco_messages::Message>]) -> String {
    let entries: Vec<serde_json::Value> = conversation_entries(messages)
        .into_iter()
        .map(|e| serde_json::json!({ "role": e.label, "text": e.body }))
        .collect();
    // Serializing an array of string-only JSON objects cannot fail; the
    // fallback is unreachable (kept only because `to_string_pretty` is fallible
    // in the type system and clippy forbids unwrap/expect on `Result`).
    serde_json::to_string_pretty(&entries).unwrap_or_else(|_| "[]".to_string())
}

#[cfg(test)]
#[path = "conversation_export.test.rs"]
mod tests;
