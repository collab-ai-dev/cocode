//! `tool_search_usage_reminder` generator.
//!
//! Standing nudge — distinct from the one-shot `deferred_tools_delta`
//! change-announcer. Whenever deferred tools remain undiscovered, remind the
//! model their schemas can be loaded via ToolSearch before it concludes a
//! capability is missing or builds a workaround. Highest-leverage for
//! non-Anthropic providers where ToolSearch is *promoted* rather than native,
//! so weaker models otherwise treat a deferred tool as absent.
//!
//! Gate: `ctx.deferred_tools` non-empty. That set is exactly the
//! still-undiscovered names, and a non-empty set implies ToolSearch is active
//! (you can't have deferred tools without it), so no extra tool-presence check
//! is needed. Config-gated via `attachments.tool_search_usage_reminder`.

use async_trait::async_trait;

use crate::error::Result;
use crate::generator::AttachmentGenerator;
use crate::generator::GeneratorContext;
use crate::types::AttachmentType;
use crate::types::SystemReminder;
use coco_config::SystemReminderConfig;
use coco_types::ToolName;

/// Max undiscovered tool names listed inline; the rest collapse to "(+N more)"
/// so the reminder stays bounded when many MCP tools are deferred.
const MAX_LISTED: usize = 10;

#[derive(Debug, Default)]
pub struct ToolSearchUsageReminderGenerator;

#[async_trait]
impl AttachmentGenerator for ToolSearchUsageReminderGenerator {
    fn name(&self) -> &str {
        "ToolSearchUsageReminderGenerator"
    }

    fn attachment_type(&self) -> AttachmentType {
        AttachmentType::ToolSearchUsageReminder
    }

    fn is_enabled(&self, config: &SystemReminderConfig) -> bool {
        config.attachments.tool_search_usage_reminder
    }

    async fn generate(&self, ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        if ctx.deferred_tools.is_empty() {
            return Ok(None);
        }
        Ok(Some(SystemReminder::new(
            AttachmentType::ToolSearchUsageReminder,
            render(&ctx.deferred_tools),
        )))
    }
}

fn render(deferred: &[String]) -> String {
    let listed: Vec<&str> = deferred
        .iter()
        .take(MAX_LISTED)
        .map(String::as_str)
        .collect();
    let overflow = deferred.len().saturating_sub(listed.len());
    let names = if overflow > 0 {
        format!("{} (+{overflow} more)", listed.join(", "))
    } else {
        listed.join(", ")
    };
    format!(
        "Some available tools' schemas are not loaded in this conversation yet: {names}. \
         Before concluding a capability is missing or building a workaround, use {tool_search} \
         to find and load relevant tools \u{2014} keywords to search, or query \
         \"select:<name>[,<name>...]\" for specific tools. Calling a tool before its schema is \
         loaded will fail. This is just a gentle reminder - ignore if not applicable to the \
         current work.",
        tool_search = ToolName::ToolSearch.as_str(),
    )
}

#[cfg(test)]
#[path = "tool_search_usage_reminder.test.rs"]
mod tests;
