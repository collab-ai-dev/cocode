//! User-side cell renderers — text input, attachments, bash invocations
//! (`!cmd` echoed plus output), engine-pushed user interruption marker.
//!
//! Dispatches directly on `cell.kind` / `cell.source: Arc<Message>`.
//! All emitted lines are `Line<'static>` (owned spans).

use coco_messages::Message;
use coco_messages::SystemMessage;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;

use super::CellsRenderer;
use crate::i18n::t;
use crate::transcript::cells::CellKind;
use crate::transcript::cells::RenderedCell;
use crate::transcript::cells::SystemCellKind;

pub(super) fn try_render(
    w: &CellsRenderer<'_>,
    cell: &RenderedCell,
    lines: &mut Vec<Line<'static>>,
) -> Option<()> {
    match &cell.kind {
        CellKind::UserText { text } => {
            // Clear-context plan exit injects the full approved plan as the
            // next user message so the post-clear model still has it. Render a
            // compact chip instead of echoing ~50 lines back as a `❯` wall.
            if matches!(
                cell.source.as_ref(),
                Message::User(u)
                    if u.origin == Some(coco_messages::MessageOrigin::PlanImplementation)
            ) {
                let mut spans = vec![
                    Span::raw("◇ ").fg(w.styles.accent()).dim(),
                    Span::raw(t!("chat.implementing_approved_plan").to_string()).fg(w.styles.dim()),
                ];
                if let Some(file) = plan_implementation_chip_file(text) {
                    spans.push(Span::raw(format!(" · {file}")).fg(w.styles.dim()).bold());
                }
                lines.push(Line::from(spans));
                return Some(());
            }
            if let Some(rendered) =
                crate::presentation::slash_command::render_slash_command_user_text(
                    cell.source.as_ref(),
                    text,
                    crate::presentation::slash_command::SlashCommandRenderOptions {
                        styles: w.styles,
                        width: w.width,
                        syntax_highlighting: w.syntax_highlighting,
                        apply_user_background: true,
                    },
                )
            {
                lines.extend(rendered);
                return Some(());
            }
            // One logical message = one `❯` (mirrors TS `HighlightedThinkingText`,
            // which renders the pointer once and lets the whole multi-line text
            // flow under it). Continuation rows align to the content column with a
            // 2-space gutter, so a recalled multi-message edit reads as a single
            // prompt with line breaks — not several `❯` submissions.
            //
            // Subtle background tint behind user prompt rows. The background must
            // paint the full row width rather than just the glyphs — the bg must
            // therefore live on the `Line`, not on individual spans.
            for (idx, line) in text.lines().enumerate() {
                let gutter = if idx == 0 { "❯ " } else { "  " };
                let span = Span::raw(format!("{gutter}{line}")).fg(w.styles.user_message());
                let mut chat_line = Line::from(span);
                if let Some(bg) = w.styles.user_message_bg() {
                    chat_line = chat_line.style(ratatui::style::Style::default().bg(bg));
                }
                lines.push(chat_line);
            }
            // Hang a `⎿ [Image #N]` confirmation row under the prompt for each
            // pasted image, so an attachment that lives only as an inline
            // placeholder still reads as a distinct, attached artifact.
            for pill in image_pill_refs(text) {
                lines.push(Line::from(vec![
                    Span::raw("  ⎿ ").fg(w.styles.dim()),
                    Span::raw(pill).fg(w.styles.dim()),
                ]));
            }
            Some(())
        }
        CellKind::Attachment => {
            // Renderable attachments (`renders_in_transcript() == true`) are
            // CONTENT, not collapsed `# [meta]` reminders — the TUI defers
            // meta-ness to the engine's `is_meta_message` (see
            // `presentation::transcript::is_meta`).
            //
            // Memory injections (nested CLAUDE.md / relevant memories) collapse to
            // a compact `◆ memory · <path>` chip: a width-1 marker aligned to the
            // column-2 gutter, distinct from tool/assistant dots by shape + dim
            // styling. Other attachments show the body's first line;
            // silent / structured payloads render nothing.
            if let Some(rows) = super::mention_summary_lines(cell.source.as_ref(), w.styles) {
                // Resolved `@`-mention summary: one compact `└ Read …` /
                // `└ Listed directory …` row per file/dir, hanging under the
                // user prompt. The raw `@-mentioned files` system-reminder is
                // suppressed in `attachment_summary_text`.
                lines.extend(rows);
            } else if let Some(path) =
                super::compact_file_reference_chip_path(cell.source.as_ref(), w.cwd)
            {
                lines.push(Line::from(vec![
                    Span::raw("◇ ").fg(w.styles.accent()).dim(),
                    Span::raw("Referenced file ").fg(w.styles.dim()),
                    Span::raw(path).fg(w.styles.dim()).bold(),
                ]));
            } else if let Some(path) = super::nested_memory_chip_path(cell.source.as_ref(), w.cwd) {
                lines.push(Line::from(vec![
                    Span::raw("◆ ").fg(w.styles.accent()).dim(),
                    Span::raw("memory · ").fg(w.styles.dim()),
                    Span::raw(path).fg(w.styles.dim()),
                ]));
            } else if let Some(summary) = super::attachment_summary_text(cell.source.as_ref()) {
                // Generic attachment: width-1 hollow `◇` (vs memory's filled `◆`)
                // so injected context still aligns at the column-2 gutter.
                lines.push(Line::from(vec![
                    Span::raw("◇ ").fg(w.styles.accent()).dim(),
                    Span::raw(summary).fg(w.styles.dim()),
                ]));
            }
            Some(())
        }
        CellKind::System(SystemCellKind::UserInterruption { for_tool_use }) => {
            // Dim "Interrupted · …" row. The `for_tool_use` flag is
            // the engine-authoritative answer to "was a tool in flight
            // when the user cancelled?" (computed once in
            // `finalize_user_cancel`). Surfaces a more specific wording
            // for mid-tool cancellation so users see the distinction
            // encoded in the `INTERRUPT_MESSAGE_FOR_TOOL_USE` text variant.
            let key = if *for_tool_use {
                "chat.interrupted_for_tool_use_marker"
            } else {
                "chat.interrupted_marker"
            };
            lines.push(Line::from(
                Span::raw(t!(key).to_string()).fg(w.styles.dim()),
            ));
            Some(())
        }
        CellKind::System(SystemCellKind::LocalCommand) => {
            // `!cmd` echo: render the command on the `! …` row and the
            // captured stdout/stderr indented below it. Exit code isn't
            // carried on `SystemLocalCommandMessage` yet — treat
            // everything as success-styled.
            let Message::System(SystemMessage::LocalCommand(lc)) = cell.source.as_ref() else {
                return Some(());
            };
            // Bash input row — re-uses the input-area mode glyph so the
            // chat echo visually matches the prompt the user typed.
            lines.push(Line::from(vec![
                Span::raw("! ").fg(w.styles.accent()).bold(),
                Span::raw(lc.command.clone()).fg(w.styles.accent()),
            ]));
            // Output body, capped at 20 visible rows with a "… truncated"
            // hint below.
            let mut iter = lc.output.lines();
            for line in iter.by_ref().take(20) {
                lines.push(Line::from(
                    Span::raw(format!("  {line}")).fg(w.styles.dim()),
                ));
            }
            if iter.next().is_some() {
                lines.push(Line::from(
                    Span::raw(t!("chat.truncated").to_string())
                        .fg(w.styles.dim())
                        .italic(),
                ));
            }
            Some(())
        }
        _ => None,
    }
}

/// Extract every `[Image #N]` placeholder (in order) from a user prompt so the
/// renderer can hang a `⎿ [Image #N]` confirmation row under the `❯` text. Only
/// well-formed pills (a non-empty all-digit `N`) match, so literal user text
/// like `[Image #foo]` is ignored.
fn image_pill_refs(text: &str) -> Vec<String> {
    let mut refs = Vec::new();
    for (start, _) in text.match_indices("[Image #") {
        let rest = &text[start..];
        if let Some(close) = rest.find(']') {
            let num = &rest["[Image #".len()..close];
            if !num.is_empty() && num.bytes().all(|b| b.is_ascii_digit()) {
                refs.push(rest[..=close].to_string());
            }
        }
    }
    refs
}

/// Extract the plan-file basename from the clear-context implement message
/// body (`…\n\nPlan file path: <path>`), for the compact chip's `· <file>`
/// suffix. `None` when the marker line is absent.
fn plan_implementation_chip_file(text: &str) -> Option<String> {
    let path = text
        .lines()
        .find_map(|line| line.strip_prefix("Plan file path: "))?;
    let name = path.trim().rsplit('/').next()?;
    (!name.is_empty()).then(|| name.to_string())
}

#[cfg(test)]
#[path = "user.test.rs"]
mod tests;
