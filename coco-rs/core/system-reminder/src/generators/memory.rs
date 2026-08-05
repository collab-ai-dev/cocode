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
use coco_context::ContextualUserFragment;

/// Lead-in prepended to the first relevant-memory entry — these are retrieved
/// by similarity, so the model is told to apply them only if they fit.
const RELEVANT_MEMORIES_LEAD_IN: &str = "Retrieved for possible relevance \u{2014} use only if it actually applies to what the user asked.\n\n";
const MAX_NESTED_MEMORY_ENTRY_BYTES: usize = 16_000;
const MAX_NESTED_MEMORY_TOTAL_BYTES: usize = 32_000;
const NESTED_MEMORY_ENTRY_TRUNCATED: &str = "\n...[nested memory entry truncated]...";
const NESTED_MEMORY_TOTAL_TRUNCATED: &str = "\n\n...[additional nested memories truncated]...";
const MAX_RELEVANT_MEMORY_ENTRY_BYTES: usize = 16_000;
const MAX_RELEVANT_MEMORY_TOTAL_BYTES: usize = 32_000;

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
        let fragment_overhead =
            coco_context::BoundedExternalContextFragment::minimum_rendered_bytes(
                coco_context::ContextFragmentKind::NestedMemory,
            );
        let content_budget = MAX_NESTED_MEMORY_TOTAL_BYTES.saturating_sub(fragment_overhead);
        let mut rendered = String::with_capacity(content_budget);
        for memory in ctx
            .nested_memories
            .iter()
            .filter(|memory| !memory.content.is_empty())
        {
            let entry = truncate_with_marker(
                &format!(
                    "Contents of {path}:\n\n{content}",
                    path = memory.path,
                    content = memory.content
                ),
                MAX_NESTED_MEMORY_ENTRY_BYTES,
                NESTED_MEMORY_ENTRY_TRUNCATED,
            );
            let separator = if rendered.is_empty() { "" } else { "\n\n" };
            if rendered
                .len()
                .saturating_add(separator.len())
                .saturating_add(entry.len())
                > content_budget
            {
                append_complete_marker(
                    &mut rendered,
                    content_budget,
                    NESTED_MEMORY_TOTAL_TRUNCATED,
                );
                break;
            }
            rendered.push_str(separator);
            rendered.push_str(&entry);
        }
        if rendered.is_empty() {
            return Ok(None);
        }
        let rendered = coco_context::BoundedExternalContextFragment::new(
            coco_context::ContextFragmentKind::NestedMemory,
            rendered,
            MAX_NESTED_MEMORY_TOTAL_BYTES,
        )
        .render();
        Ok(Some(SystemReminder::new(
            AttachmentType::NestedMemory,
            rendered,
        )))
    }
}

fn truncate_with_marker(text: &str, budget: usize, marker: &str) -> String {
    if text.len() <= budget {
        return text.to_string();
    }
    if budget <= marker.len() {
        return coco_utils_string::take_bytes_at_char_boundary(text, budget).to_string();
    }
    let mut output = String::with_capacity(budget);
    output.push_str(coco_utils_string::take_bytes_at_char_boundary(
        text,
        budget - marker.len(),
    ));
    output.push_str(marker);
    output
}

fn append_complete_marker(output: &mut String, budget: usize, marker: &str) {
    if budget <= marker.len() {
        output.clear();
        output.push_str(coco_utils_string::take_bytes_at_char_boundary(
            marker, budget,
        ));
        return;
    }
    let content_budget = budget - marker.len();
    output.truncate(output.floor_char_boundary(content_budget));
    output.push_str(marker);
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
        let mut messages: Vec<ReminderMessage> = Vec::new();
        let mut remaining = MAX_RELEVANT_MEMORY_TOTAL_BYTES;
        for (i, m) in ctx
            .relevant_memories
            .iter()
            .filter(|m| !m.content.is_empty())
            .enumerate()
        {
            let header = m
                .header
                .clone()
                .unwrap_or_else(|| fallback_header(&m.path, m.mtime_ms));
            // Lead-in on the first entry only: these were retrieved by
            // similarity, not necessarily relevance, so steer the model to
            // use them only if they actually apply (CC's `o===0` gate).
            let lead_in = if i == 0 {
                RELEVANT_MEMORIES_LEAD_IN
            } else {
                ""
            };
            let text = coco_context::BoundedExternalContextFragment::new(
                coco_context::ContextFragmentKind::RelevantMemory,
                format!("{lead_in}{header}\n\n{content}", content = m.content),
                remaining.min(MAX_RELEVANT_MEMORY_ENTRY_BYTES),
            )
            .render();
            if text.is_empty() {
                break;
            }
            remaining = remaining.saturating_sub(text.len());
            messages.push(ReminderMessage {
                role: MessageRole::User,
                blocks: vec![ContentBlock::Text { text }],
                is_meta: true,
            });
            if remaining == 0 {
                break;
            }
        }
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
