//! Derived view of engine `MessageHistory` for the TUI.
//!
//! Authority remains with `coco_messages::MessageHistory` in the
//! engine; this struct is a TUI-side **pure derivation** rebuilt
//! incrementally from `ServerNotification::MessageAppended` /
//! `MessageTruncated` / `SessionResetForResume` events. See
//! `engine-tui-unified-transcript-plan.md` §6.1.
//!
//! The renderer pipeline reads `cells()` directly.
//!
//! Per-cell render layout (`cached_lines`, `cached_height`) is
//! intentionally not part of this struct. Layout caching lives in the
//! renderer at draw time.

use std::collections::HashMap;
use std::sync::Arc;

use coco_messages::Message;
use uuid::Uuid;

use crate::transcript::cells::RenderedCell;
use crate::transcript::derive::message_to_cells;

/// Append-only-with-truncation list of derived cells.
#[derive(Debug, Default)]
pub struct TranscriptView {
    cells: Vec<RenderedCell>,
    revision: u64,
    /// First cell index per source message UUID. One `Message` may
    /// derive multiple `RenderedCell`s (e.g. `Assistant` with text +
    /// thinking + tool_use blocks); the index points at the head cell
    /// of that group.
    by_uuid: HashMap<Uuid, usize>,
}

impl TranscriptView {
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn cells(&self) -> &[RenderedCell] {
        &self.cells
    }

    #[cfg(feature = "testing")]
    pub fn cells_for_test(&self) -> &[RenderedCell] {
        self.cells()
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    pub fn find_head_index_by_uuid(&self, uuid: &Uuid) -> Option<usize> {
        self.by_uuid.get(uuid).copied()
    }

    /// Append cells derived from `msg`. Multiple cells may be produced
    /// for one Message (e.g. assistant text + tool_use blocks); the
    /// UUID index records the first cell so consumers can find the
    /// group head. The owned `Arc<Message>` is shared into each cell
    /// so renderers can recover engine-side fields (`is_meta`,
    /// `permission_mode`, timestamp, …) without re-serializing.
    ///
    /// Re-emission of an already-seen UUID is a no-op (defensive
    /// dedup). The engine re-pushes the full prior history at the top
    /// of every turn (`run_session_loop` walks `turn_messages` and
    /// fires `history_push_and_emit` for each), so this guard
    /// prevents multi-turn sessions from accumulating a duplicate
    /// cell per turn. A `tracing::warn!` on the dedup path surfaces
    /// truly-accidental double-emission upstream (e.g. resume burst
    /// overlapping a live append, an engine bug pushing twice) so the
    /// silent dedup doesn't paper over real bugs. The expected
    /// turn-boundary re-emission shouldn't reach this branch because
    /// `engine.rs:575` batch-loads `turn_messages` via direct
    /// `MessageHistory::push` *before* the loop starts emitting; if
    /// it does, the warn marks it for investigation.
    pub fn on_message_appended(&mut self, msg: Arc<Message>) {
        if let Some(uuid) = msg.uuid()
            && self.by_uuid.contains_key(uuid)
        {
            tracing::warn!(
                target: "coco_tui::transcript_view",
                %uuid,
                "duplicate MessageAppended dropped — upstream emitted a uuid already in the derived view",
            );
            return;
        }
        let derived = message_to_cells(msg.clone());
        log_message_appended(&msg, derived.len());
        if derived.is_empty() {
            return;
        }
        let head_idx = self.cells.len();
        if let Some(uuid) = msg.uuid() {
            self.by_uuid.insert(*uuid, head_idx);
        }
        self.cells.extend(derived);
        self.bump_revision();
    }

    /// Truncate to the first `keep_count` ENGINE messages. Because one
    /// engine `Message` may have produced multiple cells, this walks
    /// `by_uuid` to find the cell index where engine-message
    /// `keep_count` begins and drops the tail.
    ///
    /// Phase 3a simplification: when the truncation target UUID can't
    /// be resolved (e.g. resume hasn't populated by_uuid yet), clamp
    /// by `keep_count` directly. Resume + auto-restore both go through
    /// the same path so this is robust enough.
    pub fn on_message_truncated(&mut self, keep_count: usize) {
        // Walk by_uuid to find the smallest cell index whose source
        // message had position >= keep_count. Since by_uuid maps the
        // engine message to its head cell index but doesn't carry the
        // engine message index, we approximate: count distinct head
        // UUIDs and stop when we've kept `keep_count` of them.
        let mut seen_heads = 0usize;
        let mut cut: Option<usize> = None;
        let mut last_uuid: Option<Uuid> = None;
        for (i, cell) in self.cells.iter().enumerate() {
            if last_uuid != Some(cell.message_uuid) {
                last_uuid = Some(cell.message_uuid);
                if seen_heads == keep_count {
                    cut = Some(i);
                    break;
                }
                seen_heads += 1;
            }
        }
        if let Some(c) = cut
            && c < self.cells.len()
        {
            self.cells.truncate(c);
            self.rebuild_index();
            self.bump_revision();
        }
    }

    pub fn on_session_reset(&mut self) {
        self.cells.clear();
        self.by_uuid.clear();
        self.bump_revision();
    }

    /// Replace the entire derived view with cells derived from
    /// `messages`. Use for `ServerNotification::HistoryReplaced` — the
    /// bulk resume path that avoids N round-trips through the
    /// per-message append path. Equivalent to
    /// [`Self::on_session_reset`] + N
    /// [`Self::on_message_appended`] calls but in a single
    /// cache-rebuild pass.
    pub fn replace_from_messages(&mut self, messages: &[Arc<Message>]) {
        self.cells.clear();
        self.by_uuid.clear();
        for arc in messages {
            let derived = message_to_cells(arc.clone());
            log_message_appended(arc, derived.len());
            if derived.is_empty() {
                continue;
            }
            let head_idx = self.cells.len();
            if let Some(uuid) = arc.uuid() {
                self.by_uuid.insert(*uuid, head_idx);
            }
            self.cells.extend(derived);
        }
        self.bump_revision();
    }

    fn rebuild_index(&mut self) {
        self.by_uuid.clear();
        let mut last_uuid: Option<Uuid> = None;
        for (i, cell) in self.cells.iter().enumerate() {
            if last_uuid != Some(cell.message_uuid) {
                self.by_uuid.insert(cell.message_uuid, i);
                last_uuid = Some(cell.message_uuid);
            }
        }
    }

    fn bump_revision(&mut self) {
        self.revision = self.revision.saturating_add(1);
    }
}

fn log_message_appended(msg: &Message, cell_count: usize) {
    let visibility = msg.visibility();
    let attachment_kind = match msg {
        Message::Attachment(attachment) => Some(attachment.kind),
        _ => None,
    };
    tracing::debug!(
        target: "coco_tui::transcript_view",
        uuid = ?msg.uuid(),
        kind = ?msg.kind(),
        cell_count,
        visibility_api = visibility.api,
        visibility_ui = visibility.ui,
        attachment_kind = ?attachment_kind,
        "MessageAppended projected into TUI cells",
    );
}

#[cfg(test)]
#[path = "transcript_view.test.rs"]
mod tests;
