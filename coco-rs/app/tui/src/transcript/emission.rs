//! Exactly-once finalized-history emission: the UUID prefix tracker and the
//! anchored finalize (tui-v2 §6.2).
//!
//! The tracker keys off engine-message UUIDs (the `message_uuid` on each
//! `RenderedCell`). A `Message::Assistant` that produces multiple cells
//! (text + thinking + tool_use) is represented by one entry — the tracker
//! collapses repeated UUIDs so the "previously emitted prefix" comparison
//! works at engine-message granularity rather than per-cell.
//!
//! [`finalize_after_stream_prefix`] is the pure half of the finalize:
//! given the streamed-prefix record it decides between appending the
//! committed render's suffix (anchor match) and full replay (`None`).
//! The per-frame state (the pending slot, terminal I/O, the tail cache)
//! stays in `surface::history_driver`.
use ratatui::text::Line;
use uuid::Uuid;

use crate::transcript::cells::CellKind;
use crate::transcript::cells::RenderedCell;
use crate::transcript::cells::engine_message_starts;
use crate::transcript::render::HistoryLineRenderOptions;
use crate::transcript::render::assistant::CommittedAssistantMarkdownOptions;
use crate::transcript::render::assistant::render_committed_assistant_markdown;
use crate::transcript::render::render_finalized_history_lines;
use crate::transcript::stream::ScrollbackStreamCommit;
use crate::transcript::stream::StreamRenderKey;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HistoryEmissionPlan {
    Noop,
    Append { start: usize },
    ReplayRequired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HistoryEmissionOutcome {
    Noop,
    FastNoop {
        revision: u64,
    },
    Appended {
        start: usize,
        message_count: usize,
        rows: u16,
    },
    Replayed {
        message_count: usize,
        rows: u16,
    },
    ReplayRequired,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct HistoryEmissionTracker {
    /// UUIDs of engine messages already emitted, in append order. One
    /// entry per distinct `message_uuid` regardless of how many cells
    /// that engine message produced.
    emitted_message_uuids: Vec<Uuid>,
}

impl HistoryEmissionTracker {
    pub(crate) fn emitted_count(&self) -> usize {
        self.emitted_message_uuids.len()
    }

    pub(crate) fn plan(&self, cells: &[RenderedCell]) -> HistoryEmissionPlan {
        let total_messages = engine_message_starts(cells).count();

        if self.emitted_message_uuids.is_empty() {
            return if cells.is_empty() {
                HistoryEmissionPlan::Noop
            } else {
                HistoryEmissionPlan::Append { start: 0 }
            };
        }

        if self.emitted_message_uuids.len() > total_messages {
            return HistoryEmissionPlan::ReplayRequired;
        }

        let mut emitted_index = 0usize;
        let mut next_start = None;
        let mut prev = None;
        let mut prefix_matches = true;
        for (cell_index, cell) in cells.iter().enumerate() {
            if Some(cell.message_uuid) == prev {
                continue;
            }
            prev = Some(cell.message_uuid);
            if let Some(emitted_uuid) = self.emitted_message_uuids.get(emitted_index) {
                if *emitted_uuid != cell.message_uuid {
                    prefix_matches = false;
                    break;
                }
                emitted_index += 1;
            } else {
                next_start = Some(cell_index);
                break;
            }
        }
        if !prefix_matches {
            return HistoryEmissionPlan::ReplayRequired;
        }

        if self.emitted_message_uuids.len() == total_messages {
            HistoryEmissionPlan::Noop
        } else {
            HistoryEmissionPlan::Append {
                start: next_start.unwrap_or(cells.len()),
            }
        }
    }

    pub(crate) fn mark_emitted_through(&mut self, cells: &[RenderedCell], end: usize) {
        let end = end.min(cells.len());
        let mut uuids: Vec<Uuid> = Vec::new();
        let mut prev = None;
        for cell in cells.iter().take(end) {
            if Some(cell.message_uuid) != prev {
                uuids.push(cell.message_uuid);
                prev = Some(cell.message_uuid);
            }
        }
        self.emitted_message_uuids = uuids;
    }

    #[cfg(test)]
    pub(crate) fn mark_appended_from(&mut self, cells: &[RenderedCell], start: usize) {
        let mut prev = cells
            .get(start.wrapping_sub(1))
            .map(|cell| cell.message_uuid);
        for cell in cells.iter().skip(start) {
            if Some(cell.message_uuid) != prev {
                self.emitted_message_uuids.push(cell.message_uuid);
                prev = Some(cell.message_uuid);
            }
        }
    }

    pub(crate) fn reset(&mut self) {
        self.emitted_message_uuids.clear();
    }
}

#[cfg(test)]
impl HistoryEmissionTracker {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn emit_append_only<B, F>(
        &mut self,
        terminal: &mut coco_tui_ui::engine::terminal::SurfaceTerminal<B>,
        cells: &[RenderedCell],
        render_tail: F,
    ) -> Result<HistoryEmissionOutcome, B::Error>
    where
        B: coco_tui_ui::engine::terminal::SurfaceBackend,
        F: FnOnce(&[RenderedCell]) -> Vec<ratatui::text::Line<'static>>,
    {
        let start = match self.plan(cells) {
            HistoryEmissionPlan::Noop => return Ok(HistoryEmissionOutcome::Noop),
            HistoryEmissionPlan::ReplayRequired => {
                return Ok(HistoryEmissionOutcome::ReplayRequired);
            }
            HistoryEmissionPlan::Append { start } => start,
        };

        let rendered = coco_tui_ui::engine::history_insert::render_history_rows(
            render_tail(&cells[start..]),
            terminal.viewport_area().width,
        );
        let rows = terminal.insert_history_rows(&rendered)?;
        self.mark_emitted_through(cells, cells.len());
        Ok(HistoryEmissionOutcome::Appended {
            start,
            message_count: cells.len() - start,
            rows,
        })
    }

    pub(crate) fn replay_all<B, F>(
        &mut self,
        terminal: &mut coco_tui_ui::engine::terminal::SurfaceTerminal<B>,
        cells: &[RenderedCell],
        render_all: F,
    ) -> Result<HistoryEmissionOutcome, B::Error>
    where
        B: coco_tui_ui::engine::terminal::SurfaceBackend,
        F: FnOnce(&[RenderedCell]) -> Vec<ratatui::text::Line<'static>>,
    {
        terminal.clear_owned_scrollback()?;
        let rendered = coco_tui_ui::engine::history_insert::render_history_rows(
            render_all(cells),
            terminal.viewport_area().width,
        );
        let rows = terminal.insert_history_rows(&rendered)?;
        self.mark_emitted_through(cells, cells.len());
        Ok(HistoryEmissionOutcome::Replayed {
            message_count: cells.len(),
            rows,
        })
    }
}

/// The anchored finalize (tui-v2 §6.2): decide between suffix-append and
/// replay for committable cells `[start..end)` when streamed rows for the
/// leading assistant text are already in native scrollback.
///
/// On anchor match (the canonical text starts with the streamed source prefix
/// AND the render key matches) it returns the committed render's remaining
/// rows — the suffix past `pending.line_prefix_len` — followed by the rest of
/// the group in canonical post-text order. `None` means anchor mismatch: the
/// caller must fall back to a full replay. No rasterized row comparison is
/// performed; the row-prefix equality this relies on is the markdown
/// prefix-stability property pinned by
/// `transcript::stream::tests::test_stable_lines_are_row_prefix_of_full_committed_render`.
pub(crate) fn finalize_after_stream_prefix(
    cells: &[RenderedCell],
    start: usize,
    end: usize,
    options: HistoryLineRenderOptions<'_>,
    commit: &ScrollbackStreamCommit,
) -> Option<Vec<Line<'static>>> {
    // Mirror `push_text_first_assistant_group`: a message's leading
    // thinking cells render AFTER its text under the native presentation,
    // so the streamed text rows already in scrollback are the leading rows
    // of the group. Skip the same-message leading thinking run to find the
    // text cell the pending prefix anchors to; the thinking cells join the
    // suffix below, after the remaining text rows.
    let first = cells.get(start)?;
    let group_uuid = first.message_uuid;
    let mut text_idx = start;
    while text_idx < end
        && cells[text_idx].message_uuid == group_uuid
        && matches!(
            cells[text_idx].kind,
            CellKind::AssistantThinking { .. } | CellKind::AssistantRedactedThinking
        )
    {
        text_idx += 1;
    }
    let Some(text_cell) = cells.get(text_idx) else {
        tracing::debug!(
            target: "tui::surface::replay",
            cause = "stream_commit_next_cell_not_assistant_text",
            start,
            text_idx,
            "history full replay required",
        );
        return None;
    };
    let CellKind::AssistantText { text, .. } = &text_cell.kind else {
        tracing::debug!(
            target: "tui::surface::replay",
            cause = "stream_commit_next_cell_not_assistant_text",
            start,
            text_idx,
            "history full replay required",
        );
        return None;
    };
    // A thinking run whose text belongs to a different message is a
    // thinking-only group — the presentation renders it unreordered, so
    // the streamed text rows cannot be the group's leading rows.
    if text_idx > start && text_cell.message_uuid != group_uuid {
        tracing::debug!(
            target: "tui::surface::replay",
            cause = "stream_commit_thinking_without_same_message_text",
            start,
            text_idx,
            "history full replay required",
        );
        return None;
    }
    if !text.starts_with(&commit.source_prefix) {
        tracing::debug!(
            target: "tui::surface::replay",
            cause = "stream_commit_source_mismatch",
            start,
            commit_source_prefix_len = commit.source_prefix.len(),
            text_len = text.len(),
            "history full replay required",
        );
        return None;
    }
    let render_key =
        StreamRenderKey::committed(options.styles, options.width, options.syntax_highlighting);
    if render_key != commit.render_key {
        tracing::debug!(
            target: "tui::surface::replay",
            cause = "stream_commit_render_key_mismatch",
            "history full replay required",
        );
        return None;
    }

    // Committed render of the full canonical text. The streamed rows
    // `[..line_prefix_len]` are already in scrollback; the source anchor plus
    // the render-key gate above prove the leading rows agree, so finalize
    // appends only the suffix `[line_prefix_len..]`.
    let assistant_lines = render_committed_assistant_markdown(
        text,
        CommittedAssistantMarkdownOptions {
            styles: options.styles,
            width: options.width,
            syntax_highlighting: options.syntax_highlighting,
        },
    );
    if commit.line_len > assistant_lines.len() {
        tracing::debug!(
            target: "tui::surface::replay",
            cause = "stream_commit_line_len_exceeds_final",
            commit_line_len = commit.line_len,
            final_lines = assistant_lines.len(),
            "history full replay required",
        );
        return None;
    }

    let mut lines = assistant_lines[commit.line_len..].to_vec();
    lines.push(Line::default());
    // Presentation order: the message's leading thinking cells render
    // after its text, then everything past the text cell (tool calls,
    // following messages) renders normally.
    if start < text_idx {
        lines.extend(render_finalized_history_lines(
            &cells[start..text_idx],
            options,
        ));
    }
    if text_idx + 1 < end {
        lines.extend(render_finalized_history_lines(
            &cells[text_idx + 1..end],
            options,
        ));
    }
    Some(lines)
}

#[cfg(test)]
#[path = "emission.test.rs"]
mod tests;
