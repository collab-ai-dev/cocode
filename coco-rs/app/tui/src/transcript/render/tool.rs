//! Tool-result cell renderer. Reads `tool_name`, `output`, and
//! `is_error` from `cell.source: Arc<Message::ToolResult>`. The
//! engine flow only emits ToolResult cells (success / error), so this
//! renderer doesn't need separate file-diff / rejected / canceled arms.

use std::sync::Arc;

use coco_messages::Message;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;

use super::CellsRenderer;
use crate::transcript::cells::CellKind;
use crate::transcript::cells::RenderedCell;
use crate::transcript::derive::tool_result_output;

/// User-facing operation name for a tool invocation. Agent calls lead with
/// their concrete subagent type so every transcript surface presents the same
/// identity; older calls without that input fall back to the tool name.
pub(crate) fn tool_header_display_name(
    tool_name: &str,
    source: &Arc<Message>,
    call_id: &str,
) -> String {
    if tool_name != coco_types::ToolName::Agent.as_str() {
        return tool_name.to_string();
    }
    crate::transcript::derive::extract_tool_call_input(source, call_id)
        .as_ref()
        .and_then(|input| input.get("subagent_type"))
        .and_then(serde_json::Value::as_str)
        .filter(|agent_type| !agent_type.is_empty())
        .unwrap_or(tool_name)
        .to_string()
}

/// Committed run summary shared by native history and the Ctrl+O reader.
pub(crate) fn agent_summary_line(
    styles: coco_tui_ui::style::UiStyles<'_>,
    summary: &crate::state::session::SubagentRunSummary,
) -> Line<'static> {
    use crate::presentation::activity::format_short_tokens;
    use crate::presentation::thinking::format_duration_seconds;

    let (glyph, tone) = if summary.succeeded {
        ("✓", styles.success())
    } else {
        ("✗", styles.error())
    };
    let mut parts: Vec<String> = Vec::new();
    if summary.tool_count > 0 {
        parts.push(format!("{} tools", summary.tool_count));
    }
    if summary.duration_ms > 0 {
        parts.push(format_duration_seconds(std::time::Duration::from_millis(
            summary.duration_ms.max(0) as u64,
        )));
    }
    if summary.input_tokens > 0 || summary.output_tokens > 0 {
        parts.push(format!(
            "↑{} ↓{}",
            format_short_tokens(summary.input_tokens),
            format_short_tokens(summary.output_tokens)
        ));
        if summary.input_tokens > 0 && summary.cache_read_tokens > 0 {
            let pct = (summary.cache_read_tokens * 100 / summary.input_tokens).clamp(0, 100);
            parts.push(format!("cache {pct}%"));
        }
    }
    if summary.cost_usd > 0.0 {
        parts.push(format!("${:.2}", summary.cost_usd));
    }
    Line::from(vec![
        Span::raw("  └ ").fg(tone),
        Span::raw(format!("{glyph} ")).fg(tone),
        Span::raw(parts.join(" · ")).style(styles.dim_style()),
    ])
}

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
        header.push(Span::raw(name_suffix).style(w.styles.dim_style()));
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
