//! Tool input display helpers shared by permission prompts and chat previews.
//!
//! The pure per-tool argument summarisation lives in
//! [`coco_types::tool_summary`] so producers below the UI layer (the swarm
//! coordinator) can reuse it. This module keeps only the UI-facing concerns:
//! syntax-highlighted spans, tone colours, and overlay suppression.

use coco_tui_ui::display::SyntaxHighlighting;
use coco_tui_ui::style::UiStyles;
use coco_types::PermissionDisplayInput;
use coco_types::ToolName;
use coco_types::tool_summary::cap_single_line;
use coco_types::tool_summary::normalized_builtin_tool;
use coco_types::tool_summary::tool_input_multiline;
use coco_types::tool_summary::tool_input_summary;
use ratatui::style::Stylize;
use ratatui::text::Span;
use serde_json::Value;
use unicode_width::UnicodeWidthChar;
use unicode_width::UnicodeWidthStr;

pub(crate) const TOOL_INPUT_PREVIEW_MAX_CHARS: usize = 512;
const PERMISSION_DISPLAY_MAX_CHARS: usize = 1_200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolNameTone {
    ReadOnly,
    Shell,
    Write,
    Agent,
    Plan,
    Utility,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolInputPreview {
    Plain(String),
    ShellCommand { command: String, syntax: String },
    Code { text: String, lang: String },
}

impl ToolInputPreview {
    pub fn plain_text(&self) -> &str {
        match self {
            Self::Plain(text) => text,
            Self::ShellCommand { command, .. } => command,
            Self::Code { text, .. } => text,
        }
    }
}

pub fn permission_display_input(tool_name: &str, input: &Value) -> PermissionDisplayInput {
    if is_shell_tool(tool_name)
        && let Some(command) = input.get("command").and_then(Value::as_str)
    {
        return PermissionDisplayInput::Command(cap_single_line(
            command,
            PERMISSION_DISPLAY_MAX_CHARS,
        ));
    }

    let display = tool_input_multiline(tool_name, input, PERMISSION_DISPLAY_MAX_CHARS);
    if display.is_empty() {
        PermissionDisplayInput::Empty
    } else {
        PermissionDisplayInput::Text(display)
    }
}

pub fn tool_input_preview(tool_name: &str, input: &Value) -> String {
    cap_single_line(
        tool_input_semantic_preview(tool_name, input).plain_text(),
        TOOL_INPUT_PREVIEW_MAX_CHARS,
    )
}

pub(crate) fn tool_input_semantic_preview(tool_name: &str, input: &Value) -> ToolInputPreview {
    if let Some(tool) = normalized_builtin_tool(tool_name)
        && matches!(tool, ToolName::Bash | ToolName::PowerShell)
        && let Some(command) = input.get("command").and_then(Value::as_str)
    {
        let syntax = if matches!(tool, ToolName::PowerShell) {
            "powershell"
        } else {
            "bash"
        };
        return ToolInputPreview::ShellCommand {
            command: command.to_string(),
            syntax: syntax.to_string(),
        };
    }
    // Plain branch covers builtin non-shell tools (per-field pick) and
    // unrecognised tools (object summary) — `tool_input_summary` handles both.
    ToolInputPreview::Plain(tool_input_summary(tool_name, input))
}

pub(crate) fn render_tool_input_preview_spans(
    preview: &ToolInputPreview,
    styles: UiStyles<'_>,
    syntax_highlighting: SyntaxHighlighting,
    max_width: usize,
) -> Vec<Span<'static>> {
    if preview.plain_text().is_empty() {
        return Vec::new();
    }
    let spans = match preview {
        ToolInputPreview::ShellCommand { command, syntax } => {
            coco_tui_markdown::highlight_code_lines(command, syntax, styles, syntax_highlighting)
                .and_then(|lines| lines.first().cloned())
                .filter(|line| !line.is_empty())
                .unwrap_or_else(|| vec![Span::raw(command.clone()).fg(styles.text())])
        }
        ToolInputPreview::Code { text, lang } => {
            coco_tui_markdown::highlight_code_lines(text, lang, styles, syntax_highlighting)
                .and_then(|lines| lines.first().cloned())
                .filter(|line| !line.is_empty())
                .unwrap_or_else(|| vec![Span::raw(text.clone()).fg(styles.text())])
        }
        ToolInputPreview::Plain(text) => vec![Span::raw(text.clone()).fg(styles.text())],
    };
    truncate_spans_to_width(spans, max_width)
}

pub fn tool_name_tone(tool_name: &str) -> ToolNameTone {
    let Some(tool) = normalized_builtin_tool(tool_name) else {
        return ToolNameTone::Utility;
    };

    match tool {
        ToolName::Read
        | ToolName::Glob
        | ToolName::Grep
        | ToolName::WebFetch
        | ToolName::WebSearch
        | ToolName::TaskGet
        | ToolName::TaskList
        | ToolName::TaskOutput
        | ToolName::ToolSearch
        | ToolName::Lsp
        | ToolName::ListMcpResources
        | ToolName::ReadMcpResource
        | ToolName::CronList => ToolNameTone::ReadOnly,
        ToolName::Bash | ToolName::PowerShell | ToolName::Repl => ToolNameTone::Shell,
        ToolName::Write
        | ToolName::Edit
        | ToolName::NotebookEdit
        | ToolName::ApplyPatch
        | ToolName::TodoWrite
        | ToolName::TaskCreate
        | ToolName::TaskUpdate
        | ToolName::TaskStop
        | ToolName::SendMessage
        | ToolName::TeamCreate
        | ToolName::TeamDelete
        | ToolName::Config
        | ToolName::CronCreate
        | ToolName::CronDelete
        | ToolName::RemoteTrigger => ToolNameTone::Write,
        ToolName::Agent | ToolName::Skill => ToolNameTone::Agent,
        ToolName::EnterPlanMode
        | ToolName::ExitPlanMode
        | ToolName::VerifyPlanExecution
        | ToolName::EnterWorktree
        | ToolName::ExitWorktree => ToolNameTone::Plan,
        ToolName::AskUserQuestion
        | ToolName::McpAuth
        | ToolName::SendUserMessage
        | ToolName::Sleep
        | ToolName::StructuredOutput => ToolNameTone::Utility,
    }
}

fn is_shell_tool(tool_name: &str) -> bool {
    matches!(
        normalized_builtin_tool(tool_name),
        Some(ToolName::Bash | ToolName::PowerShell)
    )
}

fn truncate_spans_to_width(spans: Vec<Span<'static>>, max_width: usize) -> Vec<Span<'static>> {
    if max_width == 0 {
        return Vec::new();
    }
    let total = spans
        .iter()
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
        .sum::<usize>();
    if total <= max_width {
        return spans;
    }

    let content_width = max_width.saturating_sub(1);
    let mut used = 0usize;
    let mut out = Vec::new();
    let mut last_style = spans.last().map(|span| span.style).unwrap_or_default();
    for span in spans {
        if used >= content_width {
            break;
        }
        last_style = span.style;
        let mut content = String::new();
        for ch in span.content.chars() {
            let width = UnicodeWidthChar::width(ch).unwrap_or(0);
            if used + width > content_width {
                break;
            }
            content.push(ch);
            used += width;
        }
        if !content.is_empty() {
            out.push(Span::styled(content, span.style));
        }
    }
    out.push(Span::styled("…", last_style));
    out
}

/// Tools whose entire user interaction *is* a dedicated overlay or prompt
/// (the plan-approval dialog, the plan-mode banner, the question dialog).
/// Their real UI is that surface, so they must never surface as a generic
/// tool element anywhere — not an `◦ ExitPlanMode (1s)` activity row, a
/// `⠋ Processing…` busy spinner, nor a `● ExitPlanMode(plan: …)` tool-call
/// header. Their result (the plan / the answers) still renders from
/// `MessageHistory` via the result path.
///
/// This is the single predicate behind that suppression, enforced at two
/// data boundaries so every downstream render path is leak-proof by
/// construction:
/// - UI tool-ledger ingestion — [`crate::state::session::SessionState::start_tool`]
///   (kills the activity strip, the busy spinner, and foreground-task checks);
/// - message→cell derivation — [`crate::transcript::derive::message_to_cells`]
///   (kills the `● ToolName(…)` invocation header; the result orphan-renders).
///
/// Mirrors claude-code's `userFacingName() == ""` (tool-use renders `null`;
/// only the result shows) and codex-rs routing these flows as request-based
/// interrupts rather than tool-call cells.
pub(crate) fn tool_is_overlay_driven(tool_name: &str) -> bool {
    matches!(
        normalized_builtin_tool(tool_name),
        Some(ToolName::ExitPlanMode | ToolName::EnterPlanMode | ToolName::AskUserQuestion)
    )
}

#[cfg(test)]
#[path = "tool_display.test.rs"]
mod tests;
