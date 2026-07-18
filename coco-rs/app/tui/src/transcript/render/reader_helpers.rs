//! Small text/layout helpers for the expanded transcript reader.

use coco_keybindings::KeybindingAction;
use coco_messages::Message;
use coco_messages::SystemMessage;
use ratatui::prelude::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;

use crate::i18n::t;
use crate::keybinding_bridge::KeybindingContext as TuiContext;
use crate::presentation::layout::text_width;
use crate::presentation::transcript::TRANSCRIPT_LINE_CHAR_CAP;
use crate::state::AppState;
use crate::tool_display::ToolNameTone;
use crate::transcript::cells::RenderedCell;

pub(crate) fn meta_preview_text(cell: &RenderedCell) -> String {
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

pub(super) fn system_summary_text(msg: &Message) -> Option<String> {
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

pub(super) fn compact_boundary_shortcut(state: &AppState) -> String {
    state
        .ui
        .kb_handle
        .display_for(&KeybindingAction::AppToggleTranscript, TuiContext::Chat)
        .unwrap_or_else(|| "ctrl+o".to_string())
}

pub(super) fn thinking_toggle_hint(state: &AppState) -> String {
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

pub(super) fn plan_editor_hint(state: &AppState) -> String {
    let shortcut = state
        .ui
        .kb_handle
        .display_for(&KeybindingAction::AppPlanEditor, TuiContext::Chat)
        .unwrap_or_else(|| "ctrl+g".to_string());
    format!("{shortcut} to edit")
}

pub(super) fn result_line(text: String, color: ratatui::style::Color) -> Line<'static> {
    output_result_line(text, color, true)
}

pub(super) fn tool_tone_color(
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
    let prefix = if first { "    └ " } else { "      " };
    Line::from(vec![Span::raw(prefix).fg(color), Span::raw(text).fg(color)])
}

pub(super) fn single_line_capped(text: &str, max_chars: usize) -> String {
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

pub(super) fn transcript_safe_line(line: &str) -> String {
    truncate_chars(line, TRANSCRIPT_LINE_CHAR_CAP)
}

pub(super) fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    text.chars().take(max_chars).collect()
}

pub(super) fn push_capped(out: &mut String, text: &str, max_chars: usize) {
    let remaining = max_chars.saturating_sub(out.chars().count());
    out.extend(text.chars().take(remaining));
}

pub(super) fn wrapped_height(lines: &[Line<'static>], width: u16) -> usize {
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
