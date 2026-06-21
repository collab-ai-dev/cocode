//! Shared presentation helpers for plan text.

use coco_tui_ui::display::SyntaxHighlighting;
use coco_tui_ui::style::UiStyles;
use ratatui::text::Line;
use ratatui::text::Span;

/// Render plan markdown with the same indentation/wrapping used by pending and
/// approved ExitPlanMode views.
pub(crate) fn render_plan_markdown(
    plan: &str,
    styles: UiStyles<'_>,
    width: u16,
    syntax_highlighting: SyntaxHighlighting,
) -> Vec<Line<'static>> {
    let width = width.saturating_sub(4).max(1);
    let opts = coco_tui_markdown::MarkdownOptions::new(styles, width, syntax_highlighting);
    indent2(coco_tui_markdown::render_markdown(plan, opts, None))
}

fn indent2(rendered: Vec<Line<'static>>) -> Vec<Line<'static>> {
    rendered
        .into_iter()
        .map(|mut line| {
            line.spans.insert(0, Span::raw("  "));
            line
        })
        .collect()
}
