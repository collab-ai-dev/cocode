use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;

use crate::state::AppState;
use crate::status_bar::StatusBarView;
use crate::status_bar::StatusSpan;
use crate::status_bar::StatusTone;
use crate::status_bar::status_bar_view;
use coco_tui_ui::style::UiStyles;

/// Preserve enough room for the model/context identity before shortening the
/// right-aligned side-chat affordance.
const MIN_LEFT_STATUS_WIDTH: usize = 24;
const SIDE_CHAT_HINT_GAP: usize = 1;

pub(crate) struct StatusBarWidget<'a> {
    state: &'a AppState,
    styles: UiStyles<'a>,
}

impl<'a> StatusBarWidget<'a> {
    pub(crate) fn new(state: &'a AppState, styles: UiStyles<'a>) -> Self {
        Self { state, styles }
    }
}

impl Widget for StatusBarWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 {
            return;
        }
        let (lines, allow_side_chat_hint): (Vec<Line>, bool) = match status_bar_view(self.state) {
            StatusBarView::ExitPrompt { key, text } => {
                tracing::info!(
                    key = key.label(),
                    prompt = %text,
                    width = area.width,
                    "status bar rendering exit prompt"
                );
                (
                    vec![Line::from(Span::styled(
                        text,
                        Style::default().fg(self.styles.warning()).bold(),
                    ))],
                    false,
                )
            }
            StatusBarView::Custom { line } => (
                vec![Line::from(Span::styled(
                    line,
                    Style::default().fg(self.styles.primary()),
                ))],
                true,
            ),
            StatusBarView::BuiltIn { lines } => (
                lines
                    .iter()
                    .map(|spans| {
                        Line::from(
                            spans
                                .iter()
                                .map(|span| status_span(span, self.styles))
                                .collect::<Vec<_>>(),
                        )
                    })
                    .collect(),
                true,
            ),
        };
        let hint = if allow_side_chat_hint {
            side_chat_hint_line(self.state, self.styles, area.width)
        } else {
            None
        };
        let hint_width = hint.as_ref().map_or(0, |line| line.width() as u16);
        let gap = u16::from(hint_width > 0 && area.width > hint_width);
        let main_width = area.width.saturating_sub(hint_width).saturating_sub(gap);
        if let Some(hint) = hint {
            let main_area = Rect {
                width: main_width,
                height: 1,
                ..area
            };
            Paragraph::new(lines.first().cloned().unwrap_or_default()).render(main_area, buf);
            if area.height > 1 {
                let remaining_area = Rect {
                    y: area.y.saturating_add(1),
                    height: area.height.saturating_sub(1),
                    ..area
                };
                Paragraph::new(lines.into_iter().skip(1).collect::<Vec<_>>())
                    .render(remaining_area, buf);
            }
            let hint_area = Rect {
                x: area.x.saturating_add(main_width).saturating_add(gap),
                y: area.y,
                width: hint_width,
                height: 1,
            };
            Paragraph::new(hint).render(hint_area, buf);
        } else {
            Paragraph::new(lines).render(area, buf);
        }
    }
}

fn side_chat_hint_line(
    state: &AppState,
    styles: UiStyles<'_>,
    area_width: u16,
) -> Option<Line<'static>> {
    if !state.is_viewing_side_chat() {
        return None;
    }

    let label = crate::i18n::t!("status.sidechat_label").to_string();
    let (action, compact_action) = if state.has_interruptible_work() {
        (
            crate::i18n::t!("status.sidechat_interrupt_action").to_string(),
            crate::i18n::t!("status.sidechat_interrupt_compact").to_string(),
        )
    } else {
        (
            crate::i18n::t!("status.sidechat_return_action").to_string(),
            crate::i18n::t!("status.sidechat_return_compact").to_string(),
        )
    };

    let label_span = || Span::styled(label.clone(), Style::default().fg(styles.accent()).bold());
    let shortcut_span = || Span::styled("Ctrl+C", Style::default().fg(styles.accent()).bold());
    let candidates = vec![
        Line::from(vec![
            label_span(),
            Span::styled(" · ", styles.dim_style()),
            shortcut_span(),
            Span::styled(format!(" {action}"), styles.dim_style()),
        ]),
        Line::from(vec![
            shortcut_span(),
            Span::styled(format!(" {action}"), styles.dim_style()),
        ]),
        Line::from(vec![
            shortcut_span(),
            Span::styled(format!(" {compact_action}"), styles.dim_style()),
        ]),
        Line::from(shortcut_span()),
    ];
    let preferred_width =
        usize::from(area_width).saturating_sub(MIN_LEFT_STATUS_WIDTH + SIDE_CHAT_HINT_GAP);

    candidates
        .iter()
        .find(|line| line.width() <= preferred_width)
        .cloned()
        .or_else(|| {
            candidates
                .into_iter()
                .rev()
                .find(|line| line.width() <= usize::from(area_width))
        })
}

fn status_span(span: &StatusSpan, styles: UiStyles<'_>) -> Span<'static> {
    let color = match span.tone {
        StatusTone::Primary => styles.primary(),
        StatusTone::Dim => styles.dim(),
        StatusTone::Border => styles.border(),
        StatusTone::Warning => styles.warning(),
        StatusTone::Accent => styles.accent(),
        StatusTone::Plan => styles.plan(),
        StatusTone::Error => styles.error(),
        StatusTone::Success => styles.success(),
    };
    let rendered = Span::styled(span.text.clone(), Style::default().fg(color));
    if span.bold { rendered.bold() } else { rendered }
}

#[cfg(test)]
#[path = "widget.test.rs"]
mod tests;
