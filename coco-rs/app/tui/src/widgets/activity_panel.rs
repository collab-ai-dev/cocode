//! Unified live activity panel.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Block;
use ratatui::widgets::Borders;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use ratatui::widgets::Wrap;

use crate::i18n::t;
use crate::presentation::activity::ActivityBorder;
use crate::presentation::activity::ActivitySpan;
use crate::presentation::activity::ActivitySurfaceView;
use crate::presentation::activity::ActivityTitle;
use crate::presentation::activity::ActivityTone;
use crate::presentation::activity::TurnActivityView;
use coco_tui_ui::style::UiStyles;

pub(crate) struct ActivityPanel<'a> {
    view: TurnActivityView,
    styles: UiStyles<'a>,
}

impl<'a> ActivityPanel<'a> {
    pub(crate) fn new(view: TurnActivityView, styles: UiStyles<'a>) -> Self {
        Self { view, styles }
    }
}

impl Widget for ActivityPanel<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let TurnActivityView::Surface(surface) = self.view else {
            return;
        };

        let lines = surface
            .lines
            .iter()
            .map(|line| {
                Line::from(
                    line.spans
                        .iter()
                        .map(|span| render_span(span, self.styles))
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>();

        let block = activity_block(&surface, self.styles);
        let paragraph = Paragraph::new(lines).block(block);
        if matches!(surface.border, ActivityBorder::Plan) {
            paragraph.wrap(Wrap { trim: false }).render(area, buf);
        } else {
            paragraph.render(area, buf);
        }
    }
}

fn render_span<'a>(span: &'a ActivitySpan, styles: UiStyles<'_>) -> Span<'a> {
    let tone_color = match span.tone {
        ActivityTone::Text => styles.text(),
        ActivityTone::Dim => styles.dim(),
        ActivityTone::Accent => styles.accent(),
        ActivityTone::Running => styles.tool_running(),
        ActivityTone::Completed => styles.tool_completed(),
        ActivityTone::Error => styles.tool_error(),
        ActivityTone::Warning => styles.warning(),
    };
    // A per-agent badge color overrides the tone foreground (TS
    // `AgentProgressLine` parity).
    let color = span
        .color
        .map(crate::widgets::suggestion_popup::agent_color_to_ratatui)
        .unwrap_or(tone_color);
    // `Span::raw` accepts `Into<Cow<'a, str>>`; passing `&span.text`
    // lets the borrow flow through without copying the underlying
    // `Cow<'static, str>` storage.
    let rendered = Span::raw(span.text.as_ref()).fg(color);
    if span.bold { rendered.bold() } else { rendered }
}

fn activity_block(surface: &ActivitySurfaceView, styles: UiStyles<'_>) -> Block<'static> {
    let border_color = match surface.border {
        ActivityBorder::Plan => styles.focused_border(),
        ActivityBorder::Agents | ActivityBorder::Coordinator | ActivityBorder::Activity => {
            styles.border()
        }
    };
    Block::default()
        .borders(Borders::TOP)
        .title(activity_title(surface.title))
        .border_style(Style::default().fg(border_color))
}

fn activity_title(title: ActivityTitle) -> String {
    match title {
        ActivityTitle::Activity => format!(" {} ", t!("activity.title")),
        // The subagent panel drops its title word: the `Subagents (N) · …`
        // summary body line is the de-facto header, so the top border is a
        // bare rule and the label isn't repeated twice.
        ActivityTitle::Agents => String::new(),
        ActivityTitle::Coordinator => format!(" {} ", t!("coordinator.title")),
        ActivityTitle::Tasks => t!("plan_panel.title").to_string(),
    }
}
