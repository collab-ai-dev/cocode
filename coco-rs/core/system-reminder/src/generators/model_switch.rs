//! `model_switch` generator.
//!
//! Tells the model that the session's model changed under it.
//!
//! This is not cosmetic. The static system prompt is assembled once, at engine
//! construction, and its `<env>` block renders `You are powered by the model
//! <id>.` plus that model's knowledge cutoff. `/model` mid-session swaps only
//! `engine_config.model_id` — nothing rebuilds the prompt. Without this
//! reminder the conversation keeps asserting the *previous* model's identity
//! for the rest of the session, which is worse than saying nothing.
//!
//! Gate chain:
//!
//! 1. `ctx.config.attachments.model_switch` — default on.
//! 2. `ctx.model_switch.is_some()` — the engine diffs the current model id
//!    against `WorldStateSnapshot.model`, the persisted record of what this
//!    scope was last told. A first-ever turn has no previous model and emits
//!    nothing: the prompt is correct at that point, and announcing a switch
//!    that never happened would be its own falsehood.

use async_trait::async_trait;

use crate::error::Result;
use crate::generator::AttachmentGenerator;
use crate::generator::GeneratorContext;
use crate::generator::ModelSwitchInfo;
use crate::types::AMBIENT_CONTEXT_TRAILER;
use crate::types::AttachmentType;
use crate::types::SystemReminder;
use coco_config::SystemReminderConfig;

#[derive(Debug, Default)]
pub struct ModelSwitchGenerator;

#[async_trait]
impl AttachmentGenerator for ModelSwitchGenerator {
    fn name(&self) -> &str {
        "ModelSwitchGenerator"
    }

    fn attachment_type(&self) -> AttachmentType {
        AttachmentType::ModelSwitch
    }

    fn is_enabled(&self, config: &SystemReminderConfig) -> bool {
        config.attachments.model_switch
    }

    async fn generate(&self, ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        let Some(info) = ctx.model_switch.as_ref() else {
            return Ok(None);
        };
        Ok(Some(SystemReminder::new(
            AttachmentType::ModelSwitch,
            render(info),
        )))
    }
}

fn render(info: &ModelSwitchInfo) -> String {
    format!(
        "You are now running as model `{}` (previously `{}`). Any earlier \
         statement in this conversation about which model you are — including \
         the model line and knowledge cutoff in your system prompt — described \
         the previous model and no longer applies.\n\n{AMBIENT_CONTEXT_TRAILER}",
        info.current, info.previous
    )
}

#[cfg(test)]
#[path = "model_switch.test.rs"]
mod tests;
