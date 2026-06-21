//! `prependUserContext` generator (per-turn baseline user context).
//!
//! Injects a single `isMeta` `<system-reminder>` user message at the head
//! of every API request. Its body wraps `{ currentDate }` as a `# key\nvalue`
//! block. `claudeMd` (CLAUDE.md discovery) is injected through the static
//! system prompt (`app/query::build_prompt`) instead.
//!
//! Unlike [`DateChangeGenerator`](super::DateChangeGenerator) (a one-shot
//! notice when the local date rolls over mid-session), this fires every turn
//! so the date is always present. The engine supplies `ctx.current_date` each
//! turn; `None` (the unit-test default) suppresses it.

use async_trait::async_trait;

use crate::error::Result;
use crate::generator::AttachmentGenerator;
use crate::generator::GeneratorContext;
use crate::types::AttachmentType;
use crate::types::SystemReminder;
use coco_config::SystemReminderConfig;

/// Emits the per-turn `currentDate` user-context reminder.
#[derive(Debug, Default)]
pub struct UserContextGenerator;

#[async_trait]
impl AttachmentGenerator for UserContextGenerator {
    fn name(&self) -> &str {
        "UserContextGenerator"
    }

    fn attachment_type(&self) -> AttachmentType {
        AttachmentType::UserContext
    }

    fn is_enabled(&self, config: &SystemReminderConfig) -> bool {
        config.attachments.user_context
    }

    async fn generate(&self, ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        // `prependUserContext` body, minus the outer `<system-reminder>`
        // tags which the injection pipeline re-applies via `wrap_with_tag`.
        // TS sends ONE user-context message carrying one-or-more
        // `# key\nvalue` blocks; coco threads `currentDate` (always, off the
        // engine clock) and — in coordinator mode — `workerToolsContext`
        // (worker tool pool + connected MCP servers, so the leader knows
        // what its spawned workers can do). `claudeMd` lives in the system
        // prompt instead.
        let date = ctx.current_date.as_deref().filter(|d| !d.is_empty());
        let worker = ctx
            .coordinator_worker_context
            .as_deref()
            .filter(|w| !w.is_empty());
        if date.is_none() && worker.is_none() {
            return Ok(None);
        }

        let mut blocks: Vec<String> = Vec::new();
        if let Some(date) = date {
            blocks.push(format!("# currentDate\nToday's date is {date}."));
        }
        if let Some(worker) = worker {
            blocks.push(format!("# workerToolsContext\n{worker}"));
        }
        // The six-space indent before IMPORTANT is a template-literal
        // artifact preserved for model compatibility.
        let content = format!(
            "As you answer the user's questions, you can use the following context:\n\
             {}\n\
             \n      \
             IMPORTANT: this context may or may not be relevant to your tasks. \
             You should not respond to this context unless it is highly relevant to your task.",
            blocks.join("\n"),
        );
        Ok(Some(SystemReminder::new(
            AttachmentType::UserContext,
            content,
        )))
    }
}

#[cfg(test)]
#[path = "user_context.test.rs"]
mod tests;
