//! Full-color `/context` usage surface.
//!
//! Consumes the runtime-analyzed [`coco_types::ContextUsageResult`] and paints
//! a colored usage grid, a per-category legend, source-grouped detail
//! sections, and actionable suggestions. Grid allocation + suggestion logic
//! are shared with the SDK text form via `coco_types::context_usage`; this
//! module owns only the color/glyph/layout choices.

use ratatui::prelude::Color;
use ratatui::prelude::Line;
use ratatui::prelude::Span;
use ratatui::style::Modifier;
use ratatui::style::Style;

use coco_types::ContextCategoryKind;
use coco_types::ContextUsageResult;
use coco_types::GridCell;
use coco_types::GridCellKind;
use coco_types::SuggestionSeverity;
use coco_types::build_grid;
use coco_types::fmt_token_compact;
use coco_types::group_by_source;

use coco_tui_ui::style::UiStyles;

const GLYPH_FULL: char = '\u{26C1}'; // ⛁ full content cell
const GLYPH_PARTIAL: char = '\u{26C0}'; // ⛀ partial boundary cell
const GLYPH_RESERVED: char = '\u{26DD}'; // ⛝ auto-compact reserved buffer
const GLYPH_FREE: char = '\u{26F6}'; // ⛶ free cell

/// Paint the full `/context` snapshot as styled scrollback lines: headline,
/// colored category grid, per-category legend, source-grouped Memory / MCP /
/// Agents / Skills detail, and suggestions. Printed inline in the transcript,
/// not a modal — so the whole block is visible in native scrollback with no
/// windowing. `cwd` shortens memory paths to project-relative form.
pub(crate) fn report_lines(
    report: &ContextUsageResult,
    styles: UiStyles<'_>,
    cwd: Option<&str>,
) -> Vec<Line<'static>> {
    let dim = Style::default().fg(styles.dim());
    let max = report.raw_max_tokens.max(1);
    let mut lines: Vec<Line<'static>> = Vec::new();

    // Headline: billed last-API usage (independent of the tiled estimates).
    lines.push(Line::from(Span::styled(
        format!(
            "{} · {}/{} tok ({:.0}%)",
            report.model,
            fmt_token_compact(report.total_tokens),
            fmt_token_compact(report.max_tokens),
            report.percentage,
        ),
        dim,
    )));
    lines.push(Line::default());

    // Colored, category-segmented usage grid.
    let cats: Vec<(ContextCategoryKind, i64)> = report
        .categories
        .iter()
        .filter(|c| c.kind != ContextCategoryKind::Free)
        .map(|c| (c.kind, c.tokens))
        .collect();
    let reserved = report
        .auto_compact_threshold
        .map(|t| (max - t).max(0))
        .unwrap_or(0);
    let (cols, rows) = if max >= 1_000_000 { (20, 10) } else { (10, 10) };
    let grid = build_grid(&cats, max, reserved, cols, rows);
    for row in grid.chunks(cols) {
        let spans: Vec<Span<'static>> = row
            .iter()
            .map(|cell| {
                let (glyph, color) = cell_glyph_color(cell, styles);
                Span::styled(format!("{glyph} "), Style::default().fg(color))
            })
            .collect();
        lines.push(Line::from(spans));
    }
    lines.push(Line::default());

    // Legend: one row per non-zero category, then reserved, then free.
    lines.push(Line::from(Span::styled(
        "Estimated usage by category".to_string(),
        dim.add_modifier(Modifier::ITALIC),
    )));
    let category_sum: i64 = cats.iter().map(|(_, t)| *t).sum();
    let free = (max - category_sum - reserved).max(0);
    for (kind, tokens) in &cats {
        if *tokens <= 0 {
            continue;
        }
        lines.push(legend_row(
            kind_color(*kind, styles),
            GLYPH_FULL,
            kind.label(),
            *tokens,
            max,
            /*with_unit*/ true,
            styles,
        ));
    }
    if reserved > 0 {
        lines.push(legend_row(
            styles.dim(),
            GLYPH_RESERVED,
            "Reserved (auto-compact)",
            reserved,
            max,
            /*with_unit*/ true,
            styles,
        ));
    }
    // Free row drops the unit word.
    lines.push(legend_row(
        styles.context_free(),
        GLYPH_FREE,
        "Free space",
        free,
        max,
        /*with_unit*/ false,
        styles,
    ));

    append_memory(&mut lines, report, styles, cwd);
    append_mcp(&mut lines, report, styles);
    append_agents(&mut lines, report, styles);
    append_skills(&mut lines, report, styles);
    append_suggestions(&mut lines, report, styles);

    lines
}

fn cell_glyph_color(cell: &GridCell, styles: UiStyles<'_>) -> (char, Color) {
    match cell.kind {
        GridCellKind::Category(kind) => {
            let glyph = if cell.fullness >= 0.7 {
                GLYPH_FULL
            } else {
                GLYPH_PARTIAL
            };
            (glyph, kind_color(kind, styles))
        }
        GridCellKind::Reserved => (GLYPH_RESERVED, styles.dim()),
        GridCellKind::Free => (GLYPH_FREE, styles.context_free()),
    }
}

/// Map a category to a theme color.
fn kind_color(kind: ContextCategoryKind, styles: UiStyles<'_>) -> Color {
    match kind {
        ContextCategoryKind::SystemPrompt => styles.primary(),
        ContextCategoryKind::Tools => styles.dim(),
        ContextCategoryKind::McpTools => styles.accent(),
        ContextCategoryKind::Agents => styles.plan(),
        ContextCategoryKind::MemoryFiles => styles.assistant_message(),
        ContextCategoryKind::Skills => styles.warning(),
        ContextCategoryKind::Messages => styles.thinking(),
        ContextCategoryKind::Free => styles.context_free(),
    }
}

#[allow(clippy::too_many_arguments)]
fn legend_row(
    color: Color,
    glyph: char,
    label: &str,
    tokens: i64,
    max: i64,
    with_unit: bool,
    styles: UiStyles<'_>,
) -> Line<'static> {
    let pct = tokens as f64 / max as f64 * 100.0;
    let value = if with_unit {
        format!("{} tok ({pct:.1}%)", fmt_token_compact(tokens))
    } else {
        format!("{} ({pct:.1}%)", fmt_token_compact(tokens))
    };
    Line::from(vec![
        Span::styled(glyph.to_string(), Style::default().fg(color)),
        Span::raw(format!(" {label}: ")),
        Span::styled(value, Style::default().fg(styles.dim())),
    ])
}

fn section_heading(name: &str, hint: &str, styles: UiStyles<'_>) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            name.to_string(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" · {hint}"), Style::default().fg(styles.dim())),
    ])
}

fn sub_heading(label: &str, styles: UiStyles<'_>) -> Line<'static> {
    Line::from(Span::styled(
        label.to_string(),
        Style::default().fg(styles.dim()),
    ))
}

/// Tree branch glyph: `├` for every item but the last, `└` for the last.
fn branch(idx: usize, len: usize) -> char {
    if idx + 1 == len { '└' } else { '├' }
}

/// `{branch} {name}: {value}` with the value dimmed. `value` is already
/// formatted (e.g. `"7.5k tok"` or `"~80 tok"`).
fn tree_row(branch: char, name: &str, value: String, styles: UiStyles<'_>) -> Line<'static> {
    Line::from(vec![
        Span::raw(format!("{branch} {name}: ")),
        Span::styled(value, Style::default().fg(styles.dim())),
    ])
}

/// Name-only branch row (deferred MCP tools carry no token estimate).
fn tree_row_name_only(branch: char, name: &str, styles: UiStyles<'_>) -> Line<'static> {
    Line::from(Span::styled(
        format!("{branch} {name}"),
        Style::default().fg(styles.dim()),
    ))
}

/// Exact figure `N tok` via the compact (`7.5k`) formatter — memory files
/// and MCP tools where the byte count is measured, not estimated.
fn exact_tokens(tokens: i64) -> String {
    format!("{} tok", fmt_token_compact(tokens))
}

/// Rough catalog estimate: `< 20 tok` below the floor, else `~N tok`
/// rounded to the nearest 10. Skill/agent sizes are approximations, so the
/// `~` signals that (vs. the exact memory/category figures).
fn estimate_tokens(tokens: i64) -> String {
    if tokens < 20 {
        return "< 20 tok".to_string();
    }
    let rounded = (tokens + 5) / 10 * 10;
    format!("~{} tok", fmt_token_compact(rounded))
}

fn append_memory(
    lines: &mut Vec<Line<'static>>,
    report: &ContextUsageResult,
    styles: UiStyles<'_>,
    cwd: Option<&str>,
) {
    if report.memory_files.is_empty() {
        return;
    }
    lines.push(Line::default());
    lines.push(section_heading("Memory files", "/memory", styles));
    let len = report.memory_files.len();
    for (i, m) in report.memory_files.iter().enumerate() {
        lines.push(tree_row(
            branch(i, len),
            &display_path(&m.path, cwd),
            exact_tokens(m.tokens),
            styles,
        ));
    }
}

fn append_mcp(lines: &mut Vec<Line<'static>>, report: &ContextUsageResult, styles: UiStyles<'_>) {
    if report.mcp_tools.is_empty() {
        return;
    }
    lines.push(Line::default());
    let deferred_any = report.mcp_tools.iter().any(|t| !t.is_loaded);
    let hint = if deferred_any {
        "/mcp (loaded on-demand)"
    } else {
        "/mcp"
    };
    lines.push(section_heading("MCP tools", hint, styles));
    if deferred_any {
        let loaded: Vec<_> = report.mcp_tools.iter().filter(|t| t.is_loaded).collect();
        if !loaded.is_empty() {
            lines.push(sub_heading("Loaded", styles));
            for (i, t) in loaded.iter().enumerate() {
                lines.push(tree_row(
                    branch(i, loaded.len()),
                    &t.name,
                    exact_tokens(t.tokens),
                    styles,
                ));
            }
        }
        let available: Vec<_> = report.mcp_tools.iter().filter(|t| !t.is_loaded).collect();
        if !available.is_empty() {
            lines.push(sub_heading("Available", styles));
            for (i, t) in available.iter().enumerate() {
                lines.push(tree_row_name_only(
                    branch(i, available.len()),
                    &t.name,
                    styles,
                ));
            }
        }
    } else {
        let len = report.mcp_tools.len();
        for (i, t) in report.mcp_tools.iter().enumerate() {
            lines.push(tree_row(
                branch(i, len),
                &t.name,
                exact_tokens(t.tokens),
                styles,
            ));
        }
    }
}

fn append_agents(
    lines: &mut Vec<Line<'static>>,
    report: &ContextUsageResult,
    styles: UiStyles<'_>,
) {
    if report.agents.is_empty() {
        return;
    }
    lines.push(Line::default());
    lines.push(section_heading("Custom agents", "/agents", styles));
    for (group, members) in group_by_source(&report.agents, |a| a.source.as_str(), |a| a.tokens) {
        lines.push(sub_heading(group.label(), styles));
        let len = members.len();
        for (i, a) in members.iter().enumerate() {
            lines.push(tree_row(
                branch(i, len),
                &a.agent_type,
                estimate_tokens(a.tokens),
                styles,
            ));
        }
    }
}

fn append_skills(
    lines: &mut Vec<Line<'static>>,
    report: &ContextUsageResult,
    styles: UiStyles<'_>,
) {
    if report.skills.is_empty() {
        return;
    }
    lines.push(Line::default());
    lines.push(section_heading("Skills", "/skills", styles));
    for (group, members) in group_by_source(&report.skills, |s| s.source.as_str(), |s| s.tokens) {
        lines.push(sub_heading(group.label(), styles));
        let len = members.len();
        for (i, s) in members.iter().enumerate() {
            lines.push(tree_row(
                branch(i, len),
                &s.name,
                estimate_tokens(s.tokens),
                styles,
            ));
        }
    }
}

fn append_suggestions(
    lines: &mut Vec<Line<'static>>,
    report: &ContextUsageResult,
    styles: UiStyles<'_>,
) {
    if report.suggestions.is_empty() {
        return;
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "Suggestions".to_string(),
        Style::default().add_modifier(Modifier::BOLD),
    )));
    for s in &report.suggestions {
        let (icon, color) = match s.severity {
            SuggestionSeverity::Warning => ('\u{26A0}', styles.warning()), // ⚠
            SuggestionSeverity::Info => ('\u{2139}', styles.accent()),     // ℹ
        };
        let mut spans = vec![
            Span::styled(format!("{icon} "), Style::default().fg(color)),
            Span::styled(
                s.title.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ];
        if let Some(t) = s.savings_tokens {
            spans.push(Span::styled(
                format!(" → save ~{}", fmt_token_compact(t)),
                Style::default().fg(styles.dim()),
            ));
        }
        lines.push(Line::from(spans));
        lines.push(Line::from(Span::styled(
            format!("  {}", s.detail),
            Style::default().fg(styles.dim()),
        )));
    }
}

/// Project-relative when the file is under `cwd`, else `~`-shortened for
/// files in `$HOME`, else the absolute path verbatim.
///
/// `cwd` is the threaded session working dir, but the native-scrollback
/// render path leaves it `None`, so fall back to the process cwd.
fn display_path(path: &str, cwd: Option<&str>) -> String {
    let resolved_cwd = cwd
        .filter(|c| !c.is_empty())
        .map(str::to_string)
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|p| p.to_string_lossy().into_owned())
        });
    if let Some(cwd) = resolved_cwd.as_deref().filter(|c| !c.is_empty())
        && let Some(rest) = path
            .strip_prefix(cwd.trim_end_matches('/'))
            .and_then(|rest| rest.strip_prefix('/'))
        && !rest.is_empty()
    {
        return rest.to_string();
    }
    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
        && let Some(rest) = path.strip_prefix(&home)
    {
        return format!("~{rest}");
    }
    path.to_string()
}

#[cfg(test)]
#[path = "context_view.test.rs"]
mod tests;
