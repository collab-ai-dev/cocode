//! Read-only agent-view overlay — the full-viewport reader shown while
//! `session.viewing_agent_id` is set. It paints over the live viewport (like
//! the transcript modal) instead of swapping the native-scrollback source, so
//! the single scrollback-commit owner is never disturbed (tui-v2 §6.7-10).
//!
//! Layout: a bordered banner+body box summarizing the agent's run, the
//! agent-switcher rail at the bottom so the user can hop between agents, and a
//! one-line hint. The body is the agent's activity summary — its description,
//! recent tool activity (`recent_activities`), and closing `final_message` —
//! which is the intended read-only view; the per-message transcript is
//! deliberately not loaded.

use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Block;
use ratatui::widgets::Borders;
use ratatui::widgets::Clear;
use ratatui::widgets::Padding;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Wrap;

use crate::i18n::t;
use crate::state::AppState;
use crate::state::session::SubagentStatus;
use coco_tui_ui::engine::terminal::SurfaceFrame;
use coco_tui_ui::style::UiStyles;

/// Paint the agent-view overlay over `area`. No-op when no agent is being
/// viewed (the caller already gates on `viewing_agent_id`, but resolving the
/// instance can still miss if it just terminated).
pub(crate) fn render_agent_view_overlay(
    frame: &mut SurfaceFrame<'_>,
    area: Rect,
    state: &AppState,
    styles: UiStyles<'_>,
) {
    let Some(agent) = state.session.viewing_agent() else {
        return;
    };
    if area.height == 0 || area.width == 0 {
        return;
    }
    frame.render_widget(Clear, area);

    let color = agent
        .color
        .map(crate::widgets::suggestion_popup::agent_color_to_ratatui)
        .unwrap_or_else(|| styles.accent());

    // Reserve the rail + hint at the bottom; the body takes the rest.
    let switcher_view = crate::widgets::build_agent_switcher_view(state);
    let rail_rows = switcher_view.row_count().min(6);
    let [body_area, rail_area, hint_area] = area.layout(&Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(rail_rows),
        Constraint::Length(1),
    ]));

    // ── Body: banner title + conversation content ──
    let status = match agent.status {
        SubagentStatus::Running => t!("task_status.running"),
        SubagentStatus::Completed => t!("task_status.completed"),
        SubagentStatus::Failed => t!("task_status.failed"),
    };
    // Compact token format (`26.4k`) to match the Agents panel + status bar —
    // the banner previously showed raw integers (`↑26441`).
    let title = format!(
        " {} · {} · {} tools · ↑{} ↓{} ",
        agent.agent_type,
        status,
        agent.tool_count,
        crate::presentation::activity::format_short_tokens(agent.input_tokens),
        crate::presentation::activity::format_short_tokens(agent.output_tokens),
    );

    let mut lines: Vec<Line<'static>> = Vec::new();
    if !agent.description.is_empty() {
        lines.push(Line::from(Span::styled(
            agent.description.clone(),
            Style::default().fg(styles.text()),
        )));
        lines.push(Line::default());
    }
    lines.push(dim(
        t!("dialog.background_activity_label").to_string(),
        styles,
    ));
    if agent.recent_activities.is_empty() {
        lines.push(dim(t!("dialog.background_no_activity").to_string(), styles));
    } else {
        for act in &agent.recent_activities {
            let mut spans = vec![
                Span::styled(" · ".to_string(), Style::default().fg(styles.dim())),
                Span::styled(act.tool_name.clone(), Style::default().fg(styles.text())),
            ];
            if let Some(summary) = &act.summary {
                spans.push(Span::styled(
                    format!("  {summary}"),
                    Style::default().fg(styles.dim()),
                ));
            }
            lines.push(Line::from(spans));
        }
    }
    if let Some(final_message) = &agent.final_message {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            final_message.clone(),
            Style::default().fg(styles.text()),
        )));
    }

    let body = Paragraph::new(lines).wrap(Wrap { trim: false }).block(
        Block::default()
            .borders(Borders::ALL)
            .padding(Padding::horizontal(1))
            .title(title)
            .border_style(Style::default().fg(color)),
    );
    frame.render_widget(body, body_area);

    // ── Rail + hint ──
    if rail_rows > 0 {
        frame.render_widget(
            crate::widgets::AgentSwitcher::new(&switcher_view, styles),
            rail_area,
        );
    }
    let agent_name = agent.agent_type.clone();
    frame.render_widget(
        Paragraph::new(dim(
            t!("switcher.viewing_banner", agent = agent_name).to_string(),
            styles,
        )),
        hint_area,
    );
}

fn dim(text: String, styles: UiStyles<'_>) -> Line<'static> {
    Line::from(Span::styled(text, Style::default().fg(styles.dim())))
}
