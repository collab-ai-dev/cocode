//! Legacy `verify_plan_reminder` generator.
//!
//! Deprecated compatibility path. It fires every 10 turns after
//! `ExitPlanMode` only when the legacy TS-shaped
//! `pending_plan_verification` exists and is neither started nor completed,
//! nudging the agent to call the explicitly registered
//! `VerifyPlanExecution` tool.
//!
//! **Tier**: [`ReminderTier::MainAgentOnly`] — sub-agents don't own the
//! plan; the reminder would be wasted tokens.
//!
//! Gate chain (all must pass):
//!
//! 1. Config flag enabled (`config.attachments.verify_plan_reminder`).
//! 2. Main-agent only (enforced by tier filter in the orchestrator).
//! 3. `ctx.has_pending_plan_verification` (derived from
//!    `ToolAppState::pending_plan_verification`; this is only set when
//!    legacy `settings.plan_mode.verify_execution` is explicitly enabled).
//! 4. `VerifyPlanExecution` is explicitly registered and present in the
//!    model-visible tool list.
//! 5. `ctx.turns_since_plan_exit > 0` and divisible by 10 — skips turn 0
//!    and fires at 10, 20, …

use async_trait::async_trait;

use crate::error::Result;
use crate::generator::AttachmentGenerator;
use crate::generator::GeneratorContext;
use crate::types::AttachmentType;
use crate::types::SystemReminder;
use coco_config::SystemReminderConfig;
use coco_types::ToolName;

const TURNS_BETWEEN_REMINDERS: i32 = 10;

/// Verbatim body from `messages.ts:4247`.
fn body() -> String {
    format!(
        "You have completed implementing the plan. Please call the \"{}\" tool directly (NOT the Agent tool or an agent) to verify that all plan items were completed correctly.",
        ToolName::VerifyPlanExecution.as_str()
    )
}

/// Nudges the main agent to call `VerifyPlanExecution` after plan exit.
#[derive(Debug, Default)]
pub struct VerifyPlanReminderGenerator;

#[async_trait]
impl AttachmentGenerator for VerifyPlanReminderGenerator {
    fn name(&self) -> &str {
        "VerifyPlanReminderGenerator"
    }

    fn attachment_type(&self) -> AttachmentType {
        AttachmentType::VerifyPlanReminder
    }

    fn is_enabled(&self, config: &SystemReminderConfig) -> bool {
        config.attachments.verify_plan_reminder
    }

    async fn generate(&self, ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        if !ctx.has_pending_plan_verification {
            return Ok(None);
        }
        if !ctx
            .tools
            .iter()
            .any(|name| name == ToolName::VerifyPlanExecution.as_str())
        {
            return Ok(None);
        }
        let n = ctx.turns_since_plan_exit;
        if n <= 0 || n % TURNS_BETWEEN_REMINDERS != 0 {
            return Ok(None);
        }
        Ok(Some(SystemReminder::new(
            AttachmentType::VerifyPlanReminder,
            body(),
        )))
    }
}

#[cfg(test)]
#[path = "verify_plan.test.rs"]
mod tests;
