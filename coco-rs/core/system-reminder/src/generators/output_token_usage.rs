//! `output_token_usage` generator.
//!
//! Main-thread-only per-turn output-token report with optional turn budget.
//!
//! Gate chain:
//!
//! 1. `ctx.config.attachments.output_token_usage` — opt-in (default off).
//! 2. `ctx.output_token_budget` is `Some(n)` with `n > 0`. `None` or
//!    non-positive suppresses emission.

use async_trait::async_trait;

use crate::error::Result;
use crate::generator::AttachmentGenerator;
use crate::generator::GeneratorContext;
use crate::types::AttachmentType;
use crate::types::SystemReminder;
use coco_config::SystemReminderConfig;

#[derive(Debug, Default)]
pub struct OutputTokenUsageGenerator;

#[async_trait]
impl AttachmentGenerator for OutputTokenUsageGenerator {
    fn name(&self) -> &str {
        "OutputTokenUsageGenerator"
    }

    fn attachment_type(&self) -> AttachmentType {
        AttachmentType::OutputTokenUsage
    }

    fn is_enabled(&self, config: &SystemReminderConfig) -> bool {
        config.attachments.output_token_usage
    }

    async fn generate(&self, ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        let Some(budget) = ctx.output_token_budget else {
            return Ok(None);
        };
        if budget <= 0 {
            return Ok(None);
        }
        let turn = format_number(ctx.output_tokens_turn);
        let budget_str = format_number(budget);
        let session = format_number(ctx.output_tokens_session);
        // Format: `Output tokens — turn: ${turn}/${budget} · session: ${session}`.
        // Em-dash and middle-dot are kept verbatim for model compatibility.
        let body = format!(
            "Output tokens \u{2014} turn: {turn} / {budget_str} \u{00b7} session: {session}"
        );
        Ok(Some(SystemReminder::new(
            AttachmentType::OutputTokenUsage,
            body,
        )))
    }
}

/// Format `n` with comma-separated thousands (e.g. 1234567 → "1,234,567").
fn format_number(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if n < 0 {
        out.push('-');
    }
    let len = digits.len();
    let first_group = match len % 3 {
        0 => 3,
        r => r,
    };
    for (idx, ch) in digits.chars().enumerate() {
        if idx == first_group || (idx > first_group && (idx - first_group) % 3 == 0) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
#[path = "output_token_usage.test.rs"]
mod tests;
