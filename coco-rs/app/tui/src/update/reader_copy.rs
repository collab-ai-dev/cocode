//! Per-cell copy affordances for the Ctrl+O transcript reader (E3).
//!
//! `y` copies the selected cell's textual content; `Y` copies its
//! kind-specific metadata — a shell command, a file path, or a URL — extracted
//! from the tool-call accessors in [`crate::transcript::derive`]. Both route
//! through the shared clipboard stack (OSC 52 + temp-file fallback) and surface
//! a toast, mirroring `/copy`.

use crate::i18n::t;
use crate::state::AppState;
use crate::state::ModalState;
use crate::state::transcript::TranscriptCellId;
use crate::state::ui::Toast;
use crate::transcript::cells::CellKind;
use crate::transcript::cells::RenderedCell;
use crate::transcript::derive;
use coco_tui_ui::clipboard_copy;
use coco_types::ToolName;

/// `y` — copy the selected cell's textual content.
pub(super) fn copy_selected_cell_text(state: &mut AppState) -> bool {
    copy_selected_cell_text_with(state, clipboard_copy::copy_to_clipboard)
}

pub(super) fn copy_selected_cell_text_with(
    state: &mut AppState,
    copy_fn: impl FnOnce(&str) -> Result<Option<clipboard_copy::ClipboardLease>, String>,
) -> bool {
    if let Some(text) = selected_cell_text(state) {
        finish_copy(state, &text, None, copy_fn);
    } else {
        state
            .ui
            .add_toast(Toast::info(t!("toast.reader_copy_empty").to_string()));
    }
    true
}

/// `Y` — copy the selected cell's kind-specific metadata, falling back to the
/// cell text for cells without a distinct identifier.
pub(super) fn copy_selected_cell_meta(state: &mut AppState) -> bool {
    copy_selected_cell_meta_with(state, clipboard_copy::copy_to_clipboard)
}

pub(super) fn copy_selected_cell_meta_with(
    state: &mut AppState,
    copy_fn: impl FnOnce(&str) -> Result<Option<clipboard_copy::ClipboardLease>, String>,
) -> bool {
    if let Some((field, value)) = selected_cell_meta(state) {
        finish_copy(state, &value, Some(field), copy_fn);
        true
    } else {
        copy_selected_cell_text_with(state, copy_fn)
    }
}

fn selected_cell_text(state: &AppState) -> Option<String> {
    let cells = state.session.transcript.cells();
    let Some(ModalState::Transcript(t)) = state.ui.modal.as_ref() else {
        return None;
    };
    match t.selected_cell_id.as_ref()? {
        TranscriptCellId::ToolBatch { start, end } => batch_cell_text(cells, *start, *end),
        _ => cell_text(cells, resolve_selected_cell(state, cells)?),
    }
}

fn selected_cell_meta(state: &AppState) -> Option<(&'static str, String)> {
    let cells = state.session.transcript.cells();
    let Some(ModalState::Transcript(t)) = state.ui.modal.as_ref() else {
        return None;
    };
    match t.selected_cell_id.as_ref()? {
        TranscriptCellId::ToolBatch { start, end } => {
            batch_cell_meta(cells, *start, *end).map(|value| ("batch metadata", value))
        }
        _ => cell_meta(resolve_selected_cell(state, cells)?),
    }
}

fn resolve_selected_cell<'a>(
    state: &AppState,
    cells: &'a [RenderedCell],
) -> Option<&'a RenderedCell> {
    let Some(ModalState::Transcript(t)) = state.ui.modal.as_ref() else {
        return None;
    };
    match t.selected_cell_id.as_ref()? {
        // Prefer the tool-use cell; fall back to an orphan result (a result with
        // no surviving invocation, e.g. after compaction / rewind / resume).
        TranscriptCellId::ToolCall { call_id } => cells
            .iter()
            .find(|c| matches!(&c.kind, CellKind::ToolUse { call_id: cid, .. } if cid == call_id))
            .or_else(|| {
                cells.iter().find(
                    |c| matches!(&c.kind, CellKind::ToolResult { call_id: cid } if cid == call_id),
                )
            }),
        TranscriptCellId::Message { index, .. } => cells.get(*index),
        TranscriptCellId::ToolBatch { .. } => None,
        TranscriptCellId::ActiveTail => None,
    }
}

fn batch_cell_text(cells: &[RenderedCell], start: usize, end: usize) -> Option<String> {
    let parts: Vec<String> = cells
        .get(start..end.min(cells.len()))?
        .iter()
        .filter(|cell| matches!(cell.kind, CellKind::ToolUse { .. }))
        .filter_map(|cell| cell_text(cells, cell))
        .collect();
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

fn batch_cell_meta(cells: &[RenderedCell], start: usize, end: usize) -> Option<String> {
    let parts: Vec<String> = cells
        .get(start..end.min(cells.len()))?
        .iter()
        .filter_map(cell_meta)
        .map(|(field, value)| format!("{field}: {value}"))
        .collect();
    (!parts.is_empty()).then(|| parts.join("\n"))
}

fn cell_text(cells: &[RenderedCell], cell: &RenderedCell) -> Option<String> {
    let text = match &cell.kind {
        // Prose / thinking cells carry their text directly on the cell kind
        // (thinking text was otherwise dropped by `message_plain_text`).
        CellKind::UserText { text }
        | CellKind::AssistantText { text, .. }
        | CellKind::AssistantThinking { text, .. } => text.clone(),
        // A tool cell's substantive content is its result output; fall back to
        // the one-line invocation preview when no result has arrived yet.
        CellKind::ToolUse { call_id, tool_name } => paired_result_output(cells, call_id)
            .unwrap_or_else(|| {
                derive::tool_call_header_preview(cell.source.as_ref(), call_id, tool_name)
            }),
        CellKind::ToolResult { .. } => derive::tool_result_output(cell.source.as_ref())
            .map(|p| p.output)
            .unwrap_or_default(),
        _ => derive::message_plain_text(cell.source.as_ref()).unwrap_or_default(),
    };
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Output of the tool result paired with `call_id`, if any (non-empty).
fn paired_result_output(cells: &[RenderedCell], call_id: &str) -> Option<String> {
    let result = cells
        .iter()
        .find(|c| matches!(&c.kind, CellKind::ToolResult { call_id: cid } if cid == call_id))?;
    let output = derive::tool_result_output(result.source.as_ref())?.output;
    (!output.trim().is_empty()).then_some(output)
}

fn cell_meta(cell: &RenderedCell) -> Option<(&'static str, String)> {
    let CellKind::ToolUse { call_id, tool_name } = &cell.kind else {
        return None;
    };
    let (key, field) = meta_field(tool_name)?;
    let input = derive::extract_tool_call_input(cell.source.as_ref(), call_id)?;
    let value = input.get(key).and_then(|v| v.as_str())?;
    (!value.is_empty()).then(|| (field, value.to_string()))
}

/// `(JSON input key, toast field label)` for a tool's primary argument, or
/// `None` for tools without a single meaningful identifier (MCP / custom).
fn meta_field(tool_name: &str) -> Option<(&'static str, &'static str)> {
    if [ToolName::Bash, ToolName::PowerShell, ToolName::Repl]
        .iter()
        .any(|t| t.as_str() == tool_name)
    {
        Some(("command", "command"))
    } else if [ToolName::Read, ToolName::Edit, ToolName::Write]
        .iter()
        .any(|t| t.as_str() == tool_name)
    {
        Some(("file_path", "path"))
    } else if ToolName::NotebookEdit.as_str() == tool_name {
        // NotebookEdit's input field is `notebook_path`, not `file_path`.
        Some(("notebook_path", "path"))
    } else if ToolName::WebFetch.as_str() == tool_name {
        Some(("url", "url"))
    } else if ToolName::WebSearch.as_str() == tool_name {
        Some(("query", "query"))
    } else if [ToolName::Grep, ToolName::Glob]
        .iter()
        .any(|t| t.as_str() == tool_name)
    {
        Some(("pattern", "pattern"))
    } else {
        None
    }
}

/// Copy `text` and surface a toast. `field` (Some for a metadata copy) labels
/// what was copied; None reports a character count like `/copy`.
fn finish_copy(
    state: &mut AppState,
    text: &str,
    field: Option<&str>,
    copy_fn: impl FnOnce(&str) -> Result<Option<clipboard_copy::ClipboardLease>, String>,
) {
    match copy_fn(text) {
        Ok(lease) => {
            let durability = if lease.is_some() {
                t!("toast.copy_durability_until_exit")
            } else {
                t!("toast.copy_durability_persistent")
            };
            state.ui.clipboard_lease = lease;
            let msg = match field {
                Some(field) => t!("toast.reader_copied_field", field = field).to_string(),
                None => t!(
                    "toast.copied_chars",
                    count = text.chars().count(),
                    durability = durability
                )
                .to_string(),
            };
            state.ui.add_toast(Toast::success(msg));
        }
        Err(err) => {
            state.ui.add_toast(Toast::error(
                t!("toast.copy_failed_short", error = err).to_string(),
            ));
        }
    }
}

#[cfg(test)]
#[path = "reader_copy.test.rs"]
mod tests;
