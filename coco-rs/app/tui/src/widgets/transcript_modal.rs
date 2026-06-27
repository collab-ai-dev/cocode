use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

use coco_keybindings::KeybindingAction;
use coco_messages::Message;
use coco_messages::SystemMessage;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::Stylize;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Block;
use ratatui::widgets::Borders;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use ratatui::widgets::Wrap;

use crate::i18n::t;
use crate::keybinding_bridge::KeybindingContext as TuiContext;
use crate::presentation::layout::text_width;
use crate::presentation::thinking::ThinkingDisplay;
use crate::presentation::thinking::ThinkingRenderInput;
use crate::presentation::thinking::format_duration_seconds;
use crate::presentation::thinking::render_thinking_block;
use crate::presentation::transcript::TRANSCRIPT_COLLAPSED_PREVIEW_LINES;
use crate::presentation::transcript::TRANSCRIPT_EXPANDED_CELL_LINE_CAP;
use crate::presentation::transcript::TRANSCRIPT_LINE_CHAR_CAP;
use crate::presentation::transcript::TRANSCRIPT_TRUNCATED_HINT;
use crate::presentation::transcript::ToolOutputPreview;
use crate::presentation::transcript::TranscriptCell;
use crate::presentation::transcript::TranscriptSourceCell;
use crate::presentation::transcript::tool_output_preview;
use crate::presentation::transcript::transcript_presentation_with_cells;
use crate::state::AppState;
use crate::state::session::ToolExecution;
use crate::state::transcript::TranscriptCellId;
use crate::state::transcript::TranscriptScrollPosition;
use crate::state::transcript::TranscriptState;
use crate::tool_display::ToolNameTone;
use crate::tool_display::tool_name_tone;
use crate::transcript::cells::CellKind;
use crate::transcript::cells::RenderedCell;
use crate::transcript::cells::SystemCellKind;
use crate::transcript::derive::extract_tool_call_input;
use crate::transcript::derive::tool_result_output;
use crate::transcript::render::tool_result::ToolResultRenderCtx;
use coco_tui_ui::display::SyntaxHighlighting;
use coco_tui_ui::style::UiStyles;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TranscriptHeightCacheKey {
    cell_id: TranscriptCellId,
    width: u16,
    expanded: bool,
}

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
            self.reset();
            self.content_generation = Some(content_generation);
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
            Line::from(Span::raw(t!("transcript.empty").to_string()).fg(self.styles.dim()))
                .render(Rect { height: 1, ..area }, buf);
            return;
        }

        let mut renderer = TranscriptCellRenderer::new(cells, self.state, self.styles, area.width);
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
        Line::from(Span::raw(nav).fg(self.styles.dim())).render(Rect { height: 1, ..area }, buf);
        if area.height > 1 {
            let actions = t!("transcript.hint_footer_actions").to_string();
            Line::from(Span::raw(actions).fg(self.styles.dim())).render(
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

struct TranscriptCellRenderer<'a> {
    cells: &'a [RenderedCell],
    tool_executions: &'a [ToolExecution],
    reasoning_metadata: &'a HashMap<uuid::Uuid, crate::state::session::ReasoningMetadata>,
    compact_boundary_shortcut: String,
    thinking_toggle_hint: String,
    plan_editor_hint: String,
    width: u16,
    styles: UiStyles<'a>,
    syntax_highlighting: SyntaxHighlighting,
    cwd: Option<&'a str>,
}

impl<'a> TranscriptCellRenderer<'a> {
    fn new(
        cells: &'a [RenderedCell],
        state: &'a AppState,
        styles: UiStyles<'a>,
        width: u16,
    ) -> Self {
        Self {
            cells,
            tool_executions: &state.session.tool_executions,
            reasoning_metadata: &state.session.reasoning_metadata,
            compact_boundary_shortcut: compact_boundary_shortcut(state),
            thinking_toggle_hint: thinking_toggle_hint(state),
            plan_editor_hint: plan_editor_hint(state),
            width,
            styles,
            syntax_highlighting: state.ui.display_settings.syntax_highlighting,
            cwd: state.session.working_dir.as_deref(),
        }
    }

    /// Surface context for the shared per-tool result renderer. The reader IS the
    /// full-detail view, so `expanded` relaxes the inline row caps and no further
    /// "ctrl+o to expand" hint is appended.
    fn tool_result_ctx(&self, expanded: bool) -> ToolResultRenderCtx<'_> {
        ToolResultRenderCtx {
            styles: self.styles,
            width: self.width,
            syntax_highlighting: self.syntax_highlighting,
            plan_editor_hint: self.plan_editor_hint.clone(),
            expand_hint: String::new(),
            expanded,
        }
    }

    fn desired_height(
        &self,
        cell: &TranscriptSourceCell<'a>,
        expanded: bool,
        selected: bool,
    ) -> usize {
        let lines = self.render_cell(cell, expanded, selected);
        wrapped_height(&lines, self.width)
    }

    fn render_window(
        &self,
        cell: &TranscriptSourceCell<'a>,
        area: Rect,
        skip_lines: usize,
        expanded: bool,
        selected: bool,
        buf: &mut Buffer,
    ) {
        let lines = self.render_cell(cell, expanded, selected);
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((skip_lines.min(u16::MAX as usize) as u16, 0))
            .render(area, buf);
    }

    fn render_cell(
        &self,
        cell: &TranscriptSourceCell<'a>,
        expanded: bool,
        selected: bool,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        match cell {
            TranscriptSourceCell::Committed(TranscriptCell::MetaPreview { index }) => {
                if let Some(c) = self.cells.get(*index) {
                    self.render_meta_preview(c, &mut lines);
                }
            }
            TranscriptSourceCell::Committed(TranscriptCell::Cell { index }) => {
                if let Some(c) = self.cells.get(*index) {
                    self.render_cell_content(c, expanded, &mut lines);
                    lines.push(Line::default());
                }
            }
            TranscriptSourceCell::Committed(TranscriptCell::ToolCall {
                invocation,
                result,
                ..
            }) => {
                self.render_tool_call(*invocation, *result, expanded, &mut lines);
                lines.push(Line::default());
            }
            TranscriptSourceCell::Committed(TranscriptCell::ToolBatch { start, end, count }) => {
                let mut text = format!("  ‖ {}", t!("chat.tools_in_parallel", count = count));
                let names = crate::presentation::transcript::tool_batch_name_summary(
                    self.cells, *start, *end,
                );
                if !names.is_empty() {
                    text.push_str(" · ");
                    text.push_str(&names);
                }
                lines.push(Line::from(Span::raw(text).fg(self.styles.secondary())));
                lines.push(Line::default());
            }
            TranscriptSourceCell::Active(active) => match active {
                crate::presentation::transcript::ActiveTranscriptCell::Streaming(view) => {
                    if let Some(text) = view.assistant_text {
                        self.render_text_block("⏺", text, &mut lines);
                        lines.push(Line::from(Span::raw("▌").fg(self.styles.accent())));
                    }
                    if let Some(count) = view.thinking_tokens {
                        lines.extend(render_thinking_block(
                            ThinkingRenderInput {
                                content: "",
                                duration_ms: None,
                                reasoning_tokens: Some(count),
                                toggle_hint: Some(&self.thinking_toggle_hint),
                                display: ThinkingDisplay::Collapsed,
                            },
                            self.styles,
                        ));
                    }
                }
                crate::presentation::transcript::ActiveTranscriptCell::InFlightTools => {
                    lines.extend(crate::transcript::render::in_flight_tool_lines(
                        self.tool_executions,
                        self.styles,
                    ));
                }
            },
        }

        if selected {
            if let Some(first) = lines.first_mut() {
                first
                    .spans
                    .insert(0, Span::raw("▶ ").fg(self.styles.primary()));
            } else {
                lines.push(Line::from(Span::raw("▶").fg(self.styles.primary())));
            }
        }
        lines
    }

    fn render_tool_call(
        &self,
        invocation: Option<usize>,
        result: Option<usize>,
        expanded: bool,
        lines: &mut Vec<Line<'static>>,
    ) {
        let invocation_cell = invocation.and_then(|index| self.cells.get(index));
        let result_cell = result.and_then(|index| self.cells.get(index));

        // Issuing call's arguments, when the invocation cell is on hand — drives
        // the rich input-derived views (diffs, code, web target) in the reader.
        let input = invocation_cell.and_then(|cell| match &cell.kind {
            CellKind::ToolUse { call_id, .. } => extract_tool_call_input(&cell.source, call_id),
            _ => None,
        });

        if expanded {
            if let Some(cell) = invocation_cell
                && let CellKind::ToolUse { tool_name, call_id } = &cell.kind
            {
                self.render_tool_call_header(tool_name, call_id, &cell.source, lines);
            }
            if let Some(rc) = result_cell {
                self.render_tool_result_full(rc, input.as_ref(), lines);
            } else if let Some(cell) = invocation_cell {
                self.render_cell_content(cell, expanded, lines);
            }
            return;
        }

        if let Some(cell) = invocation_cell
            && let CellKind::ToolUse { tool_name, call_id } = &cell.kind
        {
            self.render_tool_call_header(tool_name, call_id, &cell.source, lines);
            if let Some(rc) = result_cell {
                self.render_tool_result_summary(rc, lines);
            }
            return;
        }
        if let Some(rc) = result_cell {
            self.render_tool_result_header(rc, lines);
            self.render_tool_result_summary(rc, lines);
        }
    }

    /// Render a single cell's body — text / thinking / tool-use header /
    /// tool-result / system row. Mirrors the chat-widget dispatch but
    /// uses the modal's expansion conventions (capped expanded output,
    /// "ctrl+o to expand" preview hint).
    fn render_cell_content(
        &self,
        cell: &RenderedCell,
        expanded: bool,
        lines: &mut Vec<Line<'static>>,
    ) {
        match &cell.kind {
            CellKind::UserText { text } => {
                if let Some(rendered) =
                    crate::presentation::slash_command::render_slash_command_user_text(
                        cell.source.as_ref(),
                        text,
                        crate::presentation::slash_command::SlashCommandRenderOptions {
                            styles: self.styles,
                            width: self.width,
                            syntax_highlighting: self.syntax_highlighting,
                            apply_user_background: false,
                        },
                    )
                {
                    lines.extend(rendered);
                } else {
                    self.render_text_block(">", text, lines);
                }
            }
            CellKind::AssistantText { text, .. } => self.render_text_block("⏺", text, lines),
            CellKind::AssistantThinking {
                text,
                metadata_anchor,
            } => {
                let meta = metadata_anchor
                    .then(|| self.reasoning_metadata.get(&cell.message_uuid))
                    .flatten();
                lines.extend(render_thinking_block(
                    ThinkingRenderInput {
                        content: text,
                        duration_ms: meta.and_then(|m| m.duration_ms),
                        reasoning_tokens: meta.map(|m| m.reasoning_tokens),
                        toggle_hint: Some(&self.thinking_toggle_hint),
                        display: if expanded {
                            ThinkingDisplay::Expanded {
                                max_body_lines: TRANSCRIPT_EXPANDED_CELL_LINE_CAP,
                                truncated_hint: TRANSCRIPT_TRUNCATED_HINT,
                            }
                        } else {
                            ThinkingDisplay::Collapsed
                        },
                    },
                    self.styles,
                ));
            }
            CellKind::AssistantRedactedThinking => lines.push(Line::from(
                Span::raw(t!("chat.redacted_thinking").to_string())
                    .fg(self.styles.thinking())
                    .dim()
                    .italic(),
            )),
            CellKind::ToolUse { call_id, tool_name } => {
                self.render_tool_call_header(tool_name, call_id, &cell.source, lines);
            }
            CellKind::ToolResult { .. } => {
                self.render_tool_result_header(cell, lines);
                if expanded {
                    self.render_tool_result_full(cell, None, lines);
                } else {
                    self.render_tool_result_summary(cell, lines);
                }
            }
            CellKind::Attachment => {
                // Memory injections collapse to `◆ memory · <path>` (path relative
                // to cwd); other attachments show their first body line behind a
                // width-1 hollow `◇`. Silent / structured payloads render nothing.
                if let Some(path) = crate::transcript::render::compact_file_reference_chip_path(
                    cell.source.as_ref(),
                    self.cwd,
                ) {
                    lines.push(Line::from(vec![
                        Span::raw("◇ ").fg(self.styles.accent()).dim(),
                        Span::raw("Referenced file ").fg(self.styles.dim()),
                        Span::raw(path).fg(self.styles.dim()).bold(),
                    ]));
                } else if let Some(path) = crate::transcript::render::nested_memory_chip_path(
                    cell.source.as_ref(),
                    self.cwd,
                ) {
                    lines.push(Line::from(vec![
                        Span::raw("◆ ").fg(self.styles.accent()).dim(),
                        Span::raw("memory · ").fg(self.styles.dim()),
                        Span::raw(path).fg(self.styles.dim()),
                    ]));
                } else if let Some(summary) =
                    crate::transcript::render::attachment_summary_text(cell.source.as_ref())
                {
                    lines.push(Line::from(vec![
                        Span::raw("◇ ").fg(self.styles.accent()).dim(),
                        Span::raw(summary).fg(self.styles.dim()),
                    ]));
                }
            }
            CellKind::System(kind) => self.render_system_cell(kind, &cell.source, lines),
        }
    }

    fn render_system_cell(
        &self,
        kind: &SystemCellKind,
        source: &Arc<Message>,
        lines: &mut Vec<Line<'static>>,
    ) {
        match kind {
            SystemCellKind::UserInterruption { .. } => {
                lines.push(Line::from(
                    Span::raw(t!("chat.interrupted_marker").to_string()).fg(self.styles.dim()),
                ));
            }
            SystemCellKind::Informational => {
                let Message::System(SystemMessage::Informational(info)) = source.as_ref() else {
                    return;
                };
                let body = if info.title.is_empty() {
                    info.message.clone()
                } else {
                    format!("{}: {}", info.title, info.message)
                };
                self.render_text_block("#", &body, lines);
            }
            SystemCellKind::ApiError => {
                let Message::System(SystemMessage::ApiError(e)) = source.as_ref() else {
                    return;
                };
                let status = e.status_code.map(|c| format!(" [{c}]")).unwrap_or_default();
                lines.push(Line::from(
                    Span::raw(format!("⚠{status} {error}", error = e.error))
                        .fg(self.styles.error()),
                ));
            }
            SystemCellKind::CompactBoundary => {
                lines.push(Line::from(
                    Span::raw(
                        t!(
                            "chat.compact_boundary",
                            shortcut = self.compact_boundary_shortcut.as_str()
                        )
                        .to_string(),
                    )
                    .fg(self.styles.border())
                    .dim(),
                ));
            }
            SystemCellKind::LocalCommand => {
                let Message::System(SystemMessage::LocalCommand(lc)) = source.as_ref() else {
                    return;
                };
                lines.push(Line::from(vec![
                    Span::raw("! ").fg(self.styles.accent()).bold(),
                    Span::raw(lc.command.clone()).fg(self.styles.accent()),
                ]));
                self.render_capped_lines("  ", &lc.output, self.styles.dim(), lines);
            }
            _ => {
                let body = system_summary_text(source.as_ref()).unwrap_or_default();
                if !body.is_empty() {
                    self.render_text_block("#", &body, lines);
                }
            }
        }
    }

    fn render_tool_call_header(
        &self,
        tool_name: &str,
        call_id: &str,
        source: &Arc<Message>,
        lines: &mut Vec<Line<'static>>,
    ) {
        let execution = self
            .tool_executions
            .iter()
            .find(|tool| tool.call_id == call_id);
        let input_preview =
            crate::transcript::derive::tool_call_header_preview_model(source, call_id, tool_name);
        let preview_spans = crate::tool_display::render_tool_input_preview_spans(
            &input_preview,
            self.styles,
            self.syntax_highlighting,
            96,
        );
        let elapsed = execution
            .map(|tool| format!(" ({})", format_duration_seconds(tool.elapsed())))
            .unwrap_or_default();
        let tone = tool_tone_color(tool_name_tone(tool_name), self.styles);
        let mut spans = vec![
            Span::raw("● ").fg(tone),
            Span::raw(tool_name.to_string()).fg(tone).bold(),
        ];
        if !preview_spans.is_empty() {
            spans.push(Span::raw("(").fg(self.styles.text()));
            spans.extend(preview_spans);
            spans.push(Span::raw(")").fg(self.styles.text()));
        }
        spans.push(Span::raw(elapsed).fg(self.styles.dim()).dim());
        lines.push(Line::from(spans));
    }

    fn render_tool_result_summary(&self, cell: &RenderedCell, lines: &mut Vec<Line<'static>>) {
        let Message::ToolResult(tr) = cell.source.as_ref() else {
            return;
        };
        let Some(projection) = tool_result_output(cell.source.as_ref()) else {
            return;
        };
        if tr.is_error {
            lines.push(result_line(
                format!(
                    "error: {}",
                    single_line_capped(&projection.output, TRANSCRIPT_LINE_CHAR_CAP)
                ),
                self.styles.error(),
            ));
            return;
        }
        self.render_output_preview(&projection.output, lines);
    }

    fn render_tool_result_header(&self, cell: &RenderedCell, lines: &mut Vec<Line<'static>>) {
        let Message::ToolResult(tr) = cell.source.as_ref() else {
            return;
        };
        let Some(projection) = tool_result_output(cell.source.as_ref()) else {
            return;
        };
        let glyph = if tr.is_error {
            ("● ", self.styles.tool_error())
        } else {
            ("● ", self.styles.tool_completed())
        };
        lines.push(Line::from(vec![
            Span::raw(glyph.0).fg(glyph.1),
            Span::raw(projection.tool_name)
                .fg(self.styles.text())
                .bold(),
        ]));
    }

    /// Expanded (full-detail) tool result. Shares the inline chat's per-tool
    /// renderer so a diff / highlighted code / web target shows here too — this
    /// is the view the inline "… (ctrl+o to expand)" hint defers to. `input` is
    /// the issuing call's arguments when the invocation cell is on hand.
    fn render_tool_result_full(
        &self,
        cell: &RenderedCell,
        input: Option<&serde_json::Value>,
        lines: &mut Vec<Line<'static>>,
    ) {
        let Message::ToolResult(tr) = cell.source.as_ref() else {
            return;
        };
        let Some(projection) = tool_result_output(cell.source.as_ref()) else {
            return;
        };
        crate::transcript::render::tool_result::render_tool_result_body(
            &self.tool_result_ctx(/*expanded*/ true),
            &projection.tool_name,
            input,
            &projection.output,
            projection.display_data,
            tr.is_error,
            lines,
        );
    }

    fn render_output_preview(&self, output: &str, lines: &mut Vec<Line<'static>>) {
        match tool_output_preview(output, TRANSCRIPT_COLLAPSED_PREVIEW_LINES) {
            ToolOutputPreview::Empty => {
                lines.push(result_line("(no output)".to_string(), self.styles.dim()));
            }
            ToolOutputPreview::Full(output_lines) => {
                for (index, line) in output_lines.into_iter().enumerate() {
                    lines.push(output_result_line(
                        transcript_safe_line(line),
                        self.styles.text(),
                        index == 0,
                    ));
                }
            }
            ToolOutputPreview::Truncated {
                head,
                omitted,
                tail,
            } => {
                let mut rendered = 0usize;
                for line in head {
                    lines.push(output_result_line(
                        transcript_safe_line(line),
                        self.styles.text(),
                        rendered == 0,
                    ));
                    rendered += 1;
                }
                lines.push(output_result_line(
                    format!("… +{omitted} lines (ctrl+o to expand)"),
                    self.styles.dim(),
                    rendered == 0,
                ));
                for line in tail {
                    lines.push(output_result_line(
                        transcript_safe_line(line),
                        self.styles.text(),
                        false,
                    ));
                }
            }
        }
    }

    fn render_text_block(&self, marker: &str, text: &str, lines: &mut Vec<Line<'static>>) {
        let mut iter = text.lines();
        if let Some(first) = iter.next() {
            lines.push(Line::from(vec![
                Span::raw(format!("{marker} ")).fg(self.styles.dim()),
                Span::raw(transcript_safe_line(first)).fg(self.styles.text()),
            ]));
            for line in iter.take(TRANSCRIPT_EXPANDED_CELL_LINE_CAP.saturating_sub(1)) {
                lines.push(Line::from(
                    Span::raw(format!("  {}", transcript_safe_line(line))).fg(self.styles.text()),
                ));
            }
        } else {
            lines.push(Line::from(
                Span::raw(marker.to_string()).fg(self.styles.dim()),
            ));
        }
    }

    fn render_capped_lines(
        &self,
        prefix: &str,
        text: &str,
        color: ratatui::style::Color,
        lines: &mut Vec<Line<'static>>,
    ) {
        let mut iter = text.lines();
        for line in iter.by_ref().take(TRANSCRIPT_EXPANDED_CELL_LINE_CAP) {
            lines.push(Line::from(
                Span::raw(format!("{prefix}{}", transcript_safe_line(line))).fg(color),
            ));
        }
        if iter.next().is_some() {
            lines.push(Line::from(
                Span::raw(format!("{prefix}{TRANSCRIPT_TRUNCATED_HINT}"))
                    .fg(self.styles.dim())
                    .italic(),
            ));
        }
    }

    fn render_meta_preview(&self, cell: &RenderedCell, lines: &mut Vec<Line<'static>>) {
        const PREVIEW_CHARS: usize = 50;
        let raw = meta_preview_text(cell);
        let single_line = raw.lines().next().unwrap_or("");
        let trimmed = single_line.split_whitespace().collect::<Vec<_>>().join(" ");
        let preview = truncate_chars(&trimmed, PREVIEW_CHARS);
        lines.push(Line::from(vec![
            Span::raw("  # [meta] ").fg(self.styles.system_message()),
            Span::raw(preview).fg(self.styles.dim()).italic(),
        ]));
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
        CellKind::Attachment | CellKind::AssistantRedactedThinking => 0,
    }
}

fn meta_preview_text(cell: &RenderedCell) -> String {
    // Only System cells collapse to a meta preview now — attachments render as
    // content rows (see `presentation::transcript::is_meta`).
    let Message::System(sm) = cell.source.as_ref() else {
        return String::new();
    };
    match sm {
        SystemMessage::Informational(info) => {
            if info.title.is_empty() {
                info.message.clone()
            } else {
                format!("{}: {}", info.title, info.message)
            }
        }
        SystemMessage::ApiError(e) => e.error.clone(),
        SystemMessage::LocalCommand(lc) => lc.command.clone(),
        SystemMessage::PermissionRetry(m) => format!("{} · {}", m.tool_name, m.message),
        SystemMessage::BridgeStatus(m) => m.message.clone().unwrap_or_default(),
        SystemMessage::CompactBoundary(_)
        | SystemMessage::MicrocompactBoundary(_)
        | SystemMessage::UserInterruption(_)
        | SystemMessage::MemorySaved(_)
        | SystemMessage::AwaySummary(_)
        | SystemMessage::AgentsKilled(_)
        | SystemMessage::ApiMetrics(_)
        | SystemMessage::StopHookSummary(_)
        | SystemMessage::TurnDuration(_)
        | SystemMessage::ScheduledTaskFire(_)
        | SystemMessage::ContextUsage(_) => String::new(),
    }
}

fn system_summary_text(msg: &Message) -> Option<String> {
    let Message::System(sm) = msg else {
        return None;
    };
    Some(match sm {
        SystemMessage::PermissionRetry(m) => {
            format!("permission retry · {} · {}", m.tool_name, m.message)
        }
        SystemMessage::BridgeStatus(m) => match (m.connected, m.message.as_deref()) {
            (true, Some(msg)) => format!("bridge connected · {msg}"),
            (true, None) => "bridge connected".to_string(),
            (false, Some(msg)) => format!("bridge disconnected · {msg}"),
            (false, None) => "bridge disconnected".to_string(),
        },
        SystemMessage::MemorySaved(_) => "memory saved".to_string(),
        SystemMessage::AwaySummary(_) => "away summary".to_string(),
        SystemMessage::AgentsKilled(_) => "agents killed".to_string(),
        SystemMessage::ApiMetrics(_) => "API metrics".to_string(),
        SystemMessage::StopHookSummary(_) => "stop hook summary".to_string(),
        SystemMessage::TurnDuration(_) => "turn duration".to_string(),
        SystemMessage::ScheduledTaskFire(_) => "scheduled task".to_string(),
        SystemMessage::ContextUsage(_) => "context usage".to_string(),
        SystemMessage::Informational(_)
        | SystemMessage::ApiError(_)
        | SystemMessage::CompactBoundary(_)
        | SystemMessage::MicrocompactBoundary(_)
        | SystemMessage::LocalCommand(_)
        | SystemMessage::UserInterruption(_) => return None,
    })
}

fn compact_boundary_shortcut(state: &AppState) -> String {
    state
        .ui
        .kb_handle
        .display_for(&KeybindingAction::AppToggleTranscript, TuiContext::Chat)
        .unwrap_or_else(|| "ctrl+o".to_string())
}

fn thinking_toggle_hint(state: &AppState) -> String {
    let shortcut = state
        .ui
        .kb_handle
        .display_for(&KeybindingAction::ChatThinkingToggle, TuiContext::Chat)
        .unwrap_or_else(|| "F2".to_string());
    let key = if state.ui.show_thinking {
        "status.thinking_toggle_collapse"
    } else {
        "status.thinking_toggle_expand"
    };
    t!(key, shortcut = shortcut.as_str()).to_string()
}

fn plan_editor_hint(state: &AppState) -> String {
    let shortcut = state
        .ui
        .kb_handle
        .display_for(&KeybindingAction::AppPlanEditor, TuiContext::Chat)
        .unwrap_or_else(|| "ctrl+g".to_string());
    format!("{shortcut} to edit")
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

fn result_line(text: String, color: ratatui::style::Color) -> Line<'static> {
    output_result_line(text, color, true)
}

fn tool_tone_color(
    tone: ToolNameTone,
    styles: coco_tui_ui::style::UiStyles<'_>,
) -> ratatui::style::Color {
    match tone {
        ToolNameTone::ReadOnly => styles.success(),
        ToolNameTone::Shell => styles.primary(),
        ToolNameTone::Write => styles.warning(),
        ToolNameTone::Agent => styles.accent(),
        ToolNameTone::Plan => styles.plan(),
        ToolNameTone::Utility => styles.secondary(),
    }
}

fn output_result_line(text: String, color: ratatui::style::Color, first: bool) -> Line<'static> {
    let prefix = if first { "    └ " } else { "      " };
    Line::from(vec![Span::raw(prefix).fg(color), Span::raw(text).fg(color)])
}

fn single_line_capped(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (index, part) in text.split_whitespace().enumerate() {
        if index > 0 {
            push_capped(&mut out, " ", max_chars);
        }
        push_capped(&mut out, part, max_chars);
        if out.chars().count() >= max_chars {
            break;
        }
    }
    out
}

fn transcript_safe_line(line: &str) -> String {
    truncate_chars(line, TRANSCRIPT_LINE_CHAR_CAP)
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    text.chars().take(max_chars).collect()
}

fn push_capped(out: &mut String, text: &str, max_chars: usize) {
    let remaining = max_chars.saturating_sub(out.chars().count());
    out.extend(text.chars().take(remaining));
}

fn wrapped_height(lines: &[Line<'static>], width: u16) -> usize {
    let width = usize::from(width).max(1);
    lines
        .iter()
        .map(|line| {
            let line_width = line
                .spans
                .iter()
                .map(|span| text_width(span.content.as_ref()))
                .sum::<usize>();
            line_width.saturating_add(width - 1) / width
        })
        .map(|rows| rows.max(1))
        .sum::<usize>()
        .max(1)
}

#[cfg(test)]
#[path = "transcript_modal.test.rs"]
mod tests;
