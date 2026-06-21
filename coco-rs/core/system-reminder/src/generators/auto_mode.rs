//! Auto-mode exit reminder generator.
//!
//! One-shot exit banner. Steady-state auto-mode Full/Sparse cadence is
//! handled by `auto_mode_enter.rs`. Text is rendered by
//! `coco_context::render_auto_mode_exit_reminder`.

use async_trait::async_trait;

use crate::error::Result;
use crate::generator::AttachmentGenerator;
use crate::generator::GeneratorContext;
use crate::types::AttachmentType;
use crate::types::SystemReminder;
use coco_config::SystemReminderConfig;

/// One-shot `## Exited Auto Mode` banner.
///
/// Fires when [`GeneratorContext::needs_auto_mode_exit_attachment`] is set.
/// The engine sets the flag on any Auto→non-Auto transition and clears it
/// after the orchestrator consumes the reminder.
#[derive(Debug, Default)]
pub struct AutoModeExitGenerator;

#[async_trait]
impl AttachmentGenerator for AutoModeExitGenerator {
    fn name(&self) -> &str {
        "AutoModeExitGenerator"
    }

    fn attachment_type(&self) -> AttachmentType {
        AttachmentType::AutoModeExit
    }

    fn is_enabled(&self, config: &SystemReminderConfig) -> bool {
        config.attachments.auto_mode_exit
    }

    async fn generate(&self, ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        if !ctx.needs_auto_mode_exit_attachment {
            return Ok(None);
        }
        // Suppress when still in auto mode — the flag is stale. The engine
        // mirrors this app-state cleanup after generation.
        if ctx.is_auto_mode {
            return Ok(None);
        }
        let text = coco_context::render_auto_mode_exit_reminder();
        Ok(Some(SystemReminder::new(
            AttachmentType::AutoModeExit,
            text,
        )))
    }
}

#[cfg(test)]
#[path = "auto_mode.test.rs"]
mod tests;
