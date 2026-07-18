//! `mcp_servers_delta` generator.
//!
//! Announces which MCP servers are connected so the model knows what to
//! ToolSearch for when their tool schemas are deferred or reached through the
//! `use_tool` carrier. The engine pre-computes a bounded, sorted
//! snapshot and fires only on a real change. MCP activation/filtering owns
//! complete suppression.

use async_trait::async_trait;
use std::fmt::Write;

use crate::error::Result;
use crate::generator::AttachmentGenerator;
use crate::generator::GeneratorContext;
use crate::generator::McpServersDeltaInfo;
use crate::types::AttachmentType;
use crate::types::SystemReminder;
use coco_config::SystemReminderConfig;

const MAX_REMINDER_BYTES: usize = 4 * 1024;

#[derive(Debug, Default)]
pub struct McpServersDeltaGenerator;

#[async_trait]
impl AttachmentGenerator for McpServersDeltaGenerator {
    fn name(&self) -> &str {
        "McpServersDeltaGenerator"
    }

    fn attachment_type(&self) -> AttachmentType {
        AttachmentType::McpServersDelta
    }

    fn is_enabled(&self, config: &SystemReminderConfig) -> bool {
        config.attachments.mcp_servers_delta
    }

    async fn generate(&self, ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        let Some(info) = ctx.mcp_servers_delta.as_ref() else {
            return Ok(None);
        };
        if info.is_empty() {
            return Ok(None);
        }
        Ok(Some(SystemReminder::new(
            AttachmentType::McpServersDelta,
            render(info),
        )))
    }
}

fn render(info: &McpServersDeltaInfo) -> String {
    let mut out = String::new();
    if !info.servers.is_empty() {
        out.push_str(
            "The following MCP servers have tools discoverable through ToolSearch \
             (query the server name or `mcp__<server>`):",
        );
        for server in &info.servers {
            let _ = write!(
                out,
                "\n- {} ({} tools)",
                escape_untrusted_text(&server.name),
                server.tool_count
            );
            if let Some(desc) = &server.description {
                let _ = write!(out, ": {}", escape_untrusted_text(desc));
            }
        }
        if info.omitted > 0 {
            let _ = write!(out, "\n(+{} more not shown)", info.omitted);
        }
    }
    if !info.removed_names.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str("These MCP servers are no longer discoverable:");
        for name in &info.removed_names {
            let _ = write!(out, "\n- {}", escape_untrusted_text(name));
        }
    }
    truncate_to_bytes(out, MAX_REMINDER_BYTES)
}

fn escape_untrusted_text(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn truncate_to_bytes(mut value: String, max_bytes: usize) -> String {
    if value.len() > max_bytes {
        let boundary = value.floor_char_boundary(max_bytes);
        value.truncate(boundary);
    }
    value
}

#[cfg(test)]
#[path = "mcp_servers_delta.test.rs"]
mod tests;
