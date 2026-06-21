//! Tool-result cell renderer. Reads `tool_name`, `output`, and
//! `is_error` from `cell.source: Arc<Message::ToolResult>`. The
//! engine flow only emits ToolResult cells (success / error), so this
//! renderer doesn't need separate file-diff / rejected / canceled arms.

use coco_messages::Message;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;

use super::CellsRenderer;
use crate::transcript::cells::CellKind;
use crate::transcript::cells::RenderedCell;
use crate::transcript::derive::tool_result_output;

pub(super) fn try_render(
    w: &CellsRenderer<'_>,
    cell: &RenderedCell,
    lines: &mut Vec<Line<'static>>,
) -> Option<()> {
    let CellKind::ToolResult { .. } = &cell.kind else {
        return None;
    };
    let Message::ToolResult(tr) = cell.source.as_ref() else {
        return Some(());
    };
    let projection = tool_result_output(cell.source.as_ref())?;
    // Header row mirrors the invocation: the `●` glyph groups call+result and
    // its colour encodes status (red ⇒ error, green ⇒ completed).
    let (glyph_color, name_suffix) = if tr.is_error {
        (w.styles.tool_error(), ": ")
    } else {
        (w.styles.tool_completed(), "")
    };
    let mut header = vec![
        Span::raw("  ● ").fg(glyph_color),
        Span::raw(projection.tool_name.clone())
            .fg(w.styles.text())
            .bold(),
    ];
    if !name_suffix.is_empty() {
        header.push(Span::raw(name_suffix).fg(w.styles.dim()));
    }
    lines.push(Line::from(header));
    // Standalone cell path — no invocation cell here, so the tool input is
    // unavailable and input-derived views (diffs) degrade to output-only.
    super::tool_result::render_tool_result_body(
        &w.tool_result_ctx(),
        &projection.tool_name,
        None,
        &projection.output,
        projection.display_data,
        tr.is_error,
        lines,
    );
    Some(())
}
