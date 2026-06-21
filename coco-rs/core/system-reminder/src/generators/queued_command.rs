//! `queued_command` generator.
//!
//! Replays queued items the model didn't see live, wrapping each in the
//! origin-specific framing from `wrapCommandText`. Each queued item
//! becomes its own `<system-reminder>` block.
//!
//! Earlier this generator filtered out everything without an
//! `origin_system: true` flag — but since no production producer ever
//! set that flag, it emitted nothing. The typed [`crate::QueueOrigin`]
//! enum + per-origin framing now handles all origins end-to-end.

use async_trait::async_trait;

use crate::error::Result;
use crate::generator::AttachmentGenerator;
use crate::generator::GeneratorContext;
use crate::queue_origin::wrap_command_text;
use crate::types::AttachmentType;
use crate::types::ReminderMessage;
use crate::types::SystemReminder;
use coco_config::SystemReminderConfig;

#[derive(Debug, Default)]
pub struct QueuedCommandGenerator;

#[async_trait]
impl AttachmentGenerator for QueuedCommandGenerator {
    fn name(&self) -> &str {
        "QueuedCommandGenerator"
    }

    fn attachment_type(&self) -> AttachmentType {
        AttachmentType::QueuedCommand
    }

    fn is_enabled(&self, config: &SystemReminderConfig) -> bool {
        config.attachments.queued_command
    }

    async fn generate(&self, ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        let messages: Vec<ReminderMessage> = ctx
            .queued_commands
            .iter()
            .filter(|q| !q.content.is_empty() || !q.images.is_empty())
            .map(|q| {
                let text = wrap_command_text(&q.content, q.origin.as_ref());
                if q.images.is_empty() {
                    ReminderMessage::user_text(text)
                } else {
                    // Text first, then images.
                    let images = q
                        .images
                        .iter()
                        .map(|img| (img.media_type.clone(), img.data_base64.clone()))
                        .collect();
                    ReminderMessage::user_text_with_images(text, images)
                }
            })
            .collect();
        if messages.is_empty() {
            return Ok(None);
        }
        Ok(Some(SystemReminder::messages(
            AttachmentType::QueuedCommand,
            messages,
        )))
    }
}

#[cfg(test)]
#[path = "queued_command.test.rs"]
mod tests;
