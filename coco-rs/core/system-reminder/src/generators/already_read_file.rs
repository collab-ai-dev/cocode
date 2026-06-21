//! Silent `already_read_file` generator.
//!
//! Emits a session-dedup marker listing paths the @-mention / memory
//! pipelines have decided not to re-inject. Produces zero API tokens; the
//! payload survives on [`ReminderMetadata::AlreadyReadFile`] so UI /
//! transcript layers can surface "already in context" hints.
//!
//! cocode-rs reference: `core/system-reminder/src/generators/` (the
//! `SystemReminder::already_read_files(paths)` constructor produces the
//! same shape inline; coco-rs packages it as a dedicated generator so
//! the engine populates
//! [`GeneratorContext::already_read_file_paths`](crate::generator::GeneratorContext::already_read_file_paths)
//! and the orchestrator's tier / throttle gates apply uniformly.
//!
//! Gate chain:
//!
//! 1. `config.attachments.already_read_file` — default on.
//! 2. `ctx.already_read_file_paths` non-empty — nothing to dedup otherwise.
//!
//! The engine is responsible for populating the path list (scanning
//! `core/context`'s file-read tracker for this turn's deduped hits).

use async_trait::async_trait;

use crate::error::Result;
use crate::generator::AttachmentGenerator;
use crate::generator::GeneratorContext;
use crate::types::AlreadyReadFileMeta;
use crate::types::AttachmentType;
use crate::types::ReminderMetadata;
use crate::types::SystemReminder;
use coco_config::SystemReminderConfig;

#[derive(Debug, Default)]
pub struct AlreadyReadFileGenerator;

#[async_trait]
impl AttachmentGenerator for AlreadyReadFileGenerator {
    fn name(&self) -> &str {
        "AlreadyReadFileGenerator"
    }

    fn attachment_type(&self) -> AttachmentType {
        AttachmentType::AlreadyReadFile
    }

    fn is_enabled(&self, config: &SystemReminderConfig) -> bool {
        config.attachments.already_read_file
    }

    async fn generate(&self, ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        if ctx.already_read_file_paths.is_empty() {
            return Ok(None);
        }
        let meta = AlreadyReadFileMeta {
            paths: ctx.already_read_file_paths.clone(),
        };
        Ok(Some(SystemReminder::silent_attachment(
            AttachmentType::AlreadyReadFile,
            ReminderMetadata::AlreadyReadFile(meta),
        )))
    }
}

#[cfg(test)]
#[path = "already_read_file.test.rs"]
mod tests;
