//! The transcript cell model plus the tool-commit boundary for the native
//! finalize — the transcript-owner half of
//! `docs/coco-rs/ui/tui-v2-design.md` §6.4 / §6.5.
//!
//! [`RenderedCell`] / [`CellKind`] / [`SystemCellKind`] are the units every
//! renderer consumes; the `Message` -> cells derivation is
//! [`super::derive::message_to_cells`], and the incremental container that
//! tracks them per session is `state::transcript_view::TranscriptView`.
//!
//! Rows are immutable once they enter native scrollback, so the finalize may
//! only commit a leading cell prefix in which every `ToolUse` already pairs
//! with its forward `ToolResult`. `committable_prefix_len` computes that bound:
//! an unresolved `ToolUse` blocks the prefix until its result arrives; orphan
//! results don't block; duplicate call ids each need their own result.
use std::sync::Arc;

use coco_messages::Message;
use coco_messages::SystemMessage;
use uuid::Uuid;

/// One render cell derived from a (possibly partial) engine `Message`.
/// Carries an `Arc<Message>` back-pointer so renderers can extract
/// engine-authoritative fields (`is_meta`, `permission_mode`,
/// `timestamp`, `is_compact_summary`, ...) without parallel storage.
/// Layout / viewport-dependent fields are intentionally absent —
/// layout caching lives in the renderer at draw time.
#[derive(Debug, Clone)]
pub struct RenderedCell {
    pub message_uuid: Uuid,
    pub kind: CellKind,
    pub source: Arc<Message>,
}

/// TUI-internal classification used for render dispatch.
///
/// Mirrors but is not identical to `coco_messages::Message` variants —
/// several `SystemMessage` sub-variants may render the same way, and
/// `AssistantMessage` content blocks map to multiple `CellKind`s
/// (text + thinking + tool_use).
///
/// Phase 3a keeps `CellKind` flat with enough fidelity to drive a
/// future renderer; field-level rendering data (markdown AST cache,
/// diff hunks, etc.) is not stored here per layer-hygiene rule from
/// `engine-tui-unified-transcript-plan.md` §2.
#[derive(Debug, Clone)]
pub enum CellKind {
    /// User text input.
    UserText { text: String },
    /// Assistant text fragment.
    AssistantText { text: String, model: String },
    /// Assistant reasoning / thinking content.
    ///
    /// Reasoning metadata (`duration_ms`, `reasoning_tokens`) lives in
    /// `SessionState.reasoning_metadata` keyed by `message_uuid` —
    /// the engine reports it on `TurnCompleted`, after the cell has
    /// already been derived from `&Message`. Side-cache keeps the
    /// cell a pure function of the source message (I-2).
    AssistantThinking { text: String, metadata_anchor: bool },
    /// Assistant redacted thinking (encrypted, displayed as opaque).
    AssistantRedactedThinking,
    /// Assistant `tool_use` content block.
    ToolUse { call_id: String, tool_name: String },
    /// Tool result returned to the model.
    ToolResult { call_id: String },
    /// Attachment message (system-reminder-wrapped queued command,
    /// hook payload, etc.).
    Attachment,
    /// System message — fine-grained kind drives render style.
    System(SystemCellKind),
}

/// Render-side classification of `SystemMessage` sub-variants.
///
/// Phase 3a includes only the kinds the engine actively emits. New
/// variants land alongside their `SystemMessage` extension.
#[derive(Debug, Clone)]
pub enum SystemCellKind {
    /// User cancellation — renders dim "Interrupted · ..." row.
    /// `for_tool_use` is read from the engine-authoritative field and
    /// never recomputed (eliminates the prior engine <-> TUI race).
    UserInterruption { for_tool_use: bool },
    /// Generic informational system row (level + title + body).
    Informational,
    /// API error reported by the engine.
    ApiError,
    /// Compaction boundary marker.
    CompactBoundary,
    /// Micro-compaction boundary (rare; usually filtered).
    MicrocompactBoundary,
    /// Local /command output preserved in transcript.
    LocalCommand,
    /// Permission-retry banner.
    PermissionRetry,
    /// IDE bridge connection status.
    BridgeStatus,
    /// Memory file saved (extract / dream).
    MemorySaved,
    /// Away-summary system row.
    AwaySummary,
    /// Agents killed system row.
    AgentsKilled,
    /// API metrics tail row.
    ApiMetrics,
    /// Stop-hook summary row.
    StopHookSummary,
    /// Turn duration row.
    TurnDuration,
    /// Scheduled task fire row.
    ScheduledTaskFire,
    /// `/context` usage snapshot — colored grid + grouped detail block.
    ContextUsage,
}

impl From<&SystemMessage> for SystemCellKind {
    fn from(m: &SystemMessage) -> Self {
        match m {
            SystemMessage::UserInterruption(i) => Self::UserInterruption {
                for_tool_use: i.for_tool_use,
            },
            SystemMessage::Informational(_) => Self::Informational,
            SystemMessage::ApiError(_) => Self::ApiError,
            SystemMessage::CompactBoundary(_) => Self::CompactBoundary,
            SystemMessage::MicrocompactBoundary(_) => Self::MicrocompactBoundary,
            SystemMessage::LocalCommand(_) => Self::LocalCommand,
            SystemMessage::PermissionRetry(_) => Self::PermissionRetry,
            SystemMessage::BridgeStatus(_) => Self::BridgeStatus,
            SystemMessage::MemorySaved(_) => Self::MemorySaved,
            SystemMessage::AwaySummary(_) => Self::AwaySummary,
            SystemMessage::AgentsKilled(_) => Self::AgentsKilled,
            SystemMessage::ApiMetrics(_) => Self::ApiMetrics,
            SystemMessage::StopHookSummary(_) => Self::StopHookSummary,
            SystemMessage::TurnDuration(_) => Self::TurnDuration,
            SystemMessage::ScheduledTaskFire(_) => Self::ScheduledTaskFire,
            SystemMessage::ContextUsage(_) => Self::ContextUsage,
        }
    }
}

/// Indices into `cells` where each engine message begins — one entry per
/// `message_uuid` run (an assistant turn's fanout cells share their first
/// index). The shared grouping primitive for emission counting and replay
/// truncation.
pub(crate) fn engine_message_starts(cells: &[RenderedCell]) -> impl Iterator<Item = usize> + '_ {
    let mut prev = None;
    cells.iter().enumerate().filter_map(move |(index, cell)| {
        if Some(cell.message_uuid) != prev {
            prev = Some(cell.message_uuid);
            Some(index)
        } else {
            None
        }
    })
}

/// Length of the leading cell prefix that is safe to insert into native
/// scrollback: every `ToolUse` in the prefix must pair with a forward,
/// not-yet-consumed `ToolResult`. The first `ToolUse` without one truncates
/// the prefix at its engine-message start — the whole message stays live in
/// the viewport until the tool resolves.
pub(crate) fn committable_prefix_len(cells: &[RenderedCell]) -> usize {
    let mut consumed_results = vec![false; cells.len()];
    for (index, cell) in cells.iter().enumerate() {
        if let CellKind::ToolUse { call_id, .. } = &cell.kind {
            let Some(result_index) =
                find_forward_unconsumed_tool_result(cells, &consumed_results, index + 1, call_id)
            else {
                return engine_message_start_containing(cells, index);
            };
            consumed_results[result_index] = true;
        }
    }
    cells.len()
}

fn find_forward_unconsumed_tool_result(
    cells: &[RenderedCell],
    consumed_results: &[bool],
    start: usize,
    call_id: &str,
) -> Option<usize> {
    for (index, cell) in cells.iter().enumerate().skip(start) {
        if consumed_results[index] {
            continue;
        }
        if let CellKind::ToolResult {
            call_id: result_call_id,
        } = &cell.kind
            && result_call_id == call_id
        {
            return Some(index);
        }
    }
    None
}

/// First index of the engine-message run containing `index` (backward scan
/// over the `message_uuid` group).
fn engine_message_start_containing(cells: &[RenderedCell], index: usize) -> usize {
    let uuid = cells[index].message_uuid;
    let mut start = index;
    while start > 0 && cells[start - 1].message_uuid == uuid {
        start -= 1;
    }
    start
}
