//! `/resume` session picker presentation.

use coco_tui_ui::style::UiStyles;
use ratatui::prelude::*;

use crate::i18n::t;
use crate::presentation::layout::truncate_to_width;
use crate::state::SessionBrowserState;

pub(crate) fn session_browser_lines(
    state: &SessionBrowserState,
    styles: UiStyles<'_>,
    list_budget: usize,
) -> (String, Vec<Line<'static>>, Color) {
    let title = t!("dialog.title_sessions").to_string();
    let sessions = state.display_sessions();
    let filter = if state.filter.is_empty() {
        dim_line(t!("dialog.type_filter_sessions"), styles)
    } else {
        dim_line(
            t!("dialog.filter_prefix", text = state.filter.as_str()),
            styles,
        )
    };
    if sessions.is_empty() {
        let status = if state.is_searching {
            t!("dialog.sessions_searching")
        } else {
            t!("dialog.no_saved_sessions")
        };
        return (
            title,
            vec![filter, Line::default(), dim_line(status, styles)],
            styles.primary(),
        );
    }

    let selected = (state.selected.max(0) as usize).min(sessions.len().saturating_sub(1));
    let mut list = Vec::new();
    let mut selected_line = 0usize;
    let mut previous_cwd: Option<&str> = None;
    for (index, session) in sessions.iter().enumerate() {
        if previous_cwd != Some(session.cwd.as_str()) {
            previous_cwd = Some(&session.cwd);
            let name = std::path::Path::new(&session.cwd)
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| !name.is_empty())
                .unwrap_or(session.cwd.as_str());
            let header = if session.cwd == state.current_cwd {
                t!(
                    "dialog.sessions_group_current",
                    name = name,
                    cwd = session.cwd.as_str()
                )
            } else {
                t!(
                    "dialog.sessions_group",
                    name = name,
                    cwd = session.cwd.as_str()
                )
            };
            list.push(Line::from(Span::styled(
                header.to_string(),
                Style::default().fg(styles.secondary()).bold(),
            )));
        }

        let focused = index == selected;
        if focused {
            selected_line = list.len();
        }
        let age = session_age(session);
        let label = truncate_to_width(
            &format!(
                "{} · {}{} · {age}",
                session.label,
                session.message_count,
                t!("dialog.sessions_item_suffix")
            ),
            88,
        );
        list.push(Line::from(vec![
            Span::styled(
                if focused { "❯ " } else { "  " },
                Style::default().fg(styles.accent()),
            ),
            Span::styled(
                label,
                Style::default().fg(if focused {
                    styles.accent()
                } else {
                    styles.text()
                }),
            ),
        ]));
        if focused {
            push_detail(&mut list, "cwd", &session.cwd, styles);
            push_detail(&mut list, "first", &session.first_prompt, styles);
            if let Some(last) = session.last_message_preview.as_deref() {
                push_detail(&mut list, "last", last, styles);
            }
            if let Some(snippet) = state.content_hits.get(&session.id) {
                push_detail(&mut list, "match", snippet, styles);
            }
        }
    }

    let visible = list_budget.max(1);
    let start = selected_line
        .saturating_sub(visible / 2)
        .min(list.len().saturating_sub(visible));
    let mut lines = vec![filter, Line::default()];
    lines.extend(list.into_iter().skip(start).take(visible));
    lines.push(Line::default());
    if state.is_searching {
        lines.push(dim_line(t!("dialog.sessions_searching"), styles));
    }
    lines.push(dim_line(t!("dialog.hints_nav_resume_cancel"), styles));
    (title, lines, styles.primary())
}

fn dim_line(text: impl Into<String>, styles: UiStyles<'_>) -> Line<'static> {
    Line::from(Span::styled(text.into(), styles.dim_style()))
}

fn push_detail(lines: &mut Vec<Line<'static>>, label: &str, value: &str, styles: UiStyles<'_>) {
    let value = value.trim();
    if value.is_empty() {
        return;
    }
    lines.push(Line::from(vec![
        Span::styled(format!("    {label}: "), styles.dim_style()),
        Span::styled(truncate_to_width(value, 82), styles.dim_style()),
    ]));
}

fn session_age(session: &crate::state::SessionOption) -> String {
    let raw = session.updated_at.as_deref().unwrap_or(&session.created_at);
    let Ok(timestamp_ms) = raw.parse::<i64>() else {
        return raw.to_string();
    };
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(timestamp_ms);
    crate::presentation::time::format_age(now_ms, timestamp_ms)
}
