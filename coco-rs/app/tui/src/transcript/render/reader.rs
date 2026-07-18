//! Expanded Ctrl+O transcript-cell projection.
//!
//! The reader and its search corpus share this renderer so row geometry,
//! expansion semantics, and searchable text cannot drift. Frame composition
//! remains in widgets; transcript rendering stays in this module.

pub(crate) use super::reader_helpers::meta_preview_text;
use super::reader_helpers::*;

use std::collections::HashMap;
use std::sync::Arc;

use coco_messages::Message;
use coco_messages::SystemMessage;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use ratatui::widgets::Wrap;

use crate::i18n::t;
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
use crate::state::AppState;
use crate::state::session::ToolExecution;
use crate::state::session::ToolStatus;
use crate::state::transcript::TranscriptCellId;
use crate::state::transcript::TranscriptSearch;
use crate::tool_display::tool_name_tone;
use crate::transcript::cells::CellKind;
use crate::transcript::cells::RenderedCell;
use crate::transcript::cells::SystemCellKind;
use crate::transcript::derive::extract_tool_call_input;
use crate::transcript::derive::tool_result_output;
use crate::transcript::render::tool_result::ToolResultRenderCtx;
use coco_tui_ui::display::SyntaxHighlighting;
use coco_tui_ui::style::UiStyles;

pub(crate) struct TranscriptCellRenderer<'a> {
    cells: &'a [RenderedCell],
    search: &'a TranscriptSearch,
    tool_executions: &'a [ToolExecution],
    reasoning_metadata: &'a HashMap<uuid::Uuid, crate::state::session::ReasoningMetadata>,
    subagent_summaries: &'a HashMap<String, crate::state::session::SubagentRunSummary>,
    compact_boundary_shortcut: String,
    thinking_toggle_hint: String,
    plan_editor_hint: String,
    pub(crate) width: u16,
    styles: UiStyles<'a>,
    syntax_highlighting: SyntaxHighlighting,
    cwd: Option<&'a str>,
}

impl<'a> TranscriptCellRenderer<'a> {
    pub(crate) fn new(
        cells: &'a [RenderedCell],
        state: &'a AppState,
        search: &'a TranscriptSearch,
        styles: UiStyles<'a>,
        width: u16,
    ) -> Self {
        Self {
            cells,
            search,
            tool_executions: &state.session.tool_executions,
            reasoning_metadata: &state.session.reasoning_metadata,
            subagent_summaries: &state.session.subagent_summaries,
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
            truncation_observed: None,
        }
    }

    pub(crate) fn desired_height(
        &self,
        cell: &TranscriptSourceCell<'a>,
        expanded: bool,
        selected: bool,
    ) -> usize {
        let lines = self.render_cell(cell, expanded, selected);
        wrapped_height(&lines, self.width)
    }

    /// Live execution status of the tool a cell renders, if it renders one.
    ///
    /// UI-only tool state that can affect wrapping. The elapsed badge is live,
    /// but only its displayed width belongs in a height key; values with the
    /// same width render to the same number of columns.
    pub(crate) fn tool_layout_for(
        &self,
        cell_id: &TranscriptCellId,
    ) -> Option<(ToolStatus, usize)> {
        let TranscriptCellId::ToolCall { call_id } = cell_id else {
            return None;
        };
        self.tool_executions
            .iter()
            .find(|tool| &tool.call_id == call_id)
            .map(|tool| {
                (
                    tool.status,
                    format_duration_seconds(tool.elapsed()).chars().count(),
                )
            })
    }

    pub(crate) fn reasoning_layout_for(
        &self,
        cell: &TranscriptSourceCell<'_>,
    ) -> Option<(Option<i64>, i64)> {
        let TranscriptSourceCell::Committed(TranscriptCell::Cell { index }) = cell else {
            return None;
        };
        let rendered = self.cells.get(*index)?;
        let metadata_anchor = match &rendered.kind {
            CellKind::AssistantThinking {
                metadata_anchor, ..
            }
            | CellKind::AssistantRedactedThinking { metadata_anchor } => *metadata_anchor,
            _ => false,
        };
        metadata_anchor
            .then(|| self.reasoning_metadata.get(&rendered.message_uuid))
            .flatten()
            .map(|meta| (meta.duration_ms, meta.reasoning_tokens))
    }

    pub(crate) fn render_window(
        &self,
        cell: &TranscriptSourceCell<'a>,
        area: Rect,
        skip_lines: usize,
        expanded: bool,
        selected: bool,
        buf: &mut Buffer,
    ) {
        let mut lines = self.render_cell(cell, expanded, selected);
        if !self.search.query.is_empty() {
            let cell_id = cell.cell_id(self.cells);
            let current = self
                .search
                .current_match()
                .filter(|current| cell_id.as_ref() == Some(&current.cell_id));
            crate::transcript::search::apply_highlights(
                &mut lines,
                &self.search.query,
                current,
                self.styles,
            );
        }
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((skip_lines.min(u16::MAX as usize) as u16, 0))
            .render(area, buf);
    }

    pub(crate) fn render_cell(
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
                let display_text =
                    crate::transcript::derive::user_display_text(cell.source.as_ref(), text);
                if let Some(rendered) =
                    crate::presentation::slash_command::render_slash_command_user_text(
                        cell.source.as_ref(),
                        &display_text,
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
                    self.render_text_block(">", &display_text, lines);
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
            CellKind::AssistantRedactedThinking { .. } => lines.push(Line::from(
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
                        Span::raw("Referenced file ").style(self.styles.dim_style()),
                        Span::raw(path).style(self.styles.dim_style()).bold(),
                    ]));
                } else if let Some(path) = crate::transcript::render::nested_memory_chip_path(
                    cell.source.as_ref(),
                    self.cwd,
                ) {
                    lines.push(Line::from(vec![
                        Span::raw("◆ ").fg(self.styles.accent()).dim(),
                        Span::raw("memory · ").style(self.styles.dim_style()),
                        Span::raw(path).style(self.styles.dim_style()),
                    ]));
                } else if let Some(summary) =
                    crate::transcript::render::attachment_summary_text(cell.source.as_ref())
                {
                    lines.push(Line::from(vec![
                        Span::raw("◇ ").fg(self.styles.accent()).dim(),
                        Span::raw(summary).style(self.styles.dim_style()),
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
                    Span::raw(t!("chat.interrupted_marker").to_string())
                        .style(self.styles.dim_style()),
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
        let header_name =
            crate::transcript::render::tool::tool_header_display_name(tool_name, source, call_id);
        let mut spans = vec![
            Span::raw("● ").fg(tone),
            Span::raw(header_name).fg(tone).bold(),
        ];
        if !preview_spans.is_empty() {
            spans.push(Span::raw("(").fg(self.styles.text()));
            spans.extend(preview_spans);
            spans.push(Span::raw(")").fg(self.styles.text()));
        }
        spans.push(Span::raw(elapsed).style(self.styles.dim_style()));
        lines.push(Line::from(spans));
    }

    fn render_tool_result_summary(&self, cell: &RenderedCell, lines: &mut Vec<Line<'static>>) {
        let Message::ToolResult(tr) = cell.source.as_ref() else {
            return;
        };
        let Some(projection) = tool_result_output(cell.source.as_ref()) else {
            return;
        };
        self.render_agent_summary(&projection.tool_name, tr, lines);
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
        self.render_agent_summary(&projection.tool_name, tr, lines);
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

    fn render_agent_summary(
        &self,
        tool_name: &str,
        result: &coco_messages::ToolResultMessage,
        lines: &mut Vec<Line<'static>>,
    ) {
        if tool_name == coco_types::ToolName::Agent.as_str()
            && let Some(summary) = self.subagent_summaries.get(&result.tool_use_id)
        {
            lines.push(crate::transcript::render::tool::agent_summary_line(
                self.styles,
                summary,
            ));
        }
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
                Span::raw(format!("{marker} ")).style(self.styles.dim_style()),
                Span::raw(transcript_safe_line(first)).fg(self.styles.text()),
            ]));
            for line in iter.take(TRANSCRIPT_EXPANDED_CELL_LINE_CAP.saturating_sub(1)) {
                lines.push(Line::from(
                    Span::raw(format!("  {}", transcript_safe_line(line))).fg(self.styles.text()),
                ));
            }
        } else {
            lines.push(Line::from(
                Span::raw(marker.to_string()).style(self.styles.dim_style()),
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
                    .style(self.styles.dim_style())
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
            Span::raw(preview).style(self.styles.dim_style()).italic(),
        ]));
    }
}
