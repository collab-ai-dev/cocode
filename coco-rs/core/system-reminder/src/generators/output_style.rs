//! `output_style` generator.
//!
//! Injects a reminder that the active output style's guidelines should
//! be followed this turn.

use async_trait::async_trait;

use crate::error::Result;
use crate::generator::AttachmentGenerator;
use crate::generator::GeneratorContext;
use crate::types::AttachmentType;
use crate::types::SystemReminder;
use coco_config::SystemReminderConfig;

#[derive(Debug, Default)]
pub struct OutputStyleGenerator;

#[async_trait]
impl AttachmentGenerator for OutputStyleGenerator {
    fn name(&self) -> &str {
        "OutputStyleGenerator"
    }

    fn attachment_type(&self) -> AttachmentType {
        AttachmentType::OutputStyle
    }

    fn is_enabled(&self, config: &SystemReminderConfig) -> bool {
        config.attachments.output_style
    }

    async fn generate(&self, ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        let Some(style) = ctx.output_style.as_ref() else {
            return Ok(None);
        };
        if style.name.is_empty() {
            return Ok(None);
        }
        let body = format!(
            "{} output style is active. Remember to follow the specific guidelines for this style.",
            style.name
        );
        Ok(Some(SystemReminder::new(AttachmentType::OutputStyle, body)))
    }
}

#[cfg(test)]
#[path = "output_style.test.rs"]
mod tests;
