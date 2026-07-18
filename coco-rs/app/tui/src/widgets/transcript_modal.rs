use std::collections::HashMap;
use std::collections::HashSet;

use coco_keybindings::KeybindingAction;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Block;
use ratatui::widgets::Borders;
use ratatui::widgets::Widget;

use crate::i18n::t;
use crate::keybinding_bridge::KeybindingContext as TuiContext;
use crate::presentation::thinking::format_duration_seconds;
use crate::presentation::transcript::TranscriptSourceCell;
use crate::presentation::transcript::transcript_presentation_with_cells;
use crate::state::AppState;
use crate::state::session::ToolStatus;
use crate::state::transcript::TranscriptCellId;
use crate::state::transcript::TranscriptScrollPosition;
use crate::state::transcript::TranscriptState;
use crate::transcript::cells::CellKind;
use crate::transcript::cells::RenderedCell;
use crate::transcript::derive::tool_result_output;
use crate::transcript::render::reader::TranscriptCellRenderer;
use crate::transcript::render::reader::meta_preview_text;
use coco_tui_ui::style::UiStyles;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TranscriptHeightCacheKey {
    cell_id: TranscriptCellId,
    width: u16,
    expanded: bool,
    /// Execution status of the tool this cell renders, when it renders one.
    ///
    /// A settled cell is a pure, immutable derivation of its message (I-2), so
    /// its own content cannot change under a stable id. A `ToolUse` cell's
    /// rendered height additionally depends on live `ToolExecution` state,
    /// which is UI-only (I-3) and NOT part of the cell — so it belongs in the
    /// key. Keying on it here is what lets the map survive a generation bump
    /// instead of being flushed wholesale. Only the elapsed badge's width is
    /// relevant to height, so ticking within the same width reuses the entry.
    tool_layout: Option<(ToolStatus, usize)>,
    /// Thinking header metadata lives in a UI side-cache rather than the
    /// immutable transcript cell, so it must participate in height identity.
    reasoning_metadata: Option<(Option<i64>, i64)>,
}

/// Floor for the [`TranscriptLayoutIndex::heights`] retention bound, so short
/// transcripts never trip the prune.
const MIN_RETAINED_HEIGHTS: usize = 256;

#[derive(Debug, Clone, Default)]
pub(crate) struct TranscriptLayoutIndex {
    content_generation: Option<u64>,
    prefix_generation: Option<u64>,
    heights: HashMap<TranscriptHeightCacheKey, usize>,
    prefix: Vec<Option<usize>>,
}

impl TranscriptLayoutIndex {
    pub(crate) fn reset(&mut self) {
        self.content_generation = None;
        self.prefix_generation = None;
        self.heights.clear();
        self.prefix.clear();
    }

    fn begin_frame(&mut self, content_generation: u64, prefix_generation: u64, cell_count: usize) {
        if self.content_generation != Some(content_generation) {
            self.content_generation = Some(content_generation);
            // `heights` is deliberately RETAINED across the bump.
            //
            // The generation hash moves on any transcript or tool-status
            // change, and flushing the whole map for it meant that with the
            // overlay open during a turn, `total_height()` re-rendered every
            // cell in the history on every change — O(history) full cell
            // renders per delta.
            //
            // Nothing that a bump signals can invalidate a *keyed* height:
            // cells are append-only-with-truncation pure derivations, live
            // cells bypass this cache entirely (see `height`), a width change
            // is already part of the key, and tool status is now part of it
            // too. Entries whose cells a truncation removed are unreachable
            // (their ids are gone) rather than wrong, and the prune below
            // bounds them.
            if self.heights.len() > cell_count.saturating_mul(4).max(MIN_RETAINED_HEIGHTS) {
                self.heights.clear();
            }
        }
        if self.prefix_generation != Some(prefix_generation) || self.prefix.len() != cell_count + 1
        {
            self.prefix_generation = Some(prefix_generation);
            self.prefix.clear();
            self.prefix.resize(cell_count + 1, None);
        }
        self.prefix[0] = Some(0);
    }

    fn invalidate_prefix_from(&mut self, index: usize) {
        for prefix in self.prefix.iter_mut().skip(index.saturating_add(1)) {
            *prefix = None;
        }
    }
}

pub(crate) struct TranscriptStateWidget<'a> {
    state: &'a AppState,
    transcript: &'a TranscriptState,
    layout_index: &'a mut TranscriptLayoutIndex,
    styles: UiStyles<'a>,
}

impl<'a> TranscriptStateWidget<'a> {
    pub(crate) fn new(
        state: &'a AppState,
        transcript: &'a TranscriptState,
        layout_index: &'a mut TranscriptLayoutIndex,
        styles: UiStyles<'a>,
    ) -> Self {
        Self {
            state,
            transcript,
            layout_index,
            styles,
        }
    }
}

impl Widget for TranscriptStateWidget<'_> {
    fn render(mut self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }

        let block = Block::default()
            .borders(Borders::ALL)
            .title(t!("transcript.title").to_string())
            .border_style(Style::default().fg(self.styles.modal_border()));
        let inner = block.inner(area);
        block.render(area, buf);
        if inner.is_empty() {
            return;
        }

        let footer_height = if inner.height > 2 {
            2
        } else {
            u16::from(inner.height > 1)
        };
        let content_area = Rect {
            height: inner.height.saturating_sub(footer_height),
            ..inner
        };
        let footer_area = Rect {
            y: inner.bottom().saturating_sub(footer_height),
            height: footer_height,
            ..inner
        };

        if content_area.height > 0 {
            self.render_cells(content_area, buf);
        }
        if footer_area.height > 0 {
            self.render_footer(footer_area, buf);
        }
    }
}

impl TranscriptStateWidget<'_> {
    fn render_cells(&mut self, area: Rect, buf: &mut Buffer) {
        // Engine-authoritative cells are the single source of truth.
        // `session.transcript.cells()` is the same slice the chat
        // widget renders from, so the modal preserves visual parity
        // with the inline chat.
        let cells = self.state.session.transcript.cells();
        let presentation = transcript_presentation_with_cells(self.state, cells);
        self.layout_index.begin_frame(
            transcript_layout_generation(self.state, cells),
            transcript_prefix_generation(
                self.state,
                cells,
                &presentation.cells,
                area.width,
                &self.transcript.collapsed_cell_ids,
            ),
            presentation.cells.len(),
        );
        if presentation.cells.is_empty() {
            Line::from(
                Span::raw(t!("transcript.empty").to_string()).style(self.styles.dim_style()),
            )
            .render(Rect { height: 1, ..area }, buf);
            return;
        }

        let mut renderer = TranscriptCellRenderer::new(
            cells,
            self.state,
            &self.transcript.search,
            self.styles,
            area.width,
        );
        let visible = {
            let mut pager = TranscriptPager::new(
                &presentation.cells,
                cells,
                &mut renderer,
                &self.transcript.collapsed_cell_ids,
                self.transcript.selected_cell_id.as_ref(),
                self.layout_index,
            );
            let scroll =
                effective_scroll(&self.transcript.scroll, &mut pager, area.height as usize);
            let visible = pager.visible_cells(scroll, area.height as usize);
            visible.cells
        };

        let mut y = area.y;
        for cell in visible {
            if y >= area.bottom() {
                break;
            }
            let cell_area = Rect {
                x: area.x,
                y,
                width: area.width,
                height: (area.bottom() - y).min(cell.height.saturating_sub(cell.skip) as u16),
            };
            if cell_area.height == 0 {
                continue;
            }
            let source = &presentation.cells[cell.index];
            let id = source.cell_id(cells);
            let expanded = id
                .as_ref()
                .is_none_or(|id| !self.transcript.collapsed_cell_ids.contains(id));
            let selected = id.as_ref() == self.transcript.selected_cell_id.as_ref();
            renderer.render_window(source, cell_area, cell.skip, expanded, selected, buf);
            y = y.saturating_add(cell_area.height);
        }
    }

    fn render_footer(&self, area: Rect, buf: &mut Buffer) {
        let toggle_chord = self
            .state
            .ui
            .kb_handle
            .display_for(&KeybindingAction::AppToggleTranscript, TuiContext::Chat)
            .unwrap_or_else(|| "ctrl+o".to_string());
        let nav = t!("transcript.hint_footer_nav", toggle = toggle_chord.as_str()).to_string();
        Line::from(Span::raw(nav).style(self.styles.dim_style()))
            .render(Rect { height: 1, ..area }, buf);
        if area.height > 1 {
            let actions =
                if self.transcript.search.editing || !self.transcript.search.query.is_empty() {
                    let (current, total) = self.transcript.search.status();
                    let prompt = t!(
                        "transcript.search_prompt",
                        query = self.transcript.search.query.as_str()
                    );
                    let status = if self.transcript.search.editing {
                        t!(
                            "transcript.search_status_editing",
                            current = current,
                            total = total
                        )
                    } else {
                        t!(
                            "transcript.search_status_idle",
                            current = current,
                            total = total
                        )
                    };
                    format!("{prompt}  {status}")
                } else {
                    t!("transcript.hint_footer_actions").to_string()
                };
            Line::from(Span::raw(actions).style(self.styles.dim_style())).render(
                Rect {
                    y: area.y.saturating_add(1),
                    height: 1,
                    ..area
                },
                buf,
            );
        }
    }
}

struct TranscriptPager<'cells, 'state, 'r> {
    cells: &'cells [TranscriptSourceCell<'state>],
    rendered_cells: &'state [RenderedCell],
    renderer: &'r mut TranscriptCellRenderer<'state>,
    collapsed_cell_ids: &'cells HashSet<TranscriptCellId>,
    layout_index: &'r mut TranscriptLayoutIndex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VisibleCell {
    index: usize,
    skip: usize,
    height: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VisibleCells {
    cells: Vec<VisibleCell>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VisibleScan {
    cells: Vec<VisibleCell>,
    reached_end: bool,
    total_height: usize,
}

impl<'cells, 'state, 'r> TranscriptPager<'cells, 'state, 'r> {
    fn new(
        cells: &'cells [TranscriptSourceCell<'state>],
        rendered_cells: &'state [RenderedCell],
        renderer: &'r mut TranscriptCellRenderer<'state>,
        collapsed_cell_ids: &'cells HashSet<TranscriptCellId>,
        _selected_cell_id: Option<&'cells TranscriptCellId>,
        layout_index: &'r mut TranscriptLayoutIndex,
    ) -> Self {
        Self {
            cells,
            rendered_cells,
            renderer,
            collapsed_cell_ids,
            layout_index,
        }
    }

    fn visible_cells(&mut self, scroll: usize, viewport_height: usize) -> VisibleCells {
        let scan = self.scan_visible_cells(scroll, viewport_height);
        if scan.reached_end && scroll > scan.total_height.saturating_sub(viewport_height) {
            let max_scroll = scan.total_height.saturating_sub(viewport_height);
            let clamped = self.scan_visible_cells(max_scroll, viewport_height);
            return VisibleCells {
                cells: clamped.cells,
            };
        }

        VisibleCells { cells: scan.cells }
    }

    fn scan_visible_cells(&mut self, scroll: usize, viewport_height: usize) -> VisibleScan {
        let end = scroll.saturating_add(viewport_height).saturating_add(2);
        let (mut index, mut top) = self.first_visible_index(scroll);
        let mut visible = Vec::new();
        while index < self.cells.len() {
            if top >= end {
                return VisibleScan {
                    cells: visible,
                    reached_end: false,
                    total_height: top,
                };
            }
            let height = self.height(index);
            let bottom = top.saturating_add(height);
            if bottom > scroll && top < end {
                visible.push(VisibleCell {
                    index,
                    skip: scroll.saturating_sub(top),
                    height,
                });
            }
            top = bottom;
            self.set_prefix(index.saturating_add(1), top);
            index = index.saturating_add(1);
        }
        VisibleScan {
            cells: visible,
            reached_end: true,
            total_height: top,
        }
    }

    fn cell_top(&mut self, cell_id: &TranscriptCellId) -> Option<usize> {
        for index in 0..self.cells.len() {
            if self.cells[index]
                .cell_id(self.rendered_cells)
                .as_ref()
                .is_some_and(|id| id == cell_id)
            {
                return Some(self.prefix_top(index));
            }
        }
        None
    }

    fn total_height(&mut self) -> usize {
        self.prefix_top(self.cells.len())
    }

    fn first_visible_index(&mut self, scroll: usize) -> (usize, usize) {
        let mut index = 0usize;
        let mut top = 0usize;
        while index < self.cells.len() {
            let height = self.height(index);
            let bottom = top.saturating_add(height);
            self.set_prefix(index.saturating_add(1), bottom);
            if bottom > scroll {
                return (index, top);
            }
            index = index.saturating_add(1);
            top = bottom;
        }
        (self.cells.len(), top)
    }

    fn prefix_top(&mut self, index: usize) -> usize {
        let index = index.min(self.cells.len());
        if let Some(top) = self
            .layout_index
            .prefix
            .get(index)
            .and_then(|prefix| *prefix)
        {
            return top;
        }

        let mut start = index;
        while start > 0
            && self
                .layout_index
                .prefix
                .get(start)
                .is_none_or(Option::is_none)
        {
            start -= 1;
        }

        let mut top = self
            .layout_index
            .prefix
            .get(start)
            .and_then(|prefix| *prefix)
            .unwrap_or(0);
        for current in start..index {
            top = top.saturating_add(self.height(current));
            self.set_prefix(current.saturating_add(1), top);
        }
        top
    }

    fn set_prefix(&mut self, index: usize, top: usize) {
        if let Some(prefix) = self.layout_index.prefix.get_mut(index) {
            *prefix = Some(top);
        }
    }

    fn height(&mut self, index: usize) -> usize {
        let cell = &self.cells[index];
        let id = cell.cell_id(self.rendered_cells);
        let expanded = id
            .as_ref()
            .is_none_or(|id| !self.collapsed_cell_ids.contains(id));
        if matches!(cell, TranscriptSourceCell::Active(_)) {
            return self.renderer.desired_height(cell, expanded, false).max(1);
        }
        let Some(id) = id else {
            return self.renderer.desired_height(cell, expanded, false).max(1);
        };
        let key = TranscriptHeightCacheKey {
            tool_layout: self.renderer.tool_layout_for(&id),
            reasoning_metadata: self.renderer.reasoning_layout_for(cell),
            cell_id: id,
            width: self.renderer.width,
            expanded,
        };
        if let Some(height) = self.layout_index.heights.get(&key).copied() {
            return height;
        }
        let height = self.renderer.desired_height(cell, expanded, false).max(1);
        self.layout_index.heights.insert(key, height);
        self.layout_index.invalidate_prefix_from(index);
        height
    }
}

fn signed_offset(base: usize, offset: i32) -> usize {
    if offset < 0 {
        base.saturating_sub(offset.unsigned_abs() as usize)
    } else {
        base.saturating_add(offset as usize)
    }
}

fn effective_scroll(
    scroll: &TranscriptScrollPosition,
    pager: &mut TranscriptPager<'_, '_, '_>,
    viewport_height: usize,
) -> usize {
    match scroll {
        TranscriptScrollPosition::Top => 0,
        TranscriptScrollPosition::Absolute(top) => *top,
        TranscriptScrollPosition::Anchor {
            cell_id,
            offset_rows,
        } => pager
            .cell_top(cell_id)
            .map(|top| signed_offset(top, *offset_rows))
            .unwrap_or(0),
        TranscriptScrollPosition::Tail { offset_from_bottom } => pager
            .total_height()
            .saturating_sub(viewport_height)
            .saturating_sub(*offset_from_bottom),
    }
}

fn transcript_layout_generation(state: &AppState, cells: &[RenderedCell]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    hash = mix_u64(hash, cells.len() as u64);
    if let Some(last) = cells.last() {
        hash = mix_str(hash, &last.message_uuid.to_string());
        hash = mix_u64(hash, cell_content_len(last) as u64);
    }
    hash = mix_u64(hash, state.session.tool_executions.len() as u64);
    for tool in &state.session.tool_executions {
        hash = mix_str(hash, &tool.call_id);
        hash = mix_u64(hash, tool.status as u64);
        hash = mix_u64(
            hash,
            format_duration_seconds(tool.elapsed()).chars().count() as u64,
        );
    }
    for cell in cells {
        let metadata_anchor = match &cell.kind {
            CellKind::AssistantThinking {
                metadata_anchor, ..
            }
            | CellKind::AssistantRedactedThinking { metadata_anchor } => *metadata_anchor,
            _ => false,
        };
        if metadata_anchor {
            hash = mix_str(hash, &cell.message_uuid.to_string());
            if let Some(meta) = state.session.reasoning_metadata.get(&cell.message_uuid) {
                hash = mix_u64(hash, meta.duration_ms.unwrap_or(-1) as u64);
                hash = mix_u64(hash, meta.reasoning_tokens as u64);
            }
        }
    }
    hash
}

fn transcript_prefix_generation(
    state: &AppState,
    cells: &[RenderedCell],
    presentation_cells: &[TranscriptSourceCell<'_>],
    width: u16,
    collapsed_cell_ids: &HashSet<TranscriptCellId>,
) -> u64 {
    let mut hash = transcript_layout_generation(state, cells);
    hash = mix_u64(hash, u64::from(width));
    for cell in presentation_cells {
        let Some(id) = cell.cell_id(cells) else {
            continue;
        };
        if collapsed_cell_ids.contains(&id) {
            hash = mix_u64(hash, 1);
            hash = mix_cell_id(hash, &id);
        }
    }
    hash
}

fn mix_cell_id(mut hash: u64, cell_id: &TranscriptCellId) -> u64 {
    match cell_id {
        TranscriptCellId::ToolCall { call_id } => {
            hash = mix_u64(hash, 1);
            mix_str(hash, call_id)
        }
        TranscriptCellId::Message { index, message_id } => {
            hash = mix_u64(hash, 2);
            hash = mix_u64(hash, *index as u64);
            mix_str(hash, message_id)
        }
        TranscriptCellId::ToolBatch { start, end } => {
            hash = mix_u64(hash, 3);
            hash = mix_u64(hash, *start as u64);
            mix_u64(hash, *end as u64)
        }
        TranscriptCellId::ActiveTail => mix_u64(hash, 4),
    }
}

/// Best-effort byte length of the rendered text inside a cell — used by
/// the layout-invalidation hash. Mirrors the chat-widget's choice of
/// summarising tool calls by name + preview rather than by output, so
/// the modal redraws on the same boundaries as the inline chat.
fn cell_content_len(cell: &RenderedCell) -> usize {
    match &cell.kind {
        CellKind::UserText { text }
        | CellKind::AssistantText { text, .. }
        | CellKind::AssistantThinking { text, .. } => text.len(),
        CellKind::ToolUse {
            call_id, tool_name, ..
        } => {
            // Same header preview the renderer draws, so the invalidation hash
            // tracks exactly what's painted.
            let preview = crate::transcript::derive::tool_call_header_preview(
                &cell.source,
                call_id,
                tool_name,
            );
            tool_name.len() + call_id.len() + preview.len()
        }
        CellKind::ToolResult { call_id, .. } => {
            let len = tool_result_output(&cell.source)
                .map(|projection| projection.tool_name.len() + projection.output.len())
                .unwrap_or(0);
            call_id.len() + len
        }
        CellKind::System(_) => meta_preview_text(cell).len(),
        CellKind::Attachment | CellKind::AssistantRedactedThinking { .. } => 0,
    }
}

fn mix_str(mut hash: u64, value: &str) -> u64 {
    for byte in value.bytes() {
        hash = mix_u64(hash, u64::from(byte));
    }
    hash
}

fn mix_u64(hash: u64, value: u64) -> u64 {
    hash.wrapping_mul(0x0000_0100_0000_01b3) ^ value
}

#[cfg(test)]
#[path = "transcript_modal.test.rs"]
mod tests;
