//! `token_usage` generator.
//!
//! Main-thread-only per-turn usage report.
//!
//! Gate chain:
//!
//! 1. `ctx.config.attachments.token_usage` — opt-in (default off).
//! 2. `ctx.effective_context_window > 0` — need a real window to
//!    compute remaining; zero means the engine hasn't populated the
//!    field yet (safe to skip).
//!
//! Content format: `Context window usage: N / M tokens`.

use async_trait::async_trait;

use crate::error::Result;
use crate::generator::AttachmentGenerator;
use crate::generator::GeneratorContext;
use crate::types::AttachmentType;
use crate::types::SystemReminder;
use coco_config::SystemReminderConfig;

#[derive(Debug, Default)]
pub struct TokenUsageGenerator;

#[async_trait]
impl AttachmentGenerator for TokenUsageGenerator {
    fn name(&self) -> &str {
        "TokenUsageGenerator"
    }

    fn attachment_type(&self) -> AttachmentType {
        AttachmentType::TokenUsage
    }

    fn is_enabled(&self, config: &SystemReminderConfig) -> bool {
        config.attachments.token_usage
    }

    async fn generate(&self, ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        // A zero window means the engine hasn't filled the field —
        // skip silently rather than emit a nonsense "used/0".
        let total = ctx.effective_context_window;
        if total <= 0 {
            return Ok(None);
        }
        let used = ctx.used_tokens.max(0);
        let remaining = (total - used).max(0);
        let body = format!("Token usage: {used}/{total}; {remaining} remaining");
        Ok(Some(SystemReminder::new(AttachmentType::TokenUsage, body)))
    }
}

#[cfg(test)]
#[path = "token_usage.test.rs"]
mod tests;
