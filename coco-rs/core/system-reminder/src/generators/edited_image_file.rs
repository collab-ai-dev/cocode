//! Silent `edited_image_file` generator.
//!
//! Emits a marker listing image files whose mtime changed since the last
//! observation. Image diffs can't be surfaced textually, so the model gets
//! zero API tokens. UI surfaces the change via
//! [`ReminderMetadata::EditedImageFile`].
//!
//! Sibling to `AlreadyReadFileGenerator` — both live in this crate because
//! file-change tracking is reminder-adjacent (fed by `core/context`'s
//! read-file state), not a separate subsystem.
//!
//! Gate chain:
//!
//! 1. `config.attachments.edited_image_file` — default on.
//! 2. `ctx.edited_image_file_paths` non-empty.

use async_trait::async_trait;

use crate::error::Result;
use crate::generator::AttachmentGenerator;
use crate::generator::GeneratorContext;
use crate::types::AttachmentType;
use crate::types::EditedImageFileMeta;
use crate::types::ReminderMetadata;
use crate::types::SystemReminder;
use coco_config::SystemReminderConfig;

#[derive(Debug, Default)]
pub struct EditedImageFileGenerator;

#[async_trait]
impl AttachmentGenerator for EditedImageFileGenerator {
    fn name(&self) -> &str {
        "EditedImageFileGenerator"
    }

    fn attachment_type(&self) -> AttachmentType {
        AttachmentType::EditedImageFile
    }

    fn is_enabled(&self, config: &SystemReminderConfig) -> bool {
        config.attachments.edited_image_file
    }

    async fn generate(&self, ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        if ctx.edited_image_file_paths.is_empty() {
            return Ok(None);
        }
        let meta = EditedImageFileMeta {
            paths: ctx.edited_image_file_paths.clone(),
        };
        Ok(Some(SystemReminder::silent_attachment(
            AttachmentType::EditedImageFile,
            ReminderMetadata::EditedImageFile(meta),
        )))
    }
}

#[cfg(test)]
#[path = "edited_image_file.test.rs"]
mod tests;
