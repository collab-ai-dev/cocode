//! `budget_usd` generator.
//!
//! Main-thread-only per-turn budget report; fires whenever the session
//! has a configured USD cap.
//!
//! Gate chain:
//!
//! 1. `ctx.config.attachments.budget_usd` — default on.
//! 2. `ctx.max_budget_usd.is_some()` — `None` means the user didn't
//!    configure a cap.

use async_trait::async_trait;

use crate::error::Result;
use crate::generator::AttachmentGenerator;
use crate::generator::GeneratorContext;
use crate::types::AttachmentType;
use crate::types::SystemReminder;
use coco_config::SystemReminderConfig;

#[derive(Debug, Default)]
pub struct BudgetUsdGenerator;

#[async_trait]
impl AttachmentGenerator for BudgetUsdGenerator {
    fn name(&self) -> &str {
        "BudgetUsdGenerator"
    }

    fn attachment_type(&self) -> AttachmentType {
        AttachmentType::BudgetUsd
    }

    fn is_enabled(&self, config: &SystemReminderConfig) -> bool {
        config.attachments.budget_usd
    }

    async fn generate(&self, ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        // Surface cumulative session cost on every turn that has spend, even
        // when no USD cap is configured (max_budget_usd = None). The
        // model-visible reminder requires a cap, but ops logs always benefit
        // from the cost signal. Format to 6 decimals so floating-point ulp
        // noise (e.g. `0.058757500000000004`) doesn't leak into the log.
        let used = ctx.total_cost_usd;
        if used > 0.0 {
            tracing::info!(
                target: "coco_system_reminder::cost",
                used_usd = format!("{used:.6}"),
                budget_usd = ?ctx.max_budget_usd.map(|v| format!("{v:.6}")),
                "session cost so far"
            );
        }

        let Some(total) = ctx.max_budget_usd else {
            return Ok(None);
        };
        let remaining = total - used;
        // Format: `USD budget: $${used}/$${total}; $${remaining} remaining`
        // (`$${}` = literal `$` + interpolated number).
        let body = format!("USD budget: ${used}/${total}; ${remaining} remaining");
        Ok(Some(SystemReminder::new(AttachmentType::BudgetUsd, body)))
    }
}

#[cfg(test)]
#[path = "budget_usd.test.rs"]
mod tests;
