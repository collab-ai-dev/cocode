//! `mcp_instructions_delta` generator.
//!
//! Fires when MCP server instructions are added or servers disconnect.
//!
//! Gate chain:
//!
//! 1. `ctx.config.attachments.mcp_instructions_delta` — default on.
//! 2. `ctx.mcp_instructions_delta.is_some()` with non-empty delta —
//!    engine pre-computes by diffing current MCP server instructions
//!    against prior announcements in history.
//!
//! Output: two optional sections
//! joined by `"\n\n"`.

use async_trait::async_trait;

use crate::error::Result;
use crate::generator::AttachmentGenerator;
use crate::generator::GeneratorContext;
use crate::generator::McpInstructionsDeltaInfo;
use crate::types::AttachmentType;
use crate::types::SystemReminder;
use coco_config::SystemReminderConfig;

#[derive(Debug, Default)]
pub struct McpInstructionsDeltaGenerator;

#[async_trait]
impl AttachmentGenerator for McpInstructionsDeltaGenerator {
    fn name(&self) -> &str {
        "McpInstructionsDeltaGenerator"
    }

    fn attachment_type(&self) -> AttachmentType {
        AttachmentType::McpInstructionsDelta
    }

    fn is_enabled(&self, config: &SystemReminderConfig) -> bool {
        config.attachments.mcp_instructions_delta
    }

    async fn generate(&self, ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        let Some(info) = ctx.mcp_instructions_delta.as_ref() else {
            return Ok(None);
        };
        if info.is_empty() {
            return Ok(None);
        }
        Ok(Some(SystemReminder::new(
            AttachmentType::McpInstructionsDelta,
            render(info),
        )))
    }
}

fn render(info: &McpInstructionsDeltaInfo) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(2);
    if !info.added_blocks.is_empty() {
        parts.push(format!(
            "# MCP Server Instructions\n\nThe following MCP servers have provided instructions for how to use their tools and resources:\n\n{}",
            info.added_blocks.join("\n\n")
        ));
    }
    if !info.removed_names.is_empty() {
        parts.push(format!(
            "The following MCP servers have disconnected. Their instructions above no longer apply:\n{}",
            info.removed_names.join("\n")
        ));
    }
    parts.join("\n\n")
}

#[cfg(test)]
#[path = "mcp_instructions_delta.test.rs"]
mod tests;
