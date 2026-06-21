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
        let lines: Vec<Line> = match status_bar_view(self.state) {
            StatusBarView::ExitPrompt { key, text } => {
                tracing::info!(
                    key = key.label(),
                    prompt = %text,
                    width = area.width,
                    "status bar rendering exit prompt"
                );
                vec![Line::from(Span::styled(
                    text,
                    Style::default().fg(self.styles.warning()).bold(),
                ))]
            }
            StatusBarView::Custom { line } => vec![Line::from(Span::styled(
                line,
                Style::default().fg(self.styles.primary()),
            ))],
            StatusBarView::BuiltIn { lines } => lines
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
        };
        Paragraph::new(lines).render(area, buf);
    }
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
    };
    let rendered = Span::styled(span.text.clone(), Style::default().fg(color));
    if span.bold { rendered.bold() } else { rendered }
}
