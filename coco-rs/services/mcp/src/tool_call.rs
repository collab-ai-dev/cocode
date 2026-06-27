//! MCP tool call handling.

use coco_types::MCP_TOOL_PREFIX;
use coco_types::MCP_TOOL_SEPARATOR;
use serde::Deserialize;
use serde::Serialize;

/// Maximum tool description length.
const MAX_DESCRIPTION_LENGTH: usize = 2048;

/// Result of an MCP tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolCallResult {
    pub tool_name: String,
    pub server_name: String,
    pub content: Vec<McpToolContent>,
    pub is_error: bool,
    pub duration_ms: i64,
}

/// Content block in an MCP tool result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpToolContent {
    Text { text: String },
    Image { data: String, mime_type: String },
    Resource { uri: String, text: Option<String> },
}

/// Truncate `s` to at most `max_len` BYTES, snapped down to the nearest
/// char boundary. #154: `&s[..max_len]` panics when `max_len` lands
/// inside a multibyte UTF-8 sequence; this never does.
fn truncate_at_char_boundary(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        return s;
    }
    let mut end = max_len;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Truncate a tool description (or server instructions) to the maximum length.
/// Uses a Unicode horizontal ellipsis (U+2026), not three ASCII dots.
pub fn truncate_description(description: &str) -> String {
    if description.len() <= MAX_DESCRIPTION_LENGTH {
        description.to_string()
    } else {
        format!(
            "{}… [truncated]",
            truncate_at_char_boundary(description, MAX_DESCRIPTION_LENGTH)
        )
    }
}

/// Format an MCP tool call error for the model.
pub fn format_mcp_error(server: &str, tool: &str, error: &str) -> String {
    format!("MCP tool call failed (server={server}, tool={tool}): {error}")
}

/// Parse MCP tool name into (server_name, tool_name).
///
/// Format: "mcp__server__tool"
pub fn parse_mcp_tool_name(full_name: &str) -> Option<(&str, &str)> {
    let stripped = full_name.strip_prefix(MCP_TOOL_PREFIX)?;
    let parts: Vec<&str> = stripped.splitn(2, MCP_TOOL_SEPARATOR).collect();
    if parts.len() == 2 {
        Some((parts[0], parts[1]))
    } else {
        None
    }
}

/// Build the full MCP tool name from server and tool names.
///
/// Delegates to `naming::mcp_tool_id` which normalizes names for wire format.
pub fn build_mcp_tool_name(server: &str, tool: &str) -> String {
    crate::naming::mcp_tool_id(server, tool)
}

#[cfg(test)]
#[path = "tool_call.test.rs"]
mod tests;
