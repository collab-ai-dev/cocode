//! Transcript state presentation.
//!
//! Consumes `&[RenderedCell]` directly. `TranscriptCell` indices point
//! into the cells slice; batch detection (tool batches) dispatches on
//! `CellKind`. Hooks land via `Attachment`, not the transcript, and
//! task notifications are no longer XML-wrapped — so there are no
//! hook-batch or task-notification variants here.
//!
//! See `engine-tui-phase3d-renderer-migration-plan.md` §6.

use std::collections::BTreeMap;
use std::collections::VecDeque;

use crate::presentation::streaming::StreamingTailInput;
use crate::presentation::streaming::StreamingTailView;
use crate::presentation::streaming::streaming_tail_view;
use crate::state::AppState;
use crate::state::session::ToolExecution;
use crate::state::session::ToolStatus;
use crate::state::transcript::TranscriptCellId;
use crate::state::ui::StreamingState;
use crate::transcript::cells::CellKind;
use crate::transcript::cells::RenderedCell;
use crate::transcript::cells::SystemCellKind;

pub(crate) const TRANSCRIPT_COLLAPSED_PREVIEW_LINES: usize = 5;
pub(crate) const TRANSCRIPT_EXPANDED_CELL_LINE_CAP: usize = 2_000;
pub(crate) const TRANSCRIPT_LINE_CHAR_CAP: usize = 512;
pub(crate) const TRANSCRIPT_TRUNCATED_HINT: &str = "… output truncated in UI";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ToolOutputPreview<'a> {
    Empty,
    Full(Vec<&'a str>),
    Truncated {
        head: Vec<&'a str>,
        omitted: usize,
        tail: Vec<&'a str>,
    },
}

pub(crate) fn tool_output_preview(output: &str, max_rows: usize) -> ToolOutputPreview<'_> {
    if max_rows == 0 {
        return ToolOutputPreview::Empty;
    }

    let visible_rows = max_rows.saturating_sub(1);
    let head_limit = visible_rows / 2;
    let tail_limit = visible_rows.saturating_sub(head_limit);
    let mut short = Vec::with_capacity(max_rows);
    let mut head = Vec::with_capacity(head_limit);
    let mut tail = VecDeque::with_capacity(tail_limit);
    let mut total = 0usize;

    for line in output.lines() {
        if total < max_rows {
            short.push(line);
        }
        if total < head_limit {
            head.push(line);
        } else if tail_limit > 0 {
            if tail.len() == tail_limit {
                tail.pop_front();
            }
            tail.push_back(line);
        }
        total += 1;
    }

    if total == 0 {
        return ToolOutputPreview::Empty;
    }
    if total <= max_rows {
        return ToolOutputPreview::Full(short);
    }

    ToolOutputPreview::Truncated {
        omitted: total.saturating_sub(head.len() + tail.len()),
        head,
        tail: tail.into_iter().collect(),
    }
}

/// One transcript-presentation cell. Indices point into the
/// `&[RenderedCell]` slice passed to `transcript_projection`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TranscriptCell {
    /// Collapsed system-reminder preview row.
    MetaPreview { index: usize },
    /// Standalone cell — assistant text, user text, etc.
    Cell { index: usize },
    /// Paired tool invocation + result.
    ToolCall {
        invocation: Option<usize>,
        result: Option<usize>,
        call_id: Option<String>,
    },
    /// Multiple adjacent tool invocations without intervening results.
    ToolBatch {
        start: usize,
        end: usize,
        count: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TranscriptSourceCell<'a> {
    Committed(TranscriptCell),
    Active(ActiveTranscriptCell<'a>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ActiveTranscriptCell<'a> {
    Streaming(StreamingTailView<'a>),
    /// One `● Tool(args) (Ns)` row per still-executing tool. Rendered from the
    /// live `tool_executions` ledger — see [`is_tool_in_flight`].
    InFlightTools,
}

/// A tool that is still executing — no `ToolResult` has committed for it yet.
///
/// Until the result pairs with the call, `committable_prefix_len`
/// (`transcript::cells`) holds the committed `● Tool` header *out* of native
/// scrollback, and it is not part of the active streaming tail either — so the
/// header renders nowhere during execution. This predicate drives the inline
/// row that fills that gap.
///
/// `message_uuid` is deliberately NOT consulted: the header is withheld whether
/// or not the owning assistant message has committed. A tool leaves this set
/// the instant its result lands — which is exactly when its committed header
/// becomes paintable — so the inline row and the committed header never overlap.
pub(crate) fn is_tool_in_flight(tool: &ToolExecution) -> bool {
    matches!(
        tool.status,
        ToolStatus::Streaming | ToolStatus::Queued | ToolStatus::Running
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TranscriptProjectionOptions {
    pub show_system_reminders: bool,
    /// Show compact boundary + compact summary internals.
    ///
    /// Default chat/native scrollback keeps these hidden and shows the
    /// `/compact` command result instead. The Ctrl+O transcript reader
    /// sets this to true so the full summary remains available.
    pub show_compact_internals: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TranscriptProjection {
    pub cells: Vec<TranscriptCell>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AssistantPresentationOrder {
    Source,
    TextBeforeLeadingThinking,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct TranscriptPresentationInput<'cells, 'state> {
    /// Engine-derived cells — single source of truth. `'cells` is
    /// decoupled from `'state` so callers can pass a slice borrowed
    /// from a temporary (rare; `state.session.transcript.cells()`
    /// usually borrows from `state` directly).
    pub cells: &'cells [RenderedCell],
    pub options: TranscriptProjectionOptions,
    pub streaming: Option<&'state StreamingState>,
    pub show_thinking: bool,
    pub tool_executions: &'state [ToolExecution],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TranscriptPresentation<'a> {
    pub cells: Vec<TranscriptSourceCell<'a>>,
}

pub(crate) fn transcript_projection(
    cells: &[RenderedCell],
    options: TranscriptProjectionOptions,
) -> TranscriptProjection {
    transcript_projection_with_assistant_order(cells, options, AssistantPresentationOrder::Source)
}

pub(crate) fn transcript_projection_with_assistant_order(
    cells: &[RenderedCell],
    options: TranscriptProjectionOptions,
    assistant_order: AssistantPresentationOrder,
) -> TranscriptProjection {
    let show_system_reminders = options.show_system_reminders;
    let show_compact_internals = options.show_compact_internals;
    let mut out = Vec::new();
    let mut consumed = vec![false; cells.len()];
    let mut i = 0;
    while i < cells.len() {
        if consumed[i] {
            i += 1;
            continue;
        }
        let cell = &cells[i];

        if assistant_order == AssistantPresentationOrder::TextBeforeLeadingThinking
            && let Some(end) = push_text_first_assistant_group(cells, i, &mut out)
        {
            i = end;
            continue;
        }

        if !show_compact_internals && is_compact_internal(cell) {
            i += 1;
            continue;
        }

        // System reminders collapse to one-line preview unless the
        // user explicitly opted in via show_system_reminders.
        if is_meta(cell) && !show_system_reminders {
            out.push(TranscriptCell::MetaPreview { index: i });
            i += 1;
            continue;
        }

        // Tool-use batch: 2+ adjacent ToolUse cells (allowing meta
        // cells between them) render a single batch header before
        // the individual paired invocation/result rows.
        let batch_end = tool_batch_end(cells, i);
        let batch_tool_count = tool_use_count(cells, i, batch_end);
        if is_tool_batch_start(cells, i) && batch_tool_count > 1 {
            log_tool_batch(cells, &consumed, i, batch_end, batch_tool_count);
            out.push(TranscriptCell::ToolBatch {
                start: i,
                end: batch_end,
                count: batch_tool_count,
            });
        }

        // Tool invocation paired with its result.
        if let CellKind::ToolUse { call_id, .. } = &cell.kind {
            let result = find_tool_result(cells, &consumed, i + 1, call_id);
            if let Some(r) = result {
                consumed[r] = true;
            }
            out.push(TranscriptCell::ToolCall {
                invocation: Some(i),
                result,
                call_id: Some(call_id.clone()),
            });
            i += 1;
            continue;
        }

        // Orphan tool result (engine emitted ToolResult without a
        // matching ToolUse in scope — fallback path).
        if is_tool_result(cell) {
            let call_id = match &cell.kind {
                CellKind::ToolResult { call_id } => Some(call_id.clone()),
                _ => None,
            };
            out.push(TranscriptCell::ToolCall {
                invocation: None,
                result: Some(i),
                call_id,
            });
            i += 1;
            continue;
        }

        out.push(TranscriptCell::Cell { index: i });
        i += 1;
    }
    TranscriptProjection { cells: out }
}

pub(crate) fn transcript_presentation<'cells, 'state>(
    input: TranscriptPresentationInput<'cells, 'state>,
) -> TranscriptPresentation<'state> {
    transcript_presentation_with_assistant_order(input, AssistantPresentationOrder::Source)
}

pub(crate) fn native_history_presentation<'cells, 'state>(
    input: TranscriptPresentationInput<'cells, 'state>,
) -> TranscriptPresentation<'state> {
    transcript_presentation_with_assistant_order(
        input,
        AssistantPresentationOrder::TextBeforeLeadingThinking,
    )
}

pub(crate) fn transcript_presentation_with_assistant_order<'cells, 'state>(
    input: TranscriptPresentationInput<'cells, 'state>,
    assistant_order: AssistantPresentationOrder,
) -> TranscriptPresentation<'state> {
    let projection = if assistant_order == AssistantPresentationOrder::Source {
        transcript_projection(input.cells, input.options)
    } else {
        transcript_projection_with_assistant_order(input.cells, input.options, assistant_order)
    };
    let mut cells = projection
        .cells
        .into_iter()
        .map(TranscriptSourceCell::Committed)
        .collect::<Vec<_>>();
    if let Some(active) =
        active_transcript_cell(input.streaming, input.show_thinking, input.tool_executions)
    {
        cells.push(TranscriptSourceCell::Active(active));
    }
    TranscriptPresentation { cells }
}

/// Reorder an assistant message's leading thinking cells AFTER its first
/// text cell: `[thinking…, text, rest…]` renders as `[text, thinking…]` and
/// resumes the main projection loop at `rest` (tool calls etc.).
///
/// The rule is deliberately independent of what FOLLOWS the text cell, so a
/// message renders identically whether it is projected mid-turn (committable
/// slice ends at the text because its tool uses are still unresolved) or
/// after its tool results paired — and so the Policy B streamed text rows,
/// which always enter native scrollback first, are the leading rows of the
/// group under both incremental append and full replay.
fn push_text_first_assistant_group(
    cells: &[RenderedCell],
    start: usize,
    out: &mut Vec<TranscriptCell>,
) -> Option<usize> {
    if !is_assistant_thinking(&cells[start]) {
        return None;
    }

    let uuid = cells[start].message_uuid;
    let mut first_non_thinking = start;
    while first_non_thinking < cells.len()
        && cells[first_non_thinking].message_uuid == uuid
        && is_assistant_thinking(&cells[first_non_thinking])
    {
        first_non_thinking += 1;
    }
    let text = cells.get(first_non_thinking)?;
    if text.message_uuid != uuid || !matches!(text.kind, CellKind::AssistantText { .. }) {
        return None;
    }

    out.push(TranscriptCell::Cell {
        index: first_non_thinking,
    });
    for index in start..first_non_thinking {
        out.push(TranscriptCell::Cell { index });
    }
    Some(first_non_thinking + 1)
}

pub(crate) fn active_transcript_cell<'a>(
    streaming: Option<&'a StreamingState>,
    show_thinking: bool,
    tool_executions: &[ToolExecution],
) -> Option<ActiveTranscriptCell<'a>> {
    if streaming.is_some() {
        return streaming.map(|streaming| {
            ActiveTranscriptCell::Streaming(streaming_tail_view(StreamingTailInput {
                streaming,
                show_thinking,
            }))
        });
    }
    // Any still-executing tool: its committed `● Tool` header is held out of
    // scrollback by `committable_prefix_len` until the result pairs, so render
    // the live header inline here. This is the only place a running tool shows
    // up between the turn's text and its result — without it the transcript
    // sits on a bare spinner for the whole execution (a multi-minute `Agent`
    // run looks frozen until it finishes). The committed header takes over the
    // instant the result lands; see [`is_tool_in_flight`].
    // Agent spawns are excluded: their live progress shows in the Agents
    // panel (`agent_surface`), and `in_flight_tool_lines` filters them out,
    // so gating on them here would render an empty in-flight cell.
    if tool_executions
        .iter()
        .any(|t| is_tool_in_flight(t) && t.name != coco_types::ToolName::Agent.as_str())
    {
        return Some(ActiveTranscriptCell::InFlightTools);
    }
    None
}

fn is_meta(cell: &RenderedCell) -> bool {
    // Attachments: defer to the engine's single predicate so the TUI can never
    // contradict it. `derive::message_to_cells` already drops attachments with
    // `renders_in_transcript() == false`, so every surviving `CellKind::
    // Attachment` is content (`is_meta_message == false`) and renders as a row,
    // not a collapsed "# [meta]" preview — renderable attachments are
    // first-class content. System reminders still collapse.
    match &cell.kind {
        CellKind::Attachment => coco_messages::predicates::is_meta_message(cell.source.as_ref()),
        CellKind::System(SystemCellKind::Informational) => {
            let coco_messages::Message::System(coco_messages::SystemMessage::Informational(info)) =
                cell.source.as_ref()
            else {
                return true;
            };
            !info.title.is_empty()
        }
        // `/context` snapshot is first-class content, not a collapsible system
        // reminder — render the full colored block.
        CellKind::System(SystemCellKind::ContextUsage) => false,
        CellKind::System(SystemCellKind::UserInterruption { .. }) => false,
        CellKind::System(_) => true,
        _ => false,
    }
}

fn is_compact_internal(cell: &RenderedCell) -> bool {
    match cell.source.as_ref() {
        coco_messages::Message::User(user) if user.is_compact_summary => true,
        coco_messages::Message::System(coco_messages::SystemMessage::CompactBoundary(_)) => true,
        _ => false,
    }
}

fn tool_batch_end(cells: &[RenderedCell], start: usize) -> usize {
    let is_tool_use = |c: &RenderedCell| matches!(c.kind, CellKind::ToolUse { .. });
    if !is_tool_use(&cells[start]) {
        return start + 1;
    }
    let mut end = start + 1;
    while end < cells.len() {
        let next = &cells[end];
        if is_tool_use(next) || is_meta(next) {
            end += 1;
        } else {
            break;
        }
    }
    end
}

fn is_tool_batch_start(cells: &[RenderedCell], index: usize) -> bool {
    if !matches!(cells[index].kind, CellKind::ToolUse { .. }) {
        return false;
    }
    let mut cursor = index;
    while cursor > 0 {
        let previous = &cells[cursor - 1];
        if matches!(previous.kind, CellKind::ToolUse { .. }) {
            return false;
        }
        if !is_meta(previous) {
            break;
        }
        cursor -= 1;
    }
    true
}

fn tool_use_count(cells: &[RenderedCell], start: usize, end: usize) -> usize {
    cells[start..end]
        .iter()
        .filter(|cell| matches!(cell.kind, CellKind::ToolUse { .. }))
        .count()
}

/// Tool names inside a batch range, sorted by name with repeats collapsed as
/// `Name ×N` — so the batch header tells the user which tools are running
/// before any result lands.
pub(crate) fn tool_batch_name_summary(cells: &[RenderedCell], start: usize, end: usize) -> String {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for cell in &cells[start..end.min(cells.len())] {
        if let CellKind::ToolUse { tool_name, .. } = &cell.kind {
            *counts.entry(tool_name.as_str()).or_insert(0) += 1;
        }
    }
    counts
        .into_iter()
        .map(|(name, n)| {
            if n > 1 {
                format!("{name} ×{n}")
            } else {
                name.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn log_tool_batch(
    cells: &[RenderedCell],
    consumed: &[bool],
    start: usize,
    end: usize,
    count: usize,
) {
    let pairings = cells[start..end]
        .iter()
        .filter_map(|cell| {
            let CellKind::ToolUse { call_id, .. } = &cell.kind else {
                return None;
            };
            Some((
                call_id.as_str(),
                find_tool_result(cells, consumed, end, call_id).is_some(),
            ))
        })
        .collect::<Vec<_>>();
    tracing::debug!(
        target: "coco_tui::transcript",
        start,
        end,
        count,
        ?pairings,
        "projected parallel tool batch",
    );
}

fn find_tool_result(
    cells: &[RenderedCell],
    consumed: &[bool],
    start: usize,
    call_id: &str,
) -> Option<usize> {
    for i in start..cells.len() {
        if consumed[i] {
            continue;
        }
        if let CellKind::ToolResult {
            call_id: result_call_id,
        } = &cells[i].kind
            && result_call_id == call_id
        {
            return Some(i);
        }
    }
    None
}

fn is_tool_result(cell: &RenderedCell) -> bool {
    matches!(cell.kind, CellKind::ToolResult { .. })
}

fn is_assistant_thinking(cell: &RenderedCell) -> bool {
    matches!(
        cell.kind,
        CellKind::AssistantThinking { .. } | CellKind::AssistantRedactedThinking
    )
}

impl TranscriptCell {
    pub(crate) fn cell_id(&self, cells: &[RenderedCell]) -> Option<TranscriptCellId> {
        match self {
            Self::ToolCall {
                call_id: Some(call_id),
                ..
            } => Some(TranscriptCellId::tool(call_id.clone())),
            Self::ToolCall {
                invocation: Some(index),
                ..
            }
            | Self::ToolCall {
                result: Some(index),
                ..
            }
            | Self::MetaPreview { index }
            | Self::Cell { index } => Some(TranscriptCellId::message(
                *index,
                cells.get(*index)?.message_uuid.to_string(),
            )),
            Self::ToolCall { .. } => None,
            Self::ToolBatch { start, end, .. } => Some(TranscriptCellId::tool_batch(*start, *end)),
        }
    }
}

impl<'a> TranscriptSourceCell<'a> {
    pub(crate) fn cell_id(&self, cells: &[RenderedCell]) -> Option<TranscriptCellId> {
        match self {
            Self::Committed(cell) => cell.cell_id(cells),
            Self::Active(_) => Some(TranscriptCellId::ActiveTail),
        }
    }

    pub(crate) fn is_expandable(&self, cells: &[RenderedCell]) -> bool {
        match self {
            Self::Committed(TranscriptCell::ToolCall { .. }) => true,
            Self::Committed(TranscriptCell::Cell { index }) => {
                cells.get(*index).is_some_and(cell_is_expandable)
            }
            Self::Committed(_) | Self::Active(_) => false,
        }
    }
}

fn cell_is_expandable(cell: &RenderedCell) -> bool {
    match &cell.kind {
        CellKind::AssistantThinking { text, .. } => !text.is_empty(),
        CellKind::ToolResult { .. } => true,
        CellKind::UserText { .. } | CellKind::AssistantText { .. } => false,
        _ => false,
    }
}

pub(crate) fn transcript_expandable_cell_ids(state: &AppState) -> Vec<TranscriptCellId> {
    let cells = state.session.transcript.cells();
    transcript_presentation(TranscriptPresentationInput {
        cells,
        options: TranscriptProjectionOptions {
            show_system_reminders: true,
            show_compact_internals: true,
        },
        streaming: state.ui.streaming.as_ref(),
        show_thinking: true,
        tool_executions: &state.session.tool_executions,
    })
    .cells
    .into_iter()
    .filter(|cell| cell.is_expandable(cells))
    .filter_map(|cell| cell.cell_id(cells))
    .collect()
}

pub(crate) fn latest_expandable_cell_id(state: &AppState) -> Option<TranscriptCellId> {
    transcript_expandable_cell_ids(state)
        .into_iter()
        .next_back()
}

/// Build a `TranscriptPresentation` from a caller-supplied cells slice
/// — the entry point for everything that wants to render the chat
/// transcript (typically the Ctrl+O modal).
pub(crate) fn transcript_presentation_with_cells<'state>(
    state: &'state AppState,
    cells: &[RenderedCell],
) -> TranscriptPresentation<'state> {
    transcript_presentation(TranscriptPresentationInput {
        cells,
        options: TranscriptProjectionOptions {
            show_system_reminders: true,
            show_compact_internals: true,
        },
        streaming: state.ui.streaming.as_ref(),
        show_thinking: true,
        tool_executions: &state.session.tool_executions,
    })
}

#[cfg(test)]
#[path = "transcript.test.rs"]
mod tests;
