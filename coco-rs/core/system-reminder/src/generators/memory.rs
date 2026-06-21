//! Memory reminder generators (2 variants).
//!
//! - `NestedMemoryGenerator` — `nested_memory` attachment. Fires per-turn
//!   when @-mention traversal surfaced nested CLAUDE.md / memory files.
//!   One text reminder per nested-memory entry, joined by `\n\n` within
//!   a single `<system-reminder>` to keep the XML tag count stable.
//!
//! - `RelevantMemoriesGenerator` — `relevant_memories` attachment.
//!   Multi-message reminder: one user message per memory entry, wrapped
//!   in a single `<system-reminder>`. Async-prefetched; engine awaits
//!   the prefetch at turn start.
//!
//! **Data flow**: the owning `memory` / `context` crates materialize
//! `Vec<NestedMemoryInfo>` / `Vec<RelevantMemoryInfo>` into ctx.
//!
//! **Scope**: the data is already modeled in
//! `core/context::Attachment::{NestedMemory, RelevantMemories}`.
//! These generators render the per-turn reminder text; the context
//! crate + memory crate own storage + retrieval.

use async_trait::async_trait;

use crate::error::Result;
use crate::generator::AttachmentGenerator;
use crate::generator::GeneratorContext;
use crate::types::AttachmentType;
use crate::types::ContentBlock;
use crate::types::MessageRole;
use crate::types::ReminderMessage;
use crate::types::ReminderOutput;
use crate::types::SystemReminder;
use coco_config::SystemReminderConfig;

// ---------------------------------------------------------------------------
// Snapshot types (populated by engine from context::Attachment variants)
// ---------------------------------------------------------------------------

/// Single nested-memory entry surfaced by @-mention traversal.
///
/// Mirrors `coco_context::NestedMemoryAttachment.content` (carries `path`
/// and `content` fields from the nested struct).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NestedMemoryInfo {
    pub path: String,
    pub content: String,
}

/// Single relevant-memory entry. Mirrors
/// `coco_context::RelevantMemoryEntry` — engine maps directly.
///
/// `header` is pre-computed at attachment-creation time so rendered
/// bytes are stable across turns (prompt-cache hit); fall back to a
/// synthesized header if None (resumed sessions that predate the
/// stored-header field).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RelevantMemoryInfo {
    pub path: String,
    pub content: String,
    pub mtime_ms: i64,
    pub header: Option<String>,
}

// ---------------------------------------------------------------------------
// NestedMemoryGenerator
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct NestedMemoryGenerator;

#[async_trait]
impl AttachmentGenerator for NestedMemoryGenerator {
    fn name(&self) -> &str {
        "NestedMemoryGenerator"
    }

    fn attachment_type(&self) -> AttachmentType {
        AttachmentType::NestedMemory
    }

    fn is_enabled(&self, config: &SystemReminderConfig) -> bool {
        config.attachments.nested_memory
    }

    async fn generate(&self, ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        if ctx.nested_memories.is_empty() {
            return Ok(None);
        }
        // Format per entry: `Contents of ${path}:\n\n${content}`.
        // Collapsed into one text reminder with `\n\n` separators so
        // the XML wrapping stays one pair of `<system-reminder>` tags.
        let parts: Vec<String> = ctx
            .nested_memories
            .iter()
            .filter(|m| !m.content.is_empty())
            .map(|m| {
                format!(
                    "Contents of {path}:\n\n{content}",
                    path = m.path,
                    content = m.content
                )
            })
            .collect();
        if parts.is_empty() {
            return Ok(None);
        }
        Ok(Some(SystemReminder::new(
            AttachmentType::NestedMemory,
            parts.join("\n\n"),
        )))
    }
}

// ---------------------------------------------------------------------------
// RelevantMemoriesGenerator
// ---------------------------------------------------------------------------

/// Produces a multi-message reminder — one user message per memory
/// entry — inside a single `<system-reminder>` wrapper.
#[derive(Debug, Default)]
pub struct RelevantMemoriesGenerator;

#[async_trait]
impl AttachmentGenerator for RelevantMemoriesGenerator {
    fn name(&self) -> &str {
        "RelevantMemoriesGenerator"
    }

    fn attachment_type(&self) -> AttachmentType {
        AttachmentType::RelevantMemories
    }

    fn is_enabled(&self, config: &SystemReminderConfig) -> bool {
        config.attachments.relevant_memories
    }

    async fn generate(&self, ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        if ctx.relevant_memories.is_empty() {
            return Ok(None);
        }
        let messages: Vec<ReminderMessage> = ctx
            .relevant_memories
            .iter()
            .filter(|m| !m.content.is_empty())
            .map(|m| {
                let header = m
                    .header
                    .clone()
                    .unwrap_or_else(|| fallback_header(&m.path, m.mtime_ms));
                ReminderMessage {
                    role: MessageRole::User,
                    blocks: vec![ContentBlock::Text {
                        text: format!("{header}\n\n{content}", content = m.content),
                    }],
                    is_meta: true,
                }
            })
            .collect();
        if messages.is_empty() {
            return Ok(None);
        }
        Ok(Some(SystemReminder {
            attachment_type: AttachmentType::RelevantMemories,
            output: ReminderOutput::Messages(messages),
            is_meta: true,
            is_silent: false,
            metadata: None,
        }))
    }
}

/// Fallback header for pre-existing relevant-memory entries that lack
/// a stored `header`. The expected format is
/// `Memory: ${path} (last modified ${relativeAge})`; without access to a
/// relative-age helper here we emit a minimal stable variant. Engine
/// should populate `header` whenever possible to preserve prompt-cache
/// stability across turns.
fn fallback_header(path: &str, _mtime_ms: i64) -> String {
    format!("Memory: {path}")
}

#[cfg(test)]
#[path = "memory.test.rs"]
mod tests;
