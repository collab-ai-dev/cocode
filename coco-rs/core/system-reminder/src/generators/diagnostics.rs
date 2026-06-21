//! `diagnostics` generator.
//!
//! Wraps diagnostic-summary text inside `<new-diagnostics>…</new-diagnostics>`
//! before the outer `<system-reminder>`. Engine pre-formats each file's block
//! through its LSP/IDE adapter and populates `ctx.diagnostics`.

use async_trait::async_trait;

use crate::error::Result;
use crate::generator::AttachmentGenerator;
use crate::generator::GeneratorContext;
use crate::types::AttachmentType;
use crate::types::SystemReminder;
use coco_config::SystemReminderConfig;

#[derive(Debug, Default)]
pub struct DiagnosticsGenerator;

#[async_trait]
impl AttachmentGenerator for DiagnosticsGenerator {
    fn name(&self) -> &str {
        "DiagnosticsGenerator"
    }

    fn attachment_type(&self) -> AttachmentType {
        AttachmentType::Diagnostics
    }

    fn is_enabled(&self, config: &SystemReminderConfig) -> bool {
        config.attachments.diagnostics
    }

    async fn generate(&self, ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        // Diagnostics are only useful when the agent has the Bash tool to
        // act on them. Read-only subagents (no Bash) get un-actionable
        // reminders, so suppress.
        if !ctx
            .tools
            .iter()
            .any(|t| t == coco_types::ToolName::Bash.as_str())
        {
            return Ok(None);
        }
        if ctx.diagnostics.is_empty() {
            return Ok(None);
        }
        let summary = ctx
            .diagnostics
            .iter()
            .map(|d| d.formatted.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let body = format!(
            "<new-diagnostics>The following new diagnostic issues were detected:\n\n{summary}</new-diagnostics>"
        );
        Ok(Some(SystemReminder::new(AttachmentType::Diagnostics, body)))
    }
}

#[cfg(test)]
#[path = "diagnostics.test.rs"]
mod tests;
