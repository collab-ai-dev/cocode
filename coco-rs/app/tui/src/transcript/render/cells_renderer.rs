//! The cells renderer — projects the engine-authoritative
//! `&[RenderedCell]` slice into owned `Line`s via the per-category
//! renderer siblings (`super::{assistant, user, system, tool, tool_result}`).
//!
//! Dispatches on `cell.kind` + `cell.source: Arc<Message>` directly —
//! engine `MessageHistory` is the only source of truth, with no parallel
//! TUI-side projection. Formerly `widgets::chat::ChatWidget`; it is not a
//! ratatui `Widget` (callers consume [`CellsRenderer::build_lines_owned`]).

use std::collections::HashMap;
use std::collections::HashSet;

use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;

use coco_types::AttachmentKind;

use crate::i18n::t;
use crate::presentation::streaming::StreamingTailView;
use crate::presentation::thinking::ThinkingDisplay;
use crate::presentation::thinking::ThinkingRenderInput;
use crate::presentation::thinking::format_duration_seconds;
use crate::presentation::thinking::render_thinking_block;
use crate::presentation::transcript::ActiveTranscriptCell;
use crate::presentation::transcript::AssistantPresentationOrder;
use crate::presentation::transcript::TRANSCRIPT_LINE_CHAR_CAP;
use crate::presentation::transcript::TranscriptCell;
use crate::presentation::transcript::TranscriptPresentationInput;
use crate::presentation::transcript::TranscriptProjectionOptions;
use crate::presentation::transcript::TranscriptSourceCell;
use crate::presentation::transcript::native_history_presentation;
use crate::presentation::transcript::transcript_presentation;
use crate::state::session::ToolExecution;
use crate::state::ui::StreamingState;
use crate::tool_display::ToolNameTone;
use crate::tool_display::tool_name_tone;
use crate::transcript::cells::CellKind;
use crate::transcript::cells::RenderedCell;
use crate::transcript::cells::SystemCellKind;
use crate::transcript::stream::streaming_cursor_line;
use coco_tui_ui::display::SyntaxHighlighting;
use coco_tui_ui::style::UiStyles;

pub(super) const TOOL_OUTPUT_PREVIEW_ROWS: usize = 5;

/// Per-cell render cost above which a `tui::perf::cell` debug line is emitted,
/// attributing slow history builds to the specific cell (tool name / kind).
const SLOW_CELL_RENDER_LOG_THRESHOLD: std::time::Duration = std::time::Duration::from_millis(2);

/// Chat history widget.
///
/// Phase 3d (§6): consumes the engine-authoritative `&[RenderedCell]`
/// slice from `session.transcript.cells()` end-to-end. The per-category
/// renderers (`super::{user, assistant, tool, system}`) dispatch on
/// `cell.kind` + `cell.source` directly.
pub struct CellsRenderer<'a> {
    cells: &'a [RenderedCell],
    streaming: Option<&'a StreamingState>,
    pub(super) show_thinking: bool,
    show_system_reminders: bool,
    pub(crate) tool_executions: &'a [ToolExecution],
    collapsed_tools: Option<&'a HashSet<String>>,
    /// Side-cache lookup for `AssistantThinking` cells.
    /// `None` ⇒ no reasoning badges (renderer falls back to header without metrics).
    pub(crate) reasoning_metadata:
        Option<&'a HashMap<uuid::Uuid, crate::state::session::ReasoningMetadata>>,
    /// Side-cache lookup for completed-subagent run summaries, keyed by the
    /// spawning Agent tool_use_id. `None` ⇒ Agent tool-result cells render
    /// without the `✓ Explore · …` summary row.
    pub(crate) subagent_summaries:
        Option<&'a HashMap<String, crate::state::session::SubagentRunSummary>>,
    pub(crate) styles: UiStyles<'a>,
    pub(crate) syntax_highlighting: SyntaxHighlighting,
    pub(crate) width: u16,
    assistant_presentation_order: AssistantPresentationOrder,
    /// Session working directory, used to show memory-chip paths relative to it.
    /// `None` (tests / no session) falls back to the absolute path.
    pub(crate) cwd: Option<&'a str>,
    /// Keybinding handle for rendering live shortcuts (e.g. the
    /// `…(<chord> to see full summary)` hint). `None` falls back to
    /// the default literal — used in tests that build a CellsRenderer
    /// without an `AppState`.
    pub(crate) kb_handle: Option<&'a crate::keybinding_resolver::KeybindingHandle>,
    pub(crate) show_thinking_internal: bool,
}

impl<'a> CellsRenderer<'a> {
    pub fn new(cells: &'a [RenderedCell], styles: UiStyles<'a>) -> Self {
        Self {
            cells,
            streaming: None,
            show_thinking: false,
            show_system_reminders: false,
            tool_executions: &[],
            collapsed_tools: None,
            reasoning_metadata: None,
            subagent_summaries: None,
            styles,
            syntax_highlighting: SyntaxHighlighting::Enabled,
            width: 80,
            assistant_presentation_order: AssistantPresentationOrder::Source,
            cwd: None,
            kb_handle: None,
            show_thinking_internal: false,
        }
    }

    pub fn cwd(mut self, cwd: Option<&'a str>) -> Self {
        self.cwd = cwd;
        self
    }

    pub fn reasoning_metadata(
        mut self,
        meta: &'a HashMap<uuid::Uuid, crate::state::session::ReasoningMetadata>,
    ) -> Self {
        self.reasoning_metadata = Some(meta);
        self
    }

    pub fn subagent_summaries(
        mut self,
        summaries: &'a HashMap<String, crate::state::session::SubagentRunSummary>,
    ) -> Self {
        self.subagent_summaries = Some(summaries);
        self
    }

    pub fn kb_handle(mut self, handle: &'a crate::keybinding_resolver::KeybindingHandle) -> Self {
        self.kb_handle = Some(handle);
        self
    }

    pub fn streaming(mut self, state: Option<&'a StreamingState>) -> Self {
        self.streaming = state;
        self
    }
    pub fn show_thinking(mut self, show: bool) -> Self {
        self.show_thinking = show;
        self.show_thinking_internal = show;
        self
    }
    pub fn show_system_reminders(mut self, show: bool) -> Self {
        self.show_system_reminders = show;
        self
    }
    pub fn tool_executions(mut self, tools: &'a [ToolExecution]) -> Self {
        self.tool_executions = tools;
        self
    }
    pub fn collapsed_tools(mut self, collapsed: &'a HashSet<String>) -> Self {
        self.collapsed_tools = Some(collapsed);
        self
    }
    pub fn width(mut self, w: u16) -> Self {
        self.width = w;
        self
    }
    pub fn syntax_highlighting(mut self, syntax_highlighting: SyntaxHighlighting) -> Self {
        self.syntax_highlighting = syntax_highlighting;
        self
    }
    pub(crate) fn native_history_presentation(mut self) -> Self {
        self.assistant_presentation_order = AssistantPresentationOrder::TextBeforeLeadingThinking;
        self
    }
    /// Build lines that own their text for native history emission.
    pub fn build_lines_owned(&self) -> Vec<Line<'static>> {
        self.build_lines()
    }

    fn build_lines(&self) -> Vec<Line<'static>> {
        let input = TranscriptPresentationInput {
            cells: self.cells,
            options: TranscriptProjectionOptions {
                show_system_reminders: self.show_system_reminders,
                show_compact_internals: false,
            },
            streaming: self.streaming,
            show_thinking: self.show_thinking,
            tool_executions: self.tool_executions,
        };
        let presentation =
            if self.assistant_presentation_order == AssistantPresentationOrder::Source {
                transcript_presentation(input)
            } else {
                native_history_presentation(input)
            };
        let mut lines: Vec<Line<'static>> = Vec::new();

        for cell in presentation.cells {
            let cell_started = std::time::Instant::now();
            let lines_before = lines.len();
            self.render_transcript_cell(self.cells, &cell, false, false, &mut lines);
            let elapsed = cell_started.elapsed();
            if elapsed >= SLOW_CELL_RENDER_LOG_THRESHOLD {
                tracing::debug!(
                    target: "tui::perf::cell",
                    cell = %cell_perf_label(self.cells, &cell),
                    lines_added = lines.len() - lines_before,
                    duration_us = elapsed.as_micros(),
                    "slow transcript cell render",
                );
            }
        }

        lines
    }

    fn render_transcript_cell(
        &self,
        cells: &[RenderedCell],
        cell: &TranscriptSourceCell<'_>,
        expanded: bool,
        selected: bool,
        lines: &mut Vec<Line<'static>>,
    ) {
        let start_line = lines.len();
        match cell {
            TranscriptSourceCell::Committed(TranscriptCell::MetaPreview { index }) => {
                if let Some(c) = cells.get(*index) {
                    self.render_meta_preview(c, lines);
                }
            }
            TranscriptSourceCell::Committed(TranscriptCell::Cell { index }) => {
                if let Some(c) = cells.get(*index) {
                    self.render_cell_with_expansion(c, expanded, lines);
                    lines.push(Line::default());
                }
            }
            TranscriptSourceCell::Committed(TranscriptCell::ToolCall {
                invocation,
                result,
                ..
            }) => {
                self.render_tool_call(cells, *invocation, *result, expanded, lines);
                lines.push(Line::default());
            }
            TranscriptSourceCell::Committed(TranscriptCell::ToolBatch { start, end, count }) => {
                let mut text = format!("  ‖ {}", t!("chat.tools_in_parallel", count = count));
                let names =
                    crate::presentation::transcript::tool_batch_name_summary(cells, *start, *end);
                if !names.is_empty() {
                    text.push_str(" · ");
                    text.push_str(&names);
                }
                lines.push(Line::from(Span::raw(text).fg(self.styles.secondary())));
                lines.push(Line::default());
            }
            TranscriptSourceCell::Active(ActiveTranscriptCell::Streaming(view)) => {
                self.render_streaming(view.clone(), lines);
            }
            TranscriptSourceCell::Active(ActiveTranscriptCell::InFlightTools) => {
                lines.extend(in_flight_tool_lines(self.tool_executions, self.styles));
            }
        }
        if selected {
            if start_line < lines.len() {
                if let Some(first) = lines.get_mut(start_line) {
                    first
                        .spans
                        .insert(0, Span::raw("▶ ").fg(self.styles.primary()));
                }
            } else {
                lines.push(Line::from(Span::raw("▶").fg(self.styles.primary())));
            }
        }
    }

    fn render_tool_call(
        &self,
        cells: &[RenderedCell],
        invocation: Option<usize>,
        result: Option<usize>,
        expanded: bool,
        lines: &mut Vec<Line<'static>>,
    ) {
        if expanded {
            if let Some(index) = invocation
                && let Some(c) = cells.get(index)
            {
                self.render_cell(c, lines);
            }
            if let Some(index) = result
                && let Some(c) = cells.get(index)
            {
                self.render_cell(c, lines);
            }
            return;
        }

        let invocation_cell = invocation.and_then(|index| cells.get(index));
        let result_cell = result.and_then(|index| cells.get(index));

        if let Some(cell) = invocation_cell
            && let CellKind::ToolUse { tool_name, call_id } = &cell.kind
        {
            self.render_tool_call_header(tool_name, call_id, &cell.source, lines);
            if let Some(rc) = result_cell {
                // Paired path: the invocation cell carries the tool input, so
                // input-derived views (diffs, code, web target) can render.
                let input = crate::transcript::derive::extract_tool_call_input(
                    cell.source.as_ref(),
                    call_id,
                );
                self.render_tool_result(input.as_ref(), rc, lines);
            }
            return;
        }

        if let Some(rc) = result_cell {
            self.render_tool_result(None, rc, lines);
        }
    }

    fn render_cell_with_expansion(
        &self,
        cell: &RenderedCell,
        expanded: bool,
        lines: &mut Vec<Line<'static>>,
    ) {
        if !expanded {
            match &cell.kind {
                CellKind::AssistantThinking { .. } => {
                    if !self.show_thinking {
                        self.render_cell(cell, lines);
                        return;
                    }
                }
                CellKind::ToolResult { .. } => {
                    self.render_tool_result(None, cell, lines);
                    return;
                }
                _ => {}
            }
        }
        self.render_cell(cell, lines);
    }

    fn render_tool_call_header(
        &self,
        tool_name: &str,
        call_id: &str,
        source: &std::sync::Arc<coco_messages::Message>,
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
        // Agent (subagent) headers lead with the subagent TYPE
        // (`Explore` / `Plan` / custom) rather than the generic "Agent" — the
        // type is the meaningful operation, and it no longer repeats in the
        // run-summary line below. Pulled from the call's `subagent_type`
        // input; falls back to the tool name when absent.
        let header_name = if tool_name == coco_types::ToolName::Agent.as_str() {
            crate::transcript::derive::extract_tool_call_input(source, call_id)
                .as_ref()
                .and_then(|input| input.get("subagent_type"))
                .and_then(serde_json::Value::as_str)
                .filter(|ty| !ty.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| tool_name.to_string())
        } else {
            tool_name.to_string()
        };
        let mut spans = vec![
            Span::raw("● ").fg(tone),
            Span::raw(header_name).fg(tone).bold(),
        ];
        if !preview_spans.is_empty() {
            spans.push(Span::raw("(").fg(self.styles.text()));
            spans.extend(preview_spans);
            spans.push(Span::raw(")").fg(self.styles.text()));
        }
        spans.push(Span::raw(elapsed).fg(self.styles.dim()).dim());
        lines.push(Line::from(spans));
    }

    /// Render a tool-result cell body. `input` is the issuing call's arguments
    /// when the caller has the invocation cell on hand (paired path), enabling
    /// input-derived views (diffs, code, web target); `None` degrades to output.
    pub(crate) fn render_tool_result(
        &self,
        input: Option<&serde_json::Value>,
        result_cell: &RenderedCell,
        lines: &mut Vec<Line<'static>>,
    ) {
        let CellKind::ToolResult { .. } = &result_cell.kind else {
            return;
        };
        let coco_messages::Message::ToolResult(tr) = result_cell.source.as_ref() else {
            return;
        };
        let Some(projection) =
            crate::transcript::derive::tool_result_output(result_cell.source.as_ref())
        else {
            return;
        };
        // Subagent run summary on the Agent tool-result cell: the transient
        // panel drops a subagent the instant it finishes, so its final
        // stats (`✓ 37 tools · 1m11s · ↑68.1k ↓468 · cache 95% · $0.18`)
        // surface here as the committed transcript record. The type lives in
        // the `● Explore(...)` header above, not in this line.
        if projection.tool_name == coco_types::ToolName::Agent.as_str()
            && let Some(summary) = self.subagent_summaries.and_then(|m| m.get(&tr.tool_use_id))
        {
            lines.push(self.agent_summary_line(summary));
        }
        super::tool_result::render_tool_result_body(
            &self.tool_result_ctx(),
            &projection.tool_name,
            input,
            &projection.output,
            projection.display_data,
            tr.is_error,
            lines,
        );
    }

    /// `  └ ✓ 37 tools · 1m11s · ↑68.1k ↓468 · cache 95% · $0.18` — the
    /// committed run summary for a finished subagent (type is in the header).
    fn agent_summary_line(&self, s: &crate::state::session::SubagentRunSummary) -> Line<'static> {
        use crate::presentation::activity::format_short_tokens;
        let (glyph, tone) = if s.succeeded {
            ("✓", self.styles.success())
        } else {
            ("✗", self.styles.error())
        };
        // The subagent type now leads the invocation header
        // (`● Explore(...)`), so it's intentionally omitted here to avoid
        // repeating it one line below.
        let mut parts: Vec<String> = Vec::new();
        if s.tool_count > 0 {
            parts.push(format!("{} tools", s.tool_count));
        }
        if s.duration_ms > 0 {
            parts.push(format_duration_seconds(std::time::Duration::from_millis(
                s.duration_ms.max(0) as u64,
            )));
        }
        if s.input_tokens > 0 || s.output_tokens > 0 {
            parts.push(format!(
                "↑{} ↓{}",
                format_short_tokens(s.input_tokens),
                format_short_tokens(s.output_tokens)
            ));
            if s.input_tokens > 0 && s.cache_read_tokens > 0 {
                let pct = (s.cache_read_tokens * 100 / s.input_tokens).clamp(0, 100);
                parts.push(format!("cache {pct}%"));
            }
        }
        if s.cost_usd > 0.0 {
            parts.push(format!("${:.2}", s.cost_usd));
        }
        Line::from(vec![
            Span::raw("  └ ").fg(tone),
            Span::raw(format!("{glyph} ")).fg(tone),
            Span::raw(parts.join(" · ")).fg(self.styles.dim()),
        ])
    }

    /// Build the surface context the per-tool renderers paint into. Inline chat
    /// is never the full-detail surface, so caps stay tight and the truncation
    /// hint points at the Ctrl+O reader (which renders the same body expanded).
    pub(crate) fn tool_result_ctx(&self) -> super::tool_result::ToolResultRenderCtx<'_> {
        super::tool_result::ToolResultRenderCtx {
            styles: self.styles,
            width: self.width,
            syntax_highlighting: self.syntax_highlighting,
            plan_editor_hint: self.plan_editor_hint(),
            expand_hint: self.expand_hint(),
            expanded: false,
        }
    }

    fn expand_hint(&self) -> String {
        let chord = self
            .kb_handle
            .and_then(|handle| {
                handle.display_for(
                    &coco_keybindings::KeybindingAction::AppToggleTranscript,
                    crate::keybinding_bridge::KeybindingContext::Chat,
                )
            })
            .unwrap_or_else(|| "ctrl+o".to_string());
        format!("({chord} to expand)")
    }

    fn plan_editor_hint(&self) -> String {
        let chord = self
            .kb_handle
            .and_then(|handle| {
                handle.display_for(
                    &coco_keybindings::KeybindingAction::AppPlanEditor,
                    crate::keybinding_bridge::KeybindingContext::Chat,
                )
            })
            .unwrap_or_else(|| "ctrl+g".to_string());
        format!("{chord} to edit")
    }

    pub(crate) fn thinking_toggle_hint(&self) -> String {
        let chord = self
            .kb_handle
            .and_then(|handle| {
                handle.display_for(
                    &coco_keybindings::KeybindingAction::ChatThinkingToggle,
                    crate::keybinding_bridge::KeybindingContext::Chat,
                )
            })
            .unwrap_or_else(|| "F2".to_string());
        let key = if self.show_thinking_internal {
            "status.thinking_toggle_collapse"
        } else {
            "status.thinking_toggle_expand"
        };
        t!(key, shortcut = chord.as_str()).to_string()
    }

    /// Render a single-line collapsed preview for a meta (system reminder)
    /// cell. Keeps the user aware that system content exists without
    /// taking vertical space.
    fn render_meta_preview(&self, cell: &RenderedCell, lines: &mut Vec<Line<'static>>) {
        const PREVIEW_CHARS: usize = 50;
        let category = meta_category(&cell.kind);
        let raw = meta_preview_text(cell);
        let single_line: String = raw.lines().next().unwrap_or("").to_string();
        let trimmed: String = single_line.split_whitespace().collect::<Vec<_>>().join(" ");
        let preview = if trimmed.chars().count() > PREVIEW_CHARS {
            let mut s = trimmed.chars().take(PREVIEW_CHARS - 1).collect::<String>();
            s.push('\u{2026}');
            s
        } else {
            trimmed
        };
        lines.push(Line::from(vec![
            Span::raw(format!("  # [{category}] ")).fg(self.styles.system_message()),
            Span::raw(preview).fg(self.styles.dim()).italic(),
        ]));
    }

    fn render_cell(&self, cell: &RenderedCell, lines: &mut Vec<Line<'static>>) {
        // Dispatch to the first category whose renderer handles the variant.
        // Each submodule returns None when the variant is outside its scope,
        // keeping the individual match statements exhaustive-by-category.
        super::user::try_render(self, cell, lines)
            .or_else(|| super::assistant::try_render(self, cell, lines))
            .or_else(|| super::tool::try_render(self, cell, lines))
            .or_else(|| super::system::try_render(self, cell, lines));
    }

    fn render_streaming(&self, view: StreamingTailView<'_>, lines: &mut Vec<Line<'static>>) {
        if let Some(content) = view.assistant_text {
            // Render the in-flight stream through the same committed assistant
            // renderer that finalized `AssistantText` cells use
            // (`render_assistant`), with the streaming flag set so mermaid
            // layout runs once at finalize instead of per delta. The
            // non-native fallback no longer owns a streaming-only renderer
            // (§6.7-2); the native surface keeps its watermark splitter in
            // `transcript::stream` for mid-stream scrollback commits (§6.5).
            lines.extend(super::assistant::render_in_flight_assistant_markdown(
                content,
                super::assistant::CommittedAssistantMarkdownOptions {
                    styles: self.styles,
                    width: self.width,
                    syntax_highlighting: self.syntax_highlighting,
                },
            ));
            lines.push(streaming_cursor_line(self.styles));
        }

        if let Some(count) = view.thinking_tokens {
            lines.extend(render_thinking_block(
                ThinkingRenderInput {
                    content: "",
                    duration_ms: None,
                    reasoning_tokens: Some(count),
                    toggle_hint: Some(&self.thinking_toggle_hint()),
                    display: ThinkingDisplay::Collapsed,
                },
                self.styles,
            ));
        }
    }
}

pub(crate) fn assistant_stream_lead_marker(styles: UiStyles<'_>) -> coco_tui_markdown::LeadMarker {
    super::assistant::assistant_lead_marker(styles.assistant_message())
}

// ── Shared helpers ──

/// Short category label for a meta-preview row. Categorizes by
/// `CellKind` / system-cell sub-variant so users can identify what's
/// hidden by the system-reminder collapse.
fn meta_category(kind: &CellKind) -> &'static str {
    match kind {
        CellKind::System(SystemCellKind::Informational) => "system",
        CellKind::System(SystemCellKind::ApiError) => "api",
        CellKind::System(SystemCellKind::CompactBoundary) => "compact",
        CellKind::System(SystemCellKind::PermissionRetry) => "permission",
        CellKind::System(SystemCellKind::BridgeStatus) => "bridge",
        CellKind::System(SystemCellKind::MemorySaved) => "memory",
        CellKind::System(SystemCellKind::AwaySummary) => "away",
        CellKind::System(SystemCellKind::AgentsKilled) => "agents",
        CellKind::System(SystemCellKind::ApiMetrics) => "metrics",
        CellKind::System(SystemCellKind::StopHookSummary) => "hook",
        CellKind::System(SystemCellKind::TurnDuration) => "turn",
        CellKind::System(SystemCellKind::ScheduledTaskFire) => "schedule",
        CellKind::System(SystemCellKind::ContextUsage) => "context",
        _ => "meta",
    }
}

/// First meaningful line of a renderable attachment's body, for a one-line
/// transcript content row (shared by the chat widget and the Ctrl+O modal).
/// `None` when the attachment carries no displayable text (silent / structured
/// payloads). Renders the body, not a `[meta]` collapse.
pub(crate) fn attachment_summary_text(source: &coco_messages::Message) -> Option<String> {
    let coco_messages::Message::Attachment(att) = source else {
        return None;
    };
    // `CompactFileReference` renders as a chip; `File` is the `@`-mention
    // carrier — its generator listing ("The user @-mentioned the following
    // file(s)…") is model-only metadata that must not leak into the
    // transcript, and the display summary rides `mention_summary_lines` from
    // typed `extras` instead of this raw-body path.
    if matches!(
        att.kind,
        AttachmentKind::CompactFileReference | AttachmentKind::File
    ) {
        return None;
    }
    let body = strip_system_reminder_wrapper(&att.as_text_for_display());
    let first = body.lines().map(str::trim).find(|line| !line.is_empty())?;
    Some(first.to_string())
}

/// Compact transcript rows for a resolved `@`-mention summary attachment
/// (`AttachmentExtras::MentionSummary`), or `None` for any other message.
///
/// One `  └ ` row per item: `Read <path> (N lines)` for files,
/// `Listed directory <path>/` for directories — mirroring the reference TUIs.
/// Returns owned `Line`s so the caller (`user::try_render`) just extends.
pub(crate) fn mention_summary_lines(
    source: &coco_messages::Message,
    styles: UiStyles<'_>,
) -> Option<Vec<Line<'static>>> {
    use coco_types::MentionItemKind;

    let coco_messages::Message::Attachment(att) = source else {
        return None;
    };
    let Some(coco_messages::AttachmentExtras::MentionSummary(payload)) = att.extras.as_ref() else {
        return None;
    };

    let lines = payload
        .items
        .iter()
        .map(|item| {
            let path = item.display_path.clone();
            let text = match item.kind {
                MentionItemKind::File => match item.count {
                    Some(n) => {
                        let count = if item.truncated {
                            format!("{n}+")
                        } else {
                            n.to_string()
                        };
                        t!("chat.mention_read_lines", path = path, count = count).to_string()
                    }
                    None => t!("chat.mention_read", path = path).to_string(),
                },
                MentionItemKind::AlreadyRead | MentionItemKind::Image => {
                    t!("chat.mention_read", path = path).to_string()
                }
                MentionItemKind::Pdf => match item.count {
                    Some(n) => t!(
                        "chat.mention_read_pages",
                        path = path,
                        count = n.to_string()
                    )
                    .to_string(),
                    None => t!("chat.mention_read", path = path).to_string(),
                },
                MentionItemKind::Directory => {
                    let p = if path.ends_with('/') {
                        path
                    } else {
                        format!("{path}/")
                    };
                    t!("chat.mention_listed_dir", path = p).to_string()
                }
            };
            Line::from(vec![
                Span::raw("  └ ").fg(styles.dim()),
                Span::raw(text).fg(styles.dim()),
            ])
        })
        .collect();
    Some(lines)
}

/// Path for a post-compact file-reference chip, or `None` for other
/// attachments. Renders `compact_file_reference` as `Referenced file
/// <displayPath>` while keeping the model-visible restore reminder out of the
/// chat surface.
pub(crate) fn compact_file_reference_chip_path(
    source: &coco_messages::Message,
    cwd: Option<&str>,
) -> Option<String> {
    let coco_messages::Message::Attachment(att) = source else {
        return None;
    };
    if att.kind != AttachmentKind::CompactFileReference {
        return None;
    }
    if let Some(coco_messages::AttachmentExtras::CompactFileReference(payload)) =
        att.extras.as_ref()
    {
        return Some(payload.display_path.clone());
    }

    compact_file_reference_path_from_legacy_body(&att.as_text_for_display(), cwd)
}

fn compact_file_reference_path_from_legacy_body(text: &str, cwd: Option<&str>) -> Option<String> {
    let body = strip_system_reminder_wrapper(text);
    let first = body.lines().map(str::trim).find(|line| !line.is_empty())?;
    let json = first
        .strip_prefix("Called the Read tool with the following input: ")
        .or_else(|| first.split_once(" input: ").map(|(_, rest)| rest))?;
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let path = value.get("file_path")?.as_str()?;
    Some(relativize_path(path, cwd))
}

/// Path for a memory-injection chip (nested CLAUDE.md / relevant memories), or
/// `None` for any other attachment. Detected by typed [`AttachmentKind`] — not
/// by sniffing the body — so the verbose `Contents of <path>:` reminder collapses
/// to a compact `◆ memory · <path>` row instead of dumping its first line.
pub(crate) fn nested_memory_chip_path(
    source: &coco_messages::Message,
    cwd: Option<&str>,
) -> Option<String> {
    let coco_messages::Message::Attachment(att) = source else {
        return None;
    };
    if !matches!(
        att.kind,
        AttachmentKind::NestedMemory | AttachmentKind::RelevantMemories
    ) {
        return None;
    }
    let body = strip_system_reminder_wrapper(&att.as_text_for_display());
    let first = body.lines().map(str::trim).find(|line| !line.is_empty())?;
    // NestedMemory header is `Contents of {path}:`; RelevantMemories is
    // `Memory: {path} (last modified …)`. Strip down to just `{path}`.
    let path = first
        .strip_prefix("Contents of ")
        .map(|rest| rest.strip_suffix(':').unwrap_or(rest))
        .or_else(|| {
            first
                .strip_prefix("Memory: ")
                .map(|rest| rest.split(" (").next().unwrap_or(rest))
        })
        .unwrap_or(first);
    Some(relativize_path(path, cwd))
}

/// Display form of a path: relative to `cwd` when it lives under the working
/// directory, else the absolute path unchanged (e.g. `~/.coco/CLAUDE.md`).
pub(crate) fn relativize_path(path: &str, cwd: Option<&str>) -> String {
    if let Some(cwd) = cwd.filter(|c| !c.is_empty())
        && let Some(rest) = path
            .strip_prefix(cwd.trim_end_matches('/'))
            .and_then(|rest| rest.strip_prefix('/'))
        && !rest.is_empty()
    {
        return rest.to_string();
    }
    path.to_string()
}

/// Strip the `<system-reminder>` wrapper lines from an attachment body so the
/// content row shows the meaningful text, not the XML tag.
fn strip_system_reminder_wrapper(text: &str) -> String {
    text.lines()
        .filter(|line| {
            let t = line.trim();
            t != "<system-reminder>" && t != "</system-reminder>"
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// Best-effort short text for the meta-preview row. Walks
/// `cell.source` for the human-readable payload of each
/// `SystemMessage` sub-variant.
fn meta_preview_text(cell: &RenderedCell) -> String {
    use coco_messages::Message;
    use coco_messages::SystemMessage as SM;
    // Only System cells collapse to a meta preview now — attachments render as
    // content rows (see `presentation::transcript::is_meta`).
    let Message::System(sm) = cell.source.as_ref() else {
        return String::new();
    };
    match sm {
        SM::Informational(info) => {
            if info.title.is_empty() {
                info.message.clone()
            } else {
                format!("{}: {}", info.title, info.message)
            }
        }
        SM::ApiError(e) => e.error.clone(),
        SM::CompactBoundary(_) => String::new(),
        SM::PermissionRetry(m) => format!("{} · {}", m.tool_name, m.message),
        SM::BridgeStatus(m) => m.message.clone().unwrap_or_default(),
        SM::LocalCommand(lc) => lc.command.clone(),
        SM::UserInterruption(_)
        | SM::MicrocompactBoundary(_)
        | SM::MemorySaved(_)
        | SM::AwaySummary(_)
        | SM::AgentsKilled(_)
        | SM::ApiMetrics(_)
        | SM::StopHookSummary(_)
        | SM::TurnDuration(_)
        | SM::ScheduledTaskFire(_)
        | SM::ContextUsage(_) => String::new(),
    }
}

pub(super) fn result_line(text: String, color: ratatui::style::Color) -> Line<'static> {
    Line::from(vec![Span::raw("  └ ").fg(color), Span::raw(text).fg(color)])
}

/// Synthesize the live `● Tool(args) (Ns)` progress rows for mid-stream tools
/// that have no committed `CellKind::ToolUse` header yet (filtered by
/// [`is_uncommitted_in_flight`]). Shared by the chat renderer and the Ctrl+O
/// transcript reader so both surfaces format in-flight tools identically — and
/// identically to the committed [`CellsRenderer::render_tool_call_header`], so
/// the row doesn't visually jump when the owning message commits. Reads
/// `elapsed()` live, so the timer ticks on each frame.
pub(crate) fn in_flight_tool_lines(
    tools: &[ToolExecution],
    styles: UiStyles<'_>,
) -> Vec<Line<'static>> {
    tools
        .iter()
        .filter(|t| crate::presentation::transcript::is_tool_in_flight(t))
        // Agent (subagent) spawns render in the dedicated Agents panel
        // (`agent_surface`); suppressing them here avoids duplicating the
        // same work as both an inline `Agent(...)` row and a panel row
        // (the two surfaces are independently keyed and drift out of sync).
        .filter(|t| t.name != coco_types::ToolName::Agent.as_str())
        .map(|tool| {
            let tone = tool_tone_color(tool_name_tone(&tool.name), styles);
            let mut spans = vec![
                Span::raw("● ").fg(tone),
                Span::raw(tool.name.clone()).fg(tone).bold(),
            ];
            if let Some(preview) = tool
                .input_preview
                .as_deref()
                .or(tool.description.as_deref())
                .filter(|preview| !preview.is_empty())
            {
                spans.push(Span::raw("(").fg(styles.text()));
                spans.push(Span::raw(truncate_chars(preview, 96)).fg(styles.dim()));
                spans.push(Span::raw(")").fg(styles.text()));
            }
            spans.push(
                Span::raw(format!(" ({})", format_duration_seconds(tool.elapsed())))
                    .fg(styles.dim())
                    .dim(),
            );
            Line::from(spans)
        })
        .collect()
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

pub(super) fn output_result_line(
    text: String,
    color: ratatui::style::Color,
    first: bool,
) -> Line<'static> {
    let prefix = if first { "  └ " } else { "    " };
    Line::from(vec![Span::raw(prefix).fg(color), Span::raw(text).fg(color)])
}

pub(super) fn single_line_capped(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut count = 0usize;
    let mut first = true;
    let mut truncated = false;

    'words: for word in text.split_whitespace() {
        if !first {
            if count + 1 >= max_chars {
                truncated = true;
                break;
            }
            out.push(' ');
            count += 1;
        }
        first = false;
        for ch in word.chars() {
            if count + 1 >= max_chars {
                truncated = true;
                break 'words;
            }
            out.push(ch);
            count += 1;
        }
    }

    if truncated {
        out.push('…');
    }
    out
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out = text.chars().take(max.saturating_sub(1)).collect::<String>();
    out.push('…');
    out
}

pub(super) fn transcript_safe_line(line: &str) -> String {
    truncate_chars(line, TRANSCRIPT_LINE_CHAR_CAP)
}

/// Compact attribution label for the slow-cell perf log: which cell kind (and
/// for tool calls, which tool) a slow render belongs to.
fn cell_perf_label(cells: &[RenderedCell], cell: &TranscriptSourceCell<'_>) -> String {
    match cell {
        TranscriptSourceCell::Committed(TranscriptCell::MetaPreview { .. }) => {
            "meta_preview".to_string()
        }
        TranscriptSourceCell::Committed(TranscriptCell::Cell { index }) => {
            cells.get(*index).map_or_else(
                || "cell:missing".to_string(),
                |c| format!("cell:{}", cell_kind_perf_name(&c.kind)),
            )
        }
        TranscriptSourceCell::Committed(TranscriptCell::ToolCall { invocation, .. }) => {
            match invocation
                .and_then(|index| cells.get(index))
                .map(|c| &c.kind)
            {
                Some(CellKind::ToolUse { tool_name, .. }) => format!("tool_call:{tool_name}"),
                Some(_) | None => "tool_call:unknown".to_string(),
            }
        }
        TranscriptSourceCell::Committed(TranscriptCell::ToolBatch { .. }) => {
            "tool_batch".to_string()
        }
        TranscriptSourceCell::Active(ActiveTranscriptCell::Streaming(_)) => {
            "streaming_tail".to_string()
        }
        TranscriptSourceCell::Active(ActiveTranscriptCell::InFlightTools) => {
            "in_flight_tools".to_string()
        }
    }
}

fn cell_kind_perf_name(kind: &CellKind) -> &'static str {
    match kind {
        CellKind::UserText { .. } => "user_text",

        CellKind::AssistantText { .. } => "assistant_text",
        CellKind::AssistantThinking { .. } => "assistant_thinking",
        CellKind::AssistantRedactedThinking => "assistant_redacted_thinking",
        CellKind::ToolUse { .. } => "tool_use",
        CellKind::ToolResult { .. } => "tool_result",
        CellKind::Attachment => "attachment",

        CellKind::System(_) => "system",
    }
}

#[cfg(test)]
#[path = "cells_renderer.test.rs"]
mod tests;
