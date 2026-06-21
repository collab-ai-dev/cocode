//! `compaction_reminder` generator.
//!
//! Reassures the model that auto-compaction will handle growing context —
//! so it doesn't start rushing or truncating work when the window fills.
//!
//! Gate (all must hold):
//!
//! 1. `config.attachments.compaction_reminder` (settings.json flag).
//! 2. `ctx.is_auto_compact_enabled`.
//! 3. `ctx.context_window >= 1_000_000`.
//! 4. `ctx.used_tokens >= 0.25 * ctx.effective_context_window`.

use async_trait::async_trait;

use crate::error::Result;
use crate::generator::AttachmentGenerator;
use crate::generator::GeneratorContext;
use crate::types::AttachmentType;
use crate::types::SystemReminder;
use coco_config::SystemReminderConfig;

/// Threshold below which the reminder does not fire.
const MIN_CONTEXT_WINDOW: i64 = 1_000_000;

/// Usage ratio at which the reminder begins firing
/// (`usedTokens >= effectiveWindow * 0.25`).
const USAGE_NUMERATOR: i64 = 1;
const USAGE_DENOMINATOR: i64 = 4;

/// Verbatim body from `messages.ts:4139-4147`. The em-dash is intentional.
const BODY: &str = "Auto-compact is enabled. When the context window is nearly full, older messages will be automatically summarized so you can continue working seamlessly. There is no need to stop or rush — you have unlimited context through automatic compaction.";

/// Reassurance reminder for long sessions approaching context capacity.
#[derive(Debug, Default)]
pub struct CompactionReminderGenerator;

#[async_trait]
impl AttachmentGenerator for CompactionReminderGenerator {
    fn name(&self) -> &str {
        "CompactionReminderGenerator"
    }

    fn attachment_type(&self) -> AttachmentType {
        AttachmentType::CompactionReminder
    }

    fn is_enabled(&self, config: &SystemReminderConfig) -> bool {
        config.attachments.compaction_reminder
    }

    async fn generate(&self, ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        if !ctx.is_auto_compact_enabled {
            return Ok(None);
        }
        if ctx.context_window < MIN_CONTEXT_WINDOW {
            return Ok(None);
        }
        // `used >= effective * 0.25` without float math: rearrange to
        // `used * 4 >= effective`. Effective of 0 disables the gate.
        if ctx.effective_context_window <= 0 {
            return Ok(None);
        }
        if ctx.used_tokens * USAGE_DENOMINATOR < ctx.effective_context_window * USAGE_NUMERATOR {
            return Ok(None);
        }
        Ok(Some(SystemReminder::new(
            AttachmentType::CompactionReminder,
            BODY.to_string(),
        )))
    }
}

#[cfg(test)]
#[path = "compaction_reminder.test.rs"]
mod tests;
