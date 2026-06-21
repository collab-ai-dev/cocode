//! `companion_intro` generator.
//!
//! One-shot intro emitted once per session when a companion is configured
//! and hasn't been announced yet.
//!
//! Gate chain:
//!
//! 1. `ctx.config.attachments.companion_intro` — opt-in; external builds
//!    default off.
//! 2. `ctx.companion_name.is_some() && ctx.companion_species.is_some()`
//!    — absence of name or species suppresses the reminder.
//! 3. `!ctx.has_prior_companion_intro` — the engine pre-scans history
//!    for a prior `companion_intro` attachment matching the current name.

use async_trait::async_trait;

use crate::error::Result;
use crate::generator::AttachmentGenerator;
use crate::generator::GeneratorContext;
use crate::types::AttachmentType;
use crate::types::SystemReminder;
use coco_config::SystemReminderConfig;

#[derive(Debug, Default)]
pub struct CompanionIntroGenerator;

#[async_trait]
impl AttachmentGenerator for CompanionIntroGenerator {
    fn name(&self) -> &str {
        "CompanionIntroGenerator"
    }

    fn attachment_type(&self) -> AttachmentType {
        AttachmentType::CompanionIntro
    }

    fn is_enabled(&self, config: &SystemReminderConfig) -> bool {
        config.attachments.companion_intro
    }

    async fn generate(&self, ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        if ctx.has_prior_companion_intro {
            return Ok(None);
        }
        let (Some(name), Some(species)) = (
            ctx.companion_name.as_deref(),
            ctx.companion_species.as_deref(),
        ) else {
            return Ok(None);
        };
        let body = render_companion_intro(name, species);
        Ok(Some(SystemReminder::new(
            AttachmentType::CompanionIntro,
            body,
        )))
    }
}

/// Renders the companion intro body. Any drift would desync model behavior
/// around companion addressing.
fn render_companion_intro(name: &str, species: &str) -> String {
    format!(
        "# Companion\n\nA small {species} named {name} sits beside the user's input box and occasionally comments in a speech bubble. You're not {name} — it's a separate watcher.\n\nWhen the user addresses {name} directly (by name), its bubble will answer. Your job in that moment is to stay out of the way: respond in ONE line or less, or just answer any part of the message meant for you. Don't explain that you're not {name} — they know. Don't narrate what {name} might say — the bubble handles that."
    )
}

#[cfg(test)]
#[path = "companion_intro.test.rs"]
mod tests;
