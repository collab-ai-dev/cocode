//! `goal_context` generator — the mandatory per-turn goal reminder (design §5.5).
//!
//! Fires on every goal-owned turn, re-injecting the re-materialized objective,
//! budget, progress, and `report_goal_turn` protocol so compaction cannot erase
//! the goal. The body is pre-rendered by the session runtime (which owns the
//! `GoalContextMaterializer`) and threaded onto [`GeneratorContext::goal_context`];
//! this generator only emits it. It bypasses optional reminder toggles because
//! it is part of the goal execution contract.

use async_trait::async_trait;

use crate::error::Result;
use crate::generator::AttachmentGenerator;
use crate::generator::GeneratorContext;
use crate::types::AttachmentType;
use crate::types::SystemReminder;
use coco_config::SystemReminderConfig;

/// Emits the per-turn `goal_context` reminder when a goal turn is running.
#[derive(Debug, Default)]
pub struct GoalContextGenerator;

#[async_trait]
impl AttachmentGenerator for GoalContextGenerator {
    fn name(&self) -> &str {
        "GoalContextGenerator"
    }

    fn attachment_type(&self) -> AttachmentType {
        AttachmentType::GoalContext
    }

    /// Part of the goal execution contract — always enabled (design §5.5).
    fn is_enabled(&self, _config: &SystemReminderConfig) -> bool {
        true
    }

    async fn generate(&self, ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        let Some(body) = ctx.goal_context.as_deref().filter(|body| !body.is_empty()) else {
            return Ok(None);
        };
        Ok(Some(SystemReminder::new(
            AttachmentType::GoalContext,
            body.to_string(),
        )))
    }
}

#[cfg(test)]
#[path = "goal_context.test.rs"]
mod tests;
