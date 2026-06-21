//! System-cell renderers — informational rows, API errors, compaction
//! boundaries, tool-use summaries.
//!
//! Dispatches on `cell.kind` / `cell.source`. System messages reach
//! the TUI through the engine `MessageHistory` flow — only variants
//! the engine actually emits have renderer arms.

#[cfg(test)]
#[path = "system.test.rs"]
mod tests;

use coco_messages::Message;
use coco_messages::SystemMessage;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;

use super::CellsRenderer;
use crate::i18n::t;
use crate::keybinding_bridge::KeybindingContext;
use crate::transcript::cells::CellKind;
use crate::transcript::cells::RenderedCell;
use crate::transcript::cells::SystemCellKind;

pub(super) fn try_render(
    w: &CellsRenderer<'_>,
    cell: &RenderedCell,
    lines: &mut Vec<Line<'static>>,
) -> Option<()> {
    match &cell.kind {
        CellKind::System(SystemCellKind::Informational) => {
            let Message::System(SystemMessage::Informational(info)) = cell.source.as_ref() else {
                return Some(());
            };
            if info.title.is_empty() {
                let opts = coco_tui_markdown::MarkdownOptions::new(
                    w.styles,
                    w.width,
                    w.syntax_highlighting,
                );
                lines.extend(coco_tui_markdown::render_markdown(
                    &info.message,
                    opts,
                    None,
                ));
                return Some(());
            }
            let body = format!("{}: {}", info.title, info.message);
            for line in body.lines() {
                lines.push(Line::from(
                    Span::raw(format!("  # {line}")).fg(w.styles.system_message()),
                ));
            }
            Some(())
        }
        CellKind::System(SystemCellKind::ApiError) => {
            let Message::System(SystemMessage::ApiError(e)) = cell.source.as_ref() else {
                return Some(());
            };
            let status = e.status_code.map(|c| format!(" [{c}]")).unwrap_or_default();
            lines.push(Line::from(
                Span::raw(format!(
                    "  ⚠{status} {error}",
                    status = status,
                    error = e.error
                ))
                .fg(w.styles.error()),
            ));
            Some(())
        }
        CellKind::System(SystemCellKind::CompactBoundary) => {
            let shortcut = compact_boundary_shortcut(w);
            lines.push(Line::from(
                Span::raw(format!("  {}", compact_boundary_text(&shortcut)))
                    .fg(w.styles.border())
                    .dim(),
            ));
            Some(())
        }
        CellKind::System(SystemCellKind::ContextUsage) => {
            // `/context` snapshot — paint the full colored grid + grouped
            // detail block inline.
            let Message::System(SystemMessage::ContextUsage(m)) = cell.source.as_ref() else {
                return Some(());
            };
            lines.extend(crate::presentation::context_view::report_lines(
                &m.result, w.styles, w.cwd,
            ));
            Some(())
        }
        // Remaining SystemCellKind sub-variants (PermissionRetry,
        // BridgeStatus, MemorySaved, AwaySummary, AgentsKilled,
        // ApiMetrics, StopHookSummary, TurnDuration, ScheduledTaskFire,
        // MicrocompactBoundary) render as plain informational rows
        // using the wrapped engine message's text content.
        CellKind::System(_) => {
            let body = system_message_summary(cell.source.as_ref()).unwrap_or_default();
            if !body.is_empty() {
                for line in body.lines() {
                    lines.push(Line::from(
                        Span::raw(format!("  # {line}")).fg(w.styles.system_message()),
                    ));
                }
            }
            Some(())
        }
        _ => None,
    }
}

fn compact_boundary_shortcut(w: &CellsRenderer<'_>) -> String {
    w.kb_handle
        .and_then(|handle| {
            handle.display_for(
                &coco_keybindings::KeybindingAction::AppToggleTranscript,
                KeybindingContext::Chat,
            )
        })
        .unwrap_or_else(|| "ctrl+o".to_string())
}

pub(super) fn compact_boundary_text(shortcut: &str) -> String {
    t!("chat.compact_boundary", shortcut = shortcut).to_string()
}

/// Best-effort short summary of a [`SystemMessage`] for the
/// generic fallback row. Each sub-variant has its own typed shape;
/// this helper picks the most readable string field.
fn system_message_summary(msg: &Message) -> Option<String> {
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
        // Handled by their own match arms above.
        SystemMessage::Informational(_)
        | SystemMessage::ApiError(_)
        | SystemMessage::CompactBoundary(_)
        | SystemMessage::MicrocompactBoundary(_)
        | SystemMessage::LocalCommand(_)
        // ContextUsage paints via its own render arm above; no summary row.
        | SystemMessage::ContextUsage(_)
        | SystemMessage::UserInterruption(_) => return None,
    })
}
