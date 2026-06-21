//! `deferred_tools_delta` generator.
//!
//! Announces tool-availability changes since the last emission (via
//! ToolSearch).
//!
//! Gate chain:
//!
//! 1. `ctx.config.attachments.deferred_tools_delta` — default on.
//! 2. `ctx.deferred_tools_delta.is_some()` with non-empty delta —
//!    engine pre-computes by diffing `ctx.tools` against prior
//!    `deferred_tools_delta` attachments in history.
//!
//! Text format: two optional sections joined by `"\n\n"`, each a header
//! line followed by newline-joined entries. The "added" header spells out
//! the ToolSearch round-trip explicitly — schemas are unloaded and a
//! direct call fails — so weaker (non-Anthropic) models don't read the
//! bare name list as "callable now" and invoke a deferred tool blind.

use async_trait::async_trait;

use crate::error::Result;
use crate::generator::AttachmentGenerator;
use crate::generator::DeferredToolsDeltaInfo;
use crate::generator::GeneratorContext;
use crate::types::AttachmentType;
use crate::types::SystemReminder;
use coco_config::SystemReminderConfig;

#[derive(Debug, Default)]
pub struct DeferredToolsDeltaGenerator;

#[async_trait]
impl AttachmentGenerator for DeferredToolsDeltaGenerator {
    fn name(&self) -> &str {
        "DeferredToolsDeltaGenerator"
    }

    fn attachment_type(&self) -> AttachmentType {
        AttachmentType::DeferredToolsDelta
    }

    fn is_enabled(&self, config: &SystemReminderConfig) -> bool {
        config.attachments.deferred_tools_delta
    }

    async fn generate(&self, ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        let Some(info) = ctx.deferred_tools_delta.as_ref() else {
            return Ok(None);
        };
        if info.is_empty() {
            return Ok(None);
        }
        Ok(Some(SystemReminder::new(
            AttachmentType::DeferredToolsDelta,
            render(info),
        )))
    }
}

fn render(info: &DeferredToolsDeltaInfo) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(2);
    if !info.added_lines.is_empty() {
        parts.push(format!(
            "The following deferred tools are now available via ToolSearch. Their schemas are NOT loaded — calling them directly will fail with InputValidationError. Use ToolSearch with query \"select:<name>[,<name>...]\" to load tool schemas before calling them:\n{}",
            info.added_lines.join("\n")
        ));
    }
    if !info.removed_names.is_empty() {
        parts.push(format!(
            "The following deferred tools are no longer available (their MCP server disconnected). Do not search for them \u{2014} ToolSearch will return no match:\n{}",
            info.removed_names.join("\n")
        ));
    }
    parts.join("\n\n")
}

#[cfg(test)]
#[path = "deferred_tools_delta.test.rs"]
mod tests;
