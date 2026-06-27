//! `auto_mode` steady-state generator.
//!
//! Full/Sparse cadence symmetric to [`PlanModeEnterGenerator`] but gated on
//! `ctx.is_auto_mode` (engine flag covering both `mode == 'auto'` and
//! `mode == 'plan'` with the auto-mode classifier active).
//!
//! Auto-mode exit is a separate one-shot generator
//! ([`super::AutoModeExitGenerator`]); this file handles entry + steady-state
//! only.

use async_trait::async_trait;

use crate::error::Result;
use crate::generator::AttachmentGenerator;
use crate::generator::GeneratorContext;
use crate::types::AttachmentType;
use crate::types::SystemReminder;
use coco_config::SystemReminderConfig;

/// Human turns between successive steady-state auto-mode reminders.
const AUTO_MODE_TURNS_BETWEEN: i32 = 5;

/// Full reminder on every Nth attachment since the last exit (1st, 6th, 11th…).
const FULL_REMINDER_EVERY_N: i32 = 5;

/// Full auto-mode instructions.
const AUTO_MODE_FULL: &str = "## Auto Mode Active

Auto mode is active. The user chose continuous, autonomous execution. You should:

1. **Execute immediately** — Start implementing right away. Make reasonable assumptions and proceed on low-risk work.
2. **Minimize interruptions** — Prefer making reasonable assumptions over asking questions for routine decisions.
3. **Prefer action over planning** — Do not enter plan mode unless the user explicitly asks. When in doubt, start coding.
4. **Expect course corrections** — The user may provide suggestions or course corrections at any point; treat those as normal input.
5. **Do not take overly destructive actions** — Auto mode is not a license to destroy. Anything that deletes data or modifies shared or production systems still needs explicit user confirmation. If you reach such a decision point, ask and wait, or course correct to a safer method instead.
6. **Avoid data exfiltration** — Post even routine messages to chat platforms or work tickets only if the user has directed you to. You must not share secrets (e.g. credentials, internal documentation) unless the user has explicitly authorized both that specific secret and its destination.";

/// One-line sparse reminder.
const AUTO_MODE_SPARSE: &str = "Auto mode still active (see full instructions earlier in conversation). Execute autonomously, minimize interruptions, prefer action over planning.";

/// Steady-state "auto mode still active" reminder.
///
/// Cadence is derived from history (no in-memory throttle), symmetric to
/// [`super::PlanModeEnterGenerator`]: always emit on the first auto-mode turn,
/// otherwise one emission every [`AUTO_MODE_TURNS_BETWEEN`] human turns, Full
/// on every [`FULL_REMINDER_EVERY_N`]th attachment since the last exit.
#[derive(Debug, Default)]
pub struct AutoModeEnterGenerator;

#[async_trait]
impl AttachmentGenerator for AutoModeEnterGenerator {
    fn name(&self) -> &str {
        "AutoModeEnterGenerator"
    }

    fn attachment_type(&self) -> AttachmentType {
        AttachmentType::AutoMode
    }

    fn is_enabled(&self, config: &SystemReminderConfig) -> bool {
        config.attachments.auto_mode
    }

    async fn generate(&self, ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        if !ctx.is_auto_mode {
            return Ok(None);
        }

        // Throttle to one emission per AUTO_MODE_TURNS_BETWEEN human turns,
        // always emitting the first time (no prior attachment → `None`).
        if let Some(n) = ctx.auto_mode_turns_since_attachment
            && n < AUTO_MODE_TURNS_BETWEEN
        {
            return Ok(None);
        }

        let attachment_index = ctx.auto_mode_attachments_since_exit + 1;
        let content = if attachment_index % FULL_REMINDER_EVERY_N == 1 {
            AUTO_MODE_FULL
        } else {
            AUTO_MODE_SPARSE
        };
        Ok(Some(SystemReminder::new(
            AttachmentType::AutoMode,
            content.to_string(),
        )))
    }
}

#[cfg(test)]
#[path = "auto_mode_enter.test.rs"]
mod tests;
