//! Rich, per-tool rendering of a tool call's RESULT body.
//!
//! Replaces the one-size-fits-all 5-line raw-text preview with renderers keyed
//! to what each tool actually produces. The dispatch is an **exhaustive match
//! over `coco_types::ToolName`** so every built-in tool is deliberately routed
//! (the compiler rejects a forgotten variant); MCP / custom tools whose wire
//! name doesn't parse to a `ToolName` fall to the structured default.
//!
//! Rich categories reuse existing primitives — the `diff_display` widget for
//! edits, the markdown highlighter for file content, and `similar` to synthesize
//! a unified diff from an Edit's `old_string`/`new_string`. The "default" path
//! still beats the old behaviour: it pretty-prints JSON instead of dumping it on
//! one line.
//!
//! The renderer is **surface-agnostic**: it takes a [`ToolResultRenderCtx`]
//! (styles + width + the syntax toggle + a truncation hint) rather than a
//! `CellsRenderer`, so the inline chat AND the Ctrl+O transcript reader render tool
//! results identically. The reader sets `expanded: true`, relaxing the inline
//! row caps so a diff truncated inline ("… +N lines (ctrl+o to expand)") is shown
//! in full once expanded — the promise the inline hint makes.
//!
//! `input` is the tool's raw invocation arguments when the caller has the
//! issuing assistant message on hand (the paired path), else `None`; input-derived
//! views (diffs, highlighted file content, the web target) degrade gracefully to
//! output-only.

use std::collections::VecDeque;
use std::path::Path;

use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::text::Text;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Wrap;
use serde_json::Value;
use unicode_width::UnicodeWidthChar;
use unicode_width::UnicodeWidthStr;

use super::TOOL_OUTPUT_PREVIEW_ROWS;
use super::output_result_line;
use super::result_line;
use super::single_line_capped;
use super::transcript_safe_line;
use crate::i18n::t;
use crate::presentation::transcript::TRANSCRIPT_EXPANDED_CELL_LINE_CAP;
use coco_tui_ui::display::SyntaxHighlighting;
use coco_tui_ui::style::UiStyles;
use coco_types::ApplyPatchPreview;
use coco_types::ApplyPatchPreviewRow;
use coco_types::ExitPlanModeResult;
use coco_types::ToolDisplayData;
use coco_types::ToolName;

/// Tool-input field keys, named once so the per-tool renderers don't scatter
/// magic strings that silently drift from the typed inputs in `coco-tools`.
mod field {
    pub(super) const OLD_STRING: &str = "old_string";
    pub(super) const NEW_STRING: &str = "new_string";
    pub(super) const FILE_PATH: &str = "file_path";
    pub(super) const CONTENT: &str = "content";
    pub(super) const NEW_SOURCE: &str = "new_source";
    pub(super) const TODOS: &str = "todos";
    pub(super) const TODO_STATUS: &str = "status";
    pub(super) const TODO_CONTENT: &str = "content";
    pub(super) const TODO_ACTIVE_FORM: &str = "activeForm";
    pub(super) const URL: &str = "url";
    pub(super) const QUERY: &str = "query";
}

/// Inline row caps before truncation (full body shown when `ctx.expanded`).
const DIFF_PREVIEW_ROWS: usize = 24;
const CODE_PREVIEW_ROWS: usize = 6;
const STRUCTURED_PREVIEW_ROWS: usize = 14;
const PLAIN_TOOL_PREVIEW_ROWS: usize = TOOL_OUTPUT_PREVIEW_ROWS;
/// Single-line header cap (matches the invocation header's preview width).
const HEADER_CAP: usize = 96;
const TAB_WIDTH: usize = 4;

/// Everything the per-tool renderers need from their host surface. Decouples the
/// renderer from `CellsRenderer` so the inline chat and the transcript reader share
/// one implementation.
pub(crate) struct ToolResultRenderCtx<'a> {
    pub(crate) styles: UiStyles<'a>,
    pub(crate) width: u16,
    pub(crate) syntax_highlighting: SyntaxHighlighting,
    /// Shortcut hint for reopening the saved plan in the editor.
    pub(crate) plan_editor_hint: String,
    /// Appended to a "… +N lines" truncation row (e.g. `(ctrl+o to expand)`).
    /// Empty when the surface is itself the full-detail view (the reader).
    pub(crate) expand_hint: String,
    /// Full-detail surface (the reader): relax row caps so nothing re-truncates.
    pub(crate) expanded: bool,
}

impl ToolResultRenderCtx<'_> {
    /// Row budget for a section. The full-detail surface (the reader) uses the
    /// single per-cell cap so an inline-truncated body resolves to its full form
    /// once expanded — bounded, never the whole transcript (I-/cap invariant).
    fn rows(&self, base: usize) -> usize {
        if self.expanded {
            TRANSCRIPT_EXPANDED_CELL_LINE_CAP
        } else {
            base
        }
    }

    /// Truncation marker — a plain indented continuation row (no `└` gutter, which
    /// reads as a new block), eliding the hint when the surface has none.
    fn truncation_line(&self, omitted: usize) -> Line<'static> {
        output_result_line(
            self.truncation_text(omitted),
            self.styles.dim(),
            /*first*/ false,
        )
    }

    fn truncation_text(&self, omitted: usize) -> String {
        if self.expand_hint.is_empty() {
            format!("… +{omitted} lines")
        } else {
            format!("… +{omitted} lines {}", self.expand_hint)
        }
    }
}

/// Render the result body for a completed tool call.
pub(crate) fn render_tool_result_body(
    cx: &ToolResultRenderCtx<'_>,
    tool_name: &str,
    input: Option<&Value>,
    output: &str,
    display_data: Option<&ToolDisplayData>,
    is_error: bool,
    lines: &mut Vec<Line<'static>>,
) {
    if is_error {
        if matches!(tool_name.parse::<ToolName>(), Ok(ToolName::ApplyPatch))
            && let Some(preview) = apply_patch_preview(display_data)
        {
            lines.extend(render_capped_apply_patch_preview(
                cx,
                preview,
                cx.rows(DIFF_PREVIEW_ROWS),
            ));
        }
        // Errors are uniform across tools: the message matters, not the shape.
        push_text_preview(
            cx,
            output,
            cx.rows(PLAIN_TOOL_PREVIEW_ROWS),
            lines,
            cx.styles.error(),
        );
        return;
    }
    match tool_name.parse::<ToolName>() {
        Ok(name) => render_known(cx, name, input, output, display_data, lines),
        // MCP (`mcp__server__tool`) / plugin / custom names don't parse.
        Err(_) => render_structured_default(cx, output, lines),
    }
}

/// Exhaustive over `ToolName` — adding a variant forces a rendering decision.
fn render_known(
    cx: &ToolResultRenderCtx<'_>,
    name: ToolName,
    input: Option<&Value>,
    output: &str,
    display_data: Option<&ToolDisplayData>,
    lines: &mut Vec<Line<'static>>,
) {
    use ToolName::*;
    match name {
        // ── Edits → colored unified diff ───────────────────────────────
        Edit => render_edit_diff(cx, input, output, lines),
        ApplyPatch => render_apply_patch(cx, input, output, display_data, lines),
        // ── File content → syntax-highlighted code ─────────────────────
        Read => render_read(cx, input, output, lines),
        Write => render_write(cx, input, output, lines),
        NotebookEdit => render_notebook_edit(cx, input, output, lines),
        // ── Shell → output (the `●` header already names the command) ─
        Bash | PowerShell | Repl => render_text(cx, output, PLAIN_TOOL_PREVIEW_ROWS, lines),
        // ── Search → match list ────────────────────────────────────────
        Grep | Glob => render_text(cx, output, PLAIN_TOOL_PREVIEW_ROWS, lines),
        // ── Checklist ──────────────────────────────────────────────────
        TodoWrite => render_todos(cx, input, output, lines),
        // ── Web → target header + output ───────────────────────────────
        WebFetch | WebSearch => render_web(cx, input, output, lines),
        // ── AskUserQuestion → styled answered-questions cell ───────────
        AskUserQuestion => render_ask_user_question(cx, display_data, output, lines),
        // ── Everything else → structured default (pretty JSON / text) ──
        ExitPlanMode => render_exit_plan_mode(cx, display_data, output, lines),
        Agent | Skill | SendMessage | TeamCreate | TeamDelete | TaskCreate | TaskGet | TaskList
        | TaskUpdate | TaskStop | TaskOutput | EnterPlanMode | VerifyPlanExecution
        | EnterWorktree | ExitWorktree | ToolSearch | Config | SendUserMessage | Lsp | McpAuth
        | ListMcpResources | ReadMcpResource | CronCreate | CronDelete | CronList
        | RemoteTrigger | Sleep | StructuredOutput => render_structured_default(cx, output, lines),
    }
}

// ── Edits ──────────────────────────────────────────────────────────────

fn render_edit_diff(
    cx: &ToolResultRenderCtx<'_>,
    input: Option<&Value>,
    output: &str,
    lines: &mut Vec<Line<'static>>,
) {
    if let Some(input) = input {
        let old = str_field(input, field::OLD_STRING).unwrap_or_default();
        let new = str_field(input, field::NEW_STRING).unwrap_or_default();
        if old != new {
            let diff = unified_diff_text(&old, &new);
            let width = cx.width.saturating_sub(2);
            let rendered = coco_tui_ui::widgets::diff_display::render_diff_preview_lines(
                &diff,
                cx.styles,
                width,
                cx.rows(DIFF_PREVIEW_ROWS),
                |omitted| Line::from(Span::raw(cx.truncation_text(omitted)).fg(cx.styles.dim())),
            );
            lines.extend(indent2(rendered));
            return;
        }
    }
    // No input (collapsed/standalone path) — show what the tool reported.
    render_output_preview(cx, output, lines);
}

fn render_apply_patch(
    cx: &ToolResultRenderCtx<'_>,
    _input: Option<&Value>,
    output: &str,
    display_data: Option<&ToolDisplayData>,
    lines: &mut Vec<Line<'static>>,
) {
    if let Some(preview) = apply_patch_preview(display_data) {
        lines.extend(render_capped_apply_patch_preview(
            cx,
            preview,
            cx.rows(DIFF_PREVIEW_ROWS),
        ));
        return;
    }

    render_output_preview(cx, output, lines);
}

fn push_apply_patch_signed_row(
    cx: &ToolResultRenderCtx<'_>,
    sign: char,
    content: &str,
    lines: &mut Vec<Line<'static>>,
) {
    let color = match sign {
        '+' => cx.styles.diff_added(),
        '-' => cx.styles.diff_removed(),
        _ => cx.styles.dim(),
    };
    let content = transcript_safe_line(content);
    let prefix_cols = 5usize;
    let content_width = (cx.width as usize).saturating_sub(prefix_cols).max(1);
    let chunks = wrap_plain_text(&content, content_width);

    for (index, chunk) in chunks.into_iter().enumerate() {
        let mut spans = if index == 0 {
            vec![
                Span::raw("    ").fg(cx.styles.dim()),
                Span::raw(sign.to_string()).fg(color),
            ]
        } else {
            vec![Span::raw(" ".repeat(prefix_cols)).fg(cx.styles.dim())]
        };
        spans.push(Span::raw(chunk).fg(color));
        lines.push(Line::from(spans));
    }
}

fn push_apply_patch_raw_row(
    cx: &ToolResultRenderCtx<'_>,
    content: &str,
    lines: &mut Vec<Line<'static>>,
) {
    push_wrapped_prefixed_row(cx, "    ".to_string(), content, cx.styles.dim(), lines);
}

fn render_apply_patch_preview(
    cx: &ToolResultRenderCtx<'_>,
    preview: &ApplyPatchPreview,
) -> Vec<Line<'static>> {
    let mut rendered = Vec::new();
    for row in &preview.rows {
        push_apply_patch_preview_row(cx, row, &mut rendered);
    }
    rendered
}

fn render_capped_apply_patch_preview(
    cx: &ToolResultRenderCtx<'_>,
    preview: &ApplyPatchPreview,
    max_rows: usize,
) -> Vec<Line<'static>> {
    let rows = cap_apply_patch_preview_rows(cx, &preview.rows, max_rows);
    let capped = ApplyPatchPreview { rows };
    render_apply_patch_preview(cx, &capped)
}

/// Styled transcript cell for a completed AskUserQuestion exchange, mirroring
/// codex `RequestUserInputResultCell` instead of dumping the model-facing prose.
/// Falls back to the prose when no structured answers were spliced (declined /
/// test fixtures).
fn render_ask_user_question(
    cx: &ToolResultRenderCtx<'_>,
    display_data: Option<&ToolDisplayData>,
    output: &str,
    lines: &mut Vec<Line<'static>>,
) {
    let Some(ToolDisplayData::AskUserQuestionResult(result)) = display_data else {
        render_structured_default(cx, output, lines);
        return;
    };
    let total = result.questions.len();
    let answered = result
        .questions
        .iter()
        .filter(|q| !q.answers.is_empty() || q.note.is_some())
        .count();
    // Header: "Questions N/total answered".
    lines.push(Line::from(vec![
        Span::raw("    ").fg(cx.styles.dim()),
        Span::raw("Questions ").fg(cx.styles.text()).bold(),
        Span::raw(format!("{answered}/{total} answered")).fg(cx.styles.dim()),
    ]));
    for q in &result.questions {
        let unanswered = q.answers.is_empty() && q.note.is_none();
        let question = if unanswered {
            format!("{} (unanswered)", q.question)
        } else {
            q.question.clone()
        };
        push_wrapped_prefixed_row(cx, "    • ".to_string(), &question, cx.styles.text(), lines);
        for answer in &q.answers {
            push_wrapped_prefixed_row(
                cx,
                "      answer: ".to_string(),
                answer,
                cx.styles.accent(),
                lines,
            );
        }
        if let Some(note) = &q.note {
            // codex labels a note on an option-question "note:", but a bare
            // free-text answer "answer:".
            let label = if q.answers.is_empty() {
                "      answer: "
            } else {
                "      note: "
            };
            push_wrapped_prefixed_row(cx, label.to_string(), note, cx.styles.accent(), lines);
        }
    }
}

fn render_exit_plan_mode(
    cx: &ToolResultRenderCtx<'_>,
    display_data: Option<&ToolDisplayData>,
    output: &str,
    lines: &mut Vec<Line<'static>>,
) {
    let Some(ToolDisplayData::ExitPlanModeResult(result)) = display_data else {
        render_structured_default(cx, output, lines);
        return;
    };

    if result.awaiting_leader_approval {
        lines.push(output_result_line(
            "Plan submitted for team lead approval".to_string(),
            cx.styles.plan(),
            /*first*/ true,
        ));
        if let Some(path) = result.file_path.as_ref().filter(|p| !p.trim().is_empty()) {
            push_exit_plan_file_path(cx, path, None, lines);
        }
        push_exit_plan_preview(cx, result, lines);
        return;
    }

    if !result.has_implementation_plan() {
        lines.push(output_result_line(
            t!("chat.plan_exited").to_string(),
            cx.styles.plan(),
            /*first*/ true,
        ));
        return;
    }

    lines.push(output_result_line(
        t!("chat.plan_approved").to_string(),
        cx.styles.plan(),
        /*first*/ true,
    ));
    if let Some(path) = result.file_path.as_ref().filter(|p| !p.trim().is_empty()) {
        push_exit_plan_file_path(cx, path, Some(cx.plan_editor_hint.as_str()), lines);
    }
    push_exit_plan_preview(cx, result, lines);
}

fn push_exit_plan_file_path(
    cx: &ToolResultRenderCtx<'_>,
    path: &str,
    edit_hint: Option<&str>,
    lines: &mut Vec<Line<'static>>,
) {
    let label = match edit_hint {
        Some(hint) if !hint.trim().is_empty() => {
            format!("{} · {hint}", t!("chat.plan_file", path = path))
        }
        _ => t!("chat.plan_file", path = path).to_string(),
    };
    push_wrapped_prefixed_row(cx, "    ".to_string(), &label, cx.styles.dim(), lines);
}

/// Render the approved plan as full markdown, indented under the result header.
///
/// Mirrors claude-code `ExitPlanModeTool/UI.tsx` (`<Markdown>{plan}</Markdown>`
/// inside `MessageResponse`). Unlike every other tool preview this is **not**
/// row-capped: the approved plan is the durable record the user scrolls back to
/// to re-read what was agreed — the ephemeral approval panel is what truncates.
fn push_exit_plan_preview(
    cx: &ToolResultRenderCtx<'_>,
    result: &ExitPlanModeResult,
    lines: &mut Vec<Line<'static>>,
) {
    let plan = result.plan.trim();
    if plan.is_empty() {
        return;
    }
    lines.extend(crate::presentation::plan::render_plan_markdown(
        plan,
        cx.styles,
        cx.width,
        cx.syntax_highlighting,
    ));
}

fn apply_patch_preview(display_data: Option<&ToolDisplayData>) -> Option<&ApplyPatchPreview> {
    match display_data {
        Some(ToolDisplayData::ApplyPatchPreview(preview)) if !preview.rows.is_empty() => {
            Some(preview)
        }
        _ => None,
    }
}

fn push_apply_patch_preview_row(
    cx: &ToolResultRenderCtx<'_>,
    row: &ApplyPatchPreviewRow,
    lines: &mut Vec<Line<'static>>,
) {
    match row {
        ApplyPatchPreviewRow::Header { action, target } => {
            lines.push(output_result_line(
                single_line_capped(&format!("{} {target}", action.as_str()), HEADER_CAP),
                cx.styles.secondary(),
                lines.is_empty(),
            ));
        }
        ApplyPatchPreviewRow::Line { sign, content } => {
            push_apply_patch_signed_row(cx, sign.as_char(), content, lines);
        }
        ApplyPatchPreviewRow::Raw { content } => {
            push_apply_patch_raw_row(cx, content, lines);
        }
        ApplyPatchPreviewRow::Omitted { rows } => {
            lines.push(output_result_line(
                cx.truncation_text(omitted_rows_to_usize(*rows)),
                cx.styles.dim(),
                lines.is_empty(),
            ));
        }
    }
}

fn cap_apply_patch_preview_rows(
    cx: &ToolResultRenderCtx<'_>,
    rows: &[ApplyPatchPreviewRow],
    max_rows: usize,
) -> Vec<ApplyPatchPreviewRow> {
    if max_rows == 0 || rows.is_empty() {
        return Vec::new();
    }

    let row_counts: Vec<usize> = rows
        .iter()
        .map(|row| {
            let mut rendered = Vec::new();
            push_apply_patch_preview_row(cx, row, &mut rendered);
            rendered_line_count(cx, &rendered)
        })
        .collect();
    let total_rows: usize = row_counts.iter().sum();
    if total_rows <= max_rows {
        return merge_adjacent_omitted_rows(rows.to_vec());
    }

    let all_omitted = sum_apply_patch_logical_rows(rows);
    let ellipsis_rows = rendered_line_count(
        cx,
        &[output_result_line(
            cx.truncation_text(omitted_rows_to_usize(all_omitted)),
            cx.styles.dim(),
            true,
        )],
    );
    if ellipsis_rows >= max_rows {
        return vec![ApplyPatchPreviewRow::Omitted { rows: all_omitted }];
    }

    let available_rows = max_rows - ellipsis_rows;
    let head_budget = available_rows / 2;
    let tail_budget = available_rows - head_budget;

    let mut head = Vec::new();
    let mut head_rows = 0usize;
    let mut head_end = 0usize;
    while head_end < rows.len() {
        let row_count = row_counts[head_end];
        if head_rows + row_count > head_budget {
            break;
        }
        head_rows += row_count;
        head.push(rows[head_end].clone());
        head_end += 1;
    }

    let mut tail_reversed = Vec::new();
    let mut tail_rows = 0usize;
    let mut tail_start = rows.len();
    while tail_start > head_end {
        let idx = tail_start - 1;
        let row_count = row_counts[idx];
        if tail_rows + row_count > tail_budget {
            break;
        }
        tail_rows += row_count;
        tail_reversed.push(rows[idx].clone());
        tail_start -= 1;
    }

    let omitted = sum_apply_patch_logical_rows(&rows[head_end..tail_start]);
    if omitted > 0 {
        head.push(ApplyPatchPreviewRow::Omitted { rows: omitted });
    }
    head.extend(tail_reversed.into_iter().rev());
    merge_adjacent_omitted_rows(head)
}

fn rendered_line_count(cx: &ToolResultRenderCtx<'_>, lines: &[Line<'static>]) -> usize {
    if lines.is_empty() {
        return 0;
    }
    let width = cx.width.max(1);
    lines
        .iter()
        .map(|line| {
            Paragraph::new(Text::from(vec![line.clone()]))
                .wrap(Wrap { trim: false })
                .line_count(width)
                .max(1)
        })
        .sum()
}

fn sum_apply_patch_logical_rows(rows: &[ApplyPatchPreviewRow]) -> i64 {
    rows.iter().fold(0i64, |total, row| {
        total.saturating_add(match row {
            ApplyPatchPreviewRow::Omitted { rows } => (*rows).max(0),
            ApplyPatchPreviewRow::Header { .. }
            | ApplyPatchPreviewRow::Line { .. }
            | ApplyPatchPreviewRow::Raw { .. } => 1,
        })
    })
}

fn merge_adjacent_omitted_rows(rows: Vec<ApplyPatchPreviewRow>) -> Vec<ApplyPatchPreviewRow> {
    let mut merged: Vec<ApplyPatchPreviewRow> = Vec::with_capacity(rows.len());
    for row in rows {
        match (merged.last_mut(), row) {
            (
                Some(ApplyPatchPreviewRow::Omitted { rows: existing }),
                ApplyPatchPreviewRow::Omitted { rows },
            ) => {
                *existing = existing.saturating_add(rows.max(0));
            }
            (_, ApplyPatchPreviewRow::Omitted { rows }) if rows <= 0 => {}
            (_, row) => merged.push(row),
        }
    }
    merged
}

fn omitted_rows_to_usize(rows: i64) -> usize {
    match usize::try_from(rows.max(0)) {
        Ok(rows) => rows,
        Err(_) => usize::MAX,
    }
}

fn push_wrapped_prefixed_row(
    cx: &ToolResultRenderCtx<'_>,
    prefix: String,
    content: &str,
    color: ratatui::style::Color,
    lines: &mut Vec<Line<'static>>,
) {
    let content = transcript_safe_line(content);
    let prefix_cols = display_width(&prefix);
    let content_width = (cx.width as usize).saturating_sub(prefix_cols).max(1);
    let chunks = wrap_plain_text(&content, content_width);
    let continuation = " ".repeat(prefix_cols);

    for (index, chunk) in chunks.into_iter().enumerate() {
        let row_prefix = if index == 0 {
            prefix.clone()
        } else {
            continuation.clone()
        };
        lines.push(Line::from(vec![
            Span::raw(row_prefix).fg(cx.styles.dim()),
            Span::raw(chunk).fg(color),
        ]));
    }
}

// ── File content ─────────────────────────────────────────────────────────

fn render_read(
    cx: &ToolResultRenderCtx<'_>,
    input: Option<&Value>,
    output: &str,
    lines: &mut Vec<Line<'static>>,
) {
    let ext = input
        .and_then(|i| str_field(i, field::FILE_PATH))
        .map(|p| file_ext(&p))
        .unwrap_or_default();
    // Read output is `cat -n` (`<n>\t<content>`): split the line number into a
    // dim gutter so it can't jam against the content and so the highlighter
    // sees real source rather than a number-prefixed line.
    render_highlighted_rows(cx, &ext, output, Gutter::LineNumbers, lines);
}

fn render_write(
    cx: &ToolResultRenderCtx<'_>,
    input: Option<&Value>,
    output: &str,
    lines: &mut Vec<Line<'static>>,
) {
    let Some(input) = input else {
        render_output_preview(cx, output, lines);
        return;
    };
    let path = str_field(input, field::FILE_PATH).unwrap_or_default();
    let Some(content) = str_field(input, field::CONTENT) else {
        render_output_preview(cx, output, lines);
        return;
    };
    let header = (!path.is_empty()).then(|| format!("wrote {path}"));
    render_code(cx, &file_ext(&path), &content, header, lines);
}

fn render_notebook_edit(
    cx: &ToolResultRenderCtx<'_>,
    input: Option<&Value>,
    output: &str,
    lines: &mut Vec<Line<'static>>,
) {
    // Cell replacement has no `old_string` to diff against; show the new source
    // as code (notebooks are Python unless told otherwise).
    if let Some(src) = input.and_then(|i| str_field(i, field::NEW_SOURCE)) {
        render_code(cx, "python", &src, None, lines);
        return;
    }
    render_structured_default(cx, output, lines);
}

// ── Checklist ──────────────────────────────────────────────────────────

fn render_todos(
    cx: &ToolResultRenderCtx<'_>,
    input: Option<&Value>,
    output: &str,
    lines: &mut Vec<Line<'static>>,
) {
    let Some(todos) = input
        .and_then(|i| i.get(field::TODOS))
        .and_then(Value::as_array)
        .filter(|t| !t.is_empty())
    else {
        render_output_preview(cx, output, lines);
        return;
    };
    for (i, todo) in todos.iter().enumerate() {
        let status = todo
            .get(field::TODO_STATUS)
            .and_then(Value::as_str)
            .unwrap_or("pending");
        let content = todo
            .get(field::TODO_CONTENT)
            .and_then(Value::as_str)
            .or_else(|| todo.get(field::TODO_ACTIVE_FORM).and_then(Value::as_str))
            .unwrap_or_default();
        let (glyph, color) = match status {
            "completed" => ("✔", cx.styles.success()),
            "in_progress" => ("◐", cx.styles.warning()),
            _ => ("☐", cx.styles.dim()),
        };
        lines.push(output_result_line(
            format!("{glyph} {}", transcript_safe_line(content)),
            color,
            i == 0,
        ));
    }
}

// ── Web ──────────────────────────────────────────────────────────────────

fn render_web(
    cx: &ToolResultRenderCtx<'_>,
    input: Option<&Value>,
    output: &str,
    lines: &mut Vec<Line<'static>>,
) {
    if let Some(target) =
        input.and_then(|i| str_field(i, field::URL).or_else(|| str_field(i, field::QUERY)))
    {
        lines.push(result_line(
            single_line_capped(&target, HEADER_CAP),
            cx.styles.secondary(),
        ));
    }
    push_text_preview(
        cx,
        output,
        cx.rows(STRUCTURED_PREVIEW_ROWS),
        lines,
        cx.styles.text(),
    );
}

// ── Structured default (everything else + MCP/custom) ─────────────────────

fn render_structured_default(
    cx: &ToolResultRenderCtx<'_>,
    output: &str,
    lines: &mut Vec<Line<'static>>,
) {
    let trimmed = output.trim();
    if (trimmed.starts_with('{') || trimmed.starts_with('['))
        && let Ok(value) = serde_json::from_str::<Value>(trimmed)
        && (value.is_object() || value.is_array())
        && let Ok(pretty) = serde_json::to_string_pretty(&value)
    {
        push_text_preview(
            cx,
            &pretty,
            cx.rows(STRUCTURED_PREVIEW_ROWS),
            lines,
            cx.styles.text(),
        );
        return;
    }
    render_output_preview(cx, output, lines);
}

// ── Shared helpers ─────────────────────────────────────────────────────────

/// Plain-text body at a fixed base row budget (scaled on the reader surface).
fn render_text(
    cx: &ToolResultRenderCtx<'_>,
    output: &str,
    base_rows: usize,
    lines: &mut Vec<Line<'static>>,
) {
    push_text_preview(cx, output, cx.rows(base_rows), lines, cx.styles.text());
}

/// Optional left gutter for [`render_highlighted_rows`].
#[derive(Clone, Copy)]
enum Gutter {
    /// Plain code body (Write / NotebookEdit content) — no gutter.
    None,
    /// `cat -n` line numbers (Read output) split into a dim, right-aligned
    /// gutter; the number is stripped before highlighting.
    LineNumbers,
}

/// Render `content` as syntax-highlighted code under a tool header.
///
/// Highlights via [`coco_tui_markdown::highlight_code_lines`] directly rather
/// than wrapping the content in a Markdown fence: a fence both
/// breaks on a `` ``` `` *inside* the content (e.g. reading a Markdown file) and
/// draws a code-block border that doesn't belong under a tool header.
fn render_code(
    cx: &ToolResultRenderCtx<'_>,
    lang: &str,
    content: &str,
    header: Option<String>,
    lines: &mut Vec<Line<'static>>,
) {
    if let Some(header) = header {
        lines.push(result_line(header, cx.styles.dim()));
    }
    render_highlighted_rows(cx, lang, content, Gutter::None, lines);
}

/// Highlight + render `content` one logical line at a time, with an optional
/// `cat -n` line-number gutter.
///
/// Only the first `rows` logical lines are highlighted — truncation happens
/// *before* the expensive work — and then the rendered block is capped by
/// wrapped screen rows so a few very long lines cannot flood the viewport. When
/// `gutter` is [`Gutter::LineNumbers`] each line's leading `<n>\t` is split off
/// into a dim, right-aligned gutter so (a) the number never jams against the
/// content and (b) the highlighter parses real source instead of `12\t…`.
fn render_highlighted_rows(
    cx: &ToolResultRenderCtx<'_>,
    lang: &str,
    content: &str,
    gutter: Gutter,
    lines: &mut Vec<Line<'static>>,
) {
    if content.trim().is_empty() {
        lines.push(result_line("(empty)".to_string(), cx.styles.dim()));
        return;
    }
    let rows = cx.rows(CODE_PREVIEW_ROWS);
    let mut body = content.lines();
    // Each row is `(line_number, content)`; the number is empty for the
    // no-gutter path and for any line lacking a `<digits>\t` prefix.
    let mut visible: Vec<(String, String)> = body
        .by_ref()
        .take(rows)
        .map(|line| match gutter {
            Gutter::LineNumbers => {
                let (num, rest) = split_line_number(line);
                (num, transcript_safe_line(rest))
            }
            Gutter::None => (String::new(), transcript_safe_line(line)),
        })
        .collect();
    // `count()` consumes the lazy remainder without materializing it.
    let mut omitted = body.count();
    // Reserve one row for the "… +N lines" marker so the preview is exactly
    // `rows` logical lines (head + marker). Without this the marker pushes the
    // block to `rows + 1` lines and `truncate_lines_middle` re-elides it into a
    // second, *middle* ellipsis whose count double-reports the omission — a
    // marker that visibly disagrees with the line-number gutter.
    if omitted > 0 {
        visible.pop();
        omitted += 1;
    }
    let gutter_width = visible
        .iter()
        .map(|(num, _)| num.chars().count())
        .max()
        .unwrap_or(0);
    let highlighted = coco_tui_markdown::highlight_code_lines(
        &visible
            .iter()
            .map(|(_, text)| text.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        lang,
        cx.styles,
        cx.syntax_highlighting,
    );
    let mut rendered = Vec::with_capacity(visible.len() + 1);
    for (index, (num, text)) in visible.iter().enumerate() {
        let prefix = if index == 0 { "  └ " } else { "    " };
        let mut spans = vec![Span::raw(prefix).fg(cx.styles.dim())];
        if gutter_width > 0 {
            spans.push(Span::raw(format!("{num:>gutter_width$}  ")).fg(cx.styles.dim()));
        }
        match highlighted.as_ref().and_then(|h| h.get(index)) {
            Some(line_spans) if !line_spans.is_empty() => {
                spans.extend(line_spans.iter().cloned());
            }
            _ => spans.push(Span::raw(text.clone()).fg(cx.styles.text())),
        }
        rendered.push(Line::from(spans));
    }
    let omitted_hint = (omitted > 0).then_some(omitted);
    if omitted > 0 {
        rendered.push(cx.truncation_line(omitted));
    }
    lines.extend(truncate_lines_middle(cx, rendered, rows, omitted_hint));
}

/// Split a `cat -n` line (`<number>\t<content>`) into `(number, content)`.
/// Returns an empty number when the line has no leading `<digits>\t` prefix so
/// non-numbered lines (e.g. the Read tool's trailing "… N more lines" note)
/// pass through unchanged.
fn split_line_number(line: &str) -> (String, &str) {
    if let Some((prefix, rest)) = line.split_once('\t')
        && !prefix.is_empty()
        && prefix.bytes().all(|b| b.is_ascii_digit() || b == b' ')
    {
        return (prefix.trim().to_string(), rest);
    }
    (String::new(), line)
}

/// Plain-text preview with a configurable row budget and base color.
///
/// Mirrors `codex-rs/tui`'s two-stage command-output algorithm:
/// 1. keep up to `line_limit` logical head lines plus `line_limit` tail lines;
/// 2. after wrapping, middle-truncate again to the visible row budget.
///
/// The second pass is what prevents a few very long lines from flooding the
/// viewport after terminal wrapping.
fn push_text_preview(
    cx: &ToolResultRenderCtx<'_>,
    output: &str,
    rows: usize,
    out: &mut Vec<Line<'static>>,
    color: ratatui::style::Color,
) {
    let Some((rendered, omitted_hint)) = logical_output_lines(cx, output, rows, color) else {
        out.push(result_line("(no output)".to_string(), cx.styles.dim()));
        return;
    };
    out.extend(truncate_lines_middle(cx, rendered, rows, omitted_hint));
}

fn logical_output_lines(
    cx: &ToolResultRenderCtx<'_>,
    output: &str,
    line_limit: usize,
    color: ratatui::style::Color,
) -> Option<(Vec<Line<'static>>, Option<usize>)> {
    if line_limit == 0 {
        return None;
    }

    let mut head = Vec::with_capacity(line_limit);
    let mut tail = VecDeque::with_capacity(line_limit);
    let mut total = 0usize;
    for line in output.lines() {
        if total < line_limit {
            head.push(line);
        } else if line_limit > 0 {
            if tail.len() == line_limit {
                tail.pop_front();
            }
            tail.push_back(line);
        }
        total += 1;
    }

    if total == 0 {
        return None;
    }

    let show_ellipsis = total > 2 * line_limit;
    let omitted = show_ellipsis.then(|| total - 2 * line_limit);
    let mut rendered = Vec::new();
    for (i, line) in head.into_iter().enumerate() {
        rendered.push(output_result_line(
            transcript_safe_line(line),
            color,
            i == 0,
        ));
    }
    if let Some(omitted) = omitted {
        rendered.push(cx.truncation_line(omitted));
    }
    if show_ellipsis {
        rendered.extend(
            tail.into_iter()
                .map(|line| output_result_line(transcript_safe_line(line), color, false)),
        );
    } else {
        rendered.extend(
            tail.into_iter()
                .map(|line| output_result_line(transcript_safe_line(line), color, false)),
        );
    }
    Some((rendered, omitted))
}

fn truncate_lines_middle(
    cx: &ToolResultRenderCtx<'_>,
    lines: Vec<Line<'static>>,
    max_rows: usize,
    omitted_hint: Option<usize>,
) -> Vec<Line<'static>> {
    if max_rows == 0 || lines.is_empty() {
        return Vec::new();
    }

    let width = cx.width.max(1);
    let row_counts: Vec<usize> = lines
        .iter()
        .map(|line| {
            Paragraph::new(Text::from(vec![line.clone()]))
                .wrap(Wrap { trim: false })
                .line_count(width)
                .max(1)
        })
        .collect();
    let total_rows: usize = row_counts.iter().sum();
    if total_rows <= max_rows {
        return lines;
    }

    let estimated_omitted = omitted_hint.unwrap_or(0)
        + lines
            .len()
            .saturating_sub(usize::from(omitted_hint.is_some()));
    let ellipsis_rows = Paragraph::new(Text::from(vec![cx.truncation_line(estimated_omitted)]))
        .wrap(Wrap { trim: false })
        .line_count(width)
        .max(1);
    if ellipsis_rows >= max_rows {
        return vec![cx.truncation_line(estimated_omitted)];
    }

    let available_rows = max_rows - ellipsis_rows;
    let head_budget = available_rows / 2;
    let tail_budget = available_rows - head_budget;

    let mut head = Vec::new();
    let mut head_rows = 0usize;
    let mut head_end = 0usize;
    while head_end < lines.len() {
        let row_count = row_counts[head_end];
        if head_rows + row_count > head_budget {
            break;
        }
        head_rows += row_count;
        head.push(lines[head_end].clone());
        head_end += 1;
    }

    let mut tail_reversed = Vec::new();
    let mut tail_rows = 0usize;
    let mut tail_start = lines.len();
    while tail_start > head_end {
        let idx = tail_start - 1;
        let row_count = row_counts[idx];
        if tail_rows + row_count > tail_budget {
            break;
        }
        tail_rows += row_count;
        tail_reversed.push(lines[idx].clone());
        tail_start -= 1;
    }

    let base = omitted_hint.unwrap_or(0);
    let additional = lines
        .len()
        .saturating_sub(head.len() + tail_reversed.len())
        .saturating_sub(usize::from(omitted_hint.is_some()));
    head.push(cx.truncation_line(base + additional));
    head.extend(tail_reversed.into_iter().rev());
    head
}

/// Default plain-text preview (text color, default row budget) — the
/// graceful-degradation fallback shared by the input-derived renderers.
pub(crate) fn render_output_preview(
    cx: &ToolResultRenderCtx<'_>,
    output: &str,
    lines: &mut Vec<Line<'static>>,
) {
    push_text_preview(
        cx,
        output,
        cx.rows(TOOL_OUTPUT_PREVIEW_ROWS),
        lines,
        cx.styles.text(),
    );
}

/// Prefix each line with a two-space indent so rich blocks nest under the tool
/// header (the diff widget owns its own line-number gutter).
fn indent2(rendered: Vec<Line<'static>>) -> Vec<Line<'static>> {
    rendered
        .into_iter()
        .map(|mut line| {
            line.spans.insert(0, Span::raw("  "));
            line
        })
        .collect()
}

/// Build unified-diff text (`@@` hunks + signed lines) from two strings for the
/// diff widget. No `---`/`+++` file header is emitted — the tool header already
/// names the file, so it would only be redundant inline. The `\ No newline at
/// end of file` markers `similar` emits when an input lacks a trailing newline
/// are stripped — git-porcelain noise with no place in a tool-result view.
fn unified_diff_text(old: &str, new: &str) -> String {
    let diff = similar::TextDiff::from_lines(old, new);
    let mut unified = diff.unified_diff();
    unified.context_radius(3);
    unified
        .to_string()
        .lines()
        .filter(|line| !line.contains("No newline at end of file"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn str_field(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn file_ext(path: &str) -> String {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_string()
}

fn wrap_plain_text(text: &str, max_cols: usize) -> Vec<String> {
    let mut rows = Vec::new();
    let mut current = String::new();
    let mut col = 0usize;

    for ch in text.chars() {
        let width = char_width(ch);
        if col + width > max_cols && !current.is_empty() {
            rows.push(std::mem::take(&mut current));
            col = 0;
        }
        current.push(ch);
        col += width;
        if col >= max_cols {
            rows.push(std::mem::take(&mut current));
            col = 0;
        }
    }

    if !current.is_empty() || rows.is_empty() {
        rows.push(current);
    }
    rows
}

fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

fn char_width(ch: char) -> usize {
    ch.width().unwrap_or(if ch == '\t' { TAB_WIDTH } else { 0 })
}

#[cfg(test)]
#[path = "tool_result.test.rs"]
mod tests;
