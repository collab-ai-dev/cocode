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
//! Besides the derived cells, this struct also owns the
//! **session-cumulative message counters** ([`Self::cumulative_counts`]).
//! They are a fold over the same event stream — like the session token
//! accumulators, they survive intra-session transcript rewrites
//! (compaction / trim / rewind via `HistoryReplaced`) and reset only at
//! a true session boundary (`SessionResetForResume`). They are *not* a
//! function of the current cells.
//!
//! Per-cell render layout (`cached_lines`, `cached_height`) is
//! intentionally not part of this struct. Layout caching lives in the
//! renderer at draw time.

use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

use coco_messages::Message;
use coco_types::HistoryReplaceReason;
use uuid::Uuid;

use crate::transcript::cells::CellKind;
use crate::transcript::cells::RenderedCell;
use crate::transcript::derive::message_to_cells;

/// Session-cumulative message counts, deduped by message uuid and
/// classified by the head cell each message derives (matching what the
/// transcript actually renders — invisible messages produce no cells
/// and are never counted).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TranscriptCounts {
    pub users: usize,
    pub assistants: usize,
    pub tools: usize,
}

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
    /// Session-cumulative counters (status bar). Unlike `cells` /
    /// `by_uuid`, these survive `replace_from_messages` and truncation;
    /// only [`Self::on_session_reset`] zeroes them.
    cumulative: TranscriptCounts,
    /// Message uuids already folded into `cumulative`. Kept separate
    /// from `by_uuid` (which is rebuilt on every replace) so a rewind /
    /// compact snapshot cannot re-count messages it re-states.
    counted: HashSet<Uuid>,
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

    /// Session-cumulative user / assistant / tool message counts.
    pub fn cumulative_counts(&self) -> TranscriptCounts {
        self.cumulative
    }

    /// Fold one message into the cumulative counters, deduped by uuid.
    /// Classification uses the head cell (one bucket per message), the
    /// same rule the old per-frame cell walk applied.
    fn count_message(&mut self, uuid: Uuid, head: &RenderedCell) {
        if !self.counted.insert(uuid) {
            return;
        }
        match &head.kind {
            CellKind::UserText { .. } => self.cumulative.users += 1,
            CellKind::AssistantText { .. }
            | CellKind::AssistantThinking { .. }
            | CellKind::AssistantRedactedThinking { .. }
            | CellKind::ToolUse { .. } => self.cumulative.assistants += 1,
            CellKind::ToolResult { .. } => self.cumulative.tools += 1,
            CellKind::Attachment | CellKind::System(_) => {}
        }
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
            self.count_message(*uuid, &derived[0]);
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
        self.cumulative = TranscriptCounts::default();
        self.counted.clear();
        self.bump_revision();
    }

    /// Replace the entire derived view with cells derived from
    /// `messages`. Use for `ServerNotification::HistoryReplaced` — the
    /// bulk resume path that avoids N round-trips through the
    /// per-message append path. Equivalent to
    /// [`Self::on_session_reset`] + N
    /// [`Self::on_message_appended`] calls but in a single
    /// cache-rebuild pass.
    ///
    /// Cumulative counters never decrease here. Per `reason`:
    /// - `Hydrate` / `Trim` / `Rewind`: unseen messages are counted
    ///   normally (seeds the fold after a resume reset; a strict-subset
    ///   snapshot is a no-op).
    /// - `Compact`: snapshot messages are only marked seen — the
    ///   boundary / summary / re-injected attachments are compaction
    ///   artifacts, not organic conversation — and the summarizer's one
    ///   LLM response is folded as a single assistant message.
    pub fn replace_from_messages(
        &mut self,
        messages: &[Arc<Message>],
        reason: HistoryReplaceReason,
    ) {
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
                match reason {
                    HistoryReplaceReason::Compact => {
                        self.counted.insert(*uuid);
                    }
                    HistoryReplaceReason::Hydrate
                    | HistoryReplaceReason::Trim
                    | HistoryReplaceReason::Rewind => self.count_message(*uuid, &derived[0]),
                }
            }
            self.cells.extend(derived);
        }
        if reason == HistoryReplaceReason::Compact {
            self.cumulative.assistants += 1;
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
