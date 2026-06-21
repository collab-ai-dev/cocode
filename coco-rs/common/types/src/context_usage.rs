//! Shared `/context` data + presentation-neutral logic.
//!
//! This module owns the closed category vocabulary, the usage-grid
//! allocation, and the actionable-suggestion heuristics. It is the single
//! home consumed by both the SDK/headless text renderer
//! (`coco_query::context_analysis::format_markdown`) and the TUI styled
//! surface (`coco_tui::presentation::context_view`) so the two never drift.

use serde::Deserialize;
use serde::Serialize;

use crate::server_request::ContextUsageResult;

/// Closed set of context-window usage categories. Each renderer maps the
/// kind to its own label + color; the wire carries the typed kind so the
/// vocabulary cannot drift across SDK / TUI / cross-language codegens.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextCategoryKind {
    SystemPrompt,
    Tools,
    McpTools,
    Agents,
    MemoryFiles,
    Skills,
    Messages,
    Free,
}

impl ContextCategoryKind {
    /// Canonical label (SDK / headless text form).
    pub fn label(self) -> &'static str {
        match self {
            Self::SystemPrompt => "System prompt",
            Self::Tools => "System tools",
            Self::McpTools => "MCP tools",
            Self::Agents => "Custom agents",
            Self::MemoryFiles => "Memory files",
            Self::Skills => "Skills",
            Self::Messages => "Messages",
            Self::Free => "Free space",
        }
    }
}

/// What a single usage-grid cell represents. Color/glyph selection lives in
/// each renderer; this is the presentation-neutral classification.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GridCellKind {
    Category(ContextCategoryKind),
    /// Auto-compact / manual-compact reserved buffer.
    Reserved,
    Free,
}

/// One cell of the usage grid. `fullness` is the 0..=1 fractional fill of
/// the boundary cell between a category's whole squares and the next
/// category (TS `squareFullness`); whole cells are `1.0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridCell {
    pub kind: GridCellKind,
    pub fullness: f64,
}

/// Build the usage grid as a flat `cols * rows` cell vector laid out
/// category-first, then free padding, then the reserved buffer at the end —
/// matching TS `analyzeContext.ts` grid assembly. The renderer chunks the
/// result into rows of `cols`.
pub fn build_grid(
    categories: &[(ContextCategoryKind, i64)],
    raw_max: i64,
    reserved: i64,
    cols: usize,
    rows: usize,
) -> Vec<GridCell> {
    let total = cols.saturating_mul(rows);
    let max = raw_max.max(1) as f64;
    let mut cells: Vec<GridCell> = Vec::with_capacity(total);

    for &(kind, tokens) in categories {
        if tokens <= 0 {
            continue;
        }
        for cell in category_cells(kind, tokens, max, total) {
            if cells.len() >= total {
                break;
            }
            cells.push(cell);
        }
    }

    // Reserved squares are placed at the very end; free space fills the gap.
    let reserved_squares = if reserved > 0 {
        let exact = (reserved as f64 / max) * total as f64;
        (exact.round() as i64).max(1) as usize
    } else {
        0
    };
    let free_target = total.saturating_sub(reserved_squares).max(cells.len());

    while cells.len() < free_target {
        cells.push(GridCell {
            kind: GridCellKind::Free,
            fullness: 1.0,
        });
    }
    while cells.len() < total {
        cells.push(GridCell {
            kind: GridCellKind::Reserved,
            fullness: 1.0,
        });
    }
    cells.truncate(total);
    cells
}

/// One category's contiguous cells: `max(1, round(ratio))` squares with the
/// fractional boundary square (index == whole squares) carrying the leftover
/// fill — mirrors TS `createCategorySquares`.
fn category_cells(kind: ContextCategoryKind, tokens: i64, max: f64, total: usize) -> Vec<GridCell> {
    let exact = (tokens as f64 / max) * total as f64;
    let whole = exact.floor();
    let frac = exact - whole;
    let squares = (exact.round() as i64).max(1) as usize;
    (0..squares)
        .map(|i| GridCell {
            kind: GridCellKind::Category(kind),
            fullness: if (i as f64) == whole && frac > 0.0 {
                frac
            } else {
                1.0
            },
        })
        .collect()
}

/// Display group for source-grouped detail sections (agents, skills). Fixed
/// render order mirrors TS `SOURCE_DISPLAY_ORDER`, extended with `Mcp` for
/// coco-rs MCP-sourced skills.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceGroup {
    Project,
    User,
    Managed,
    Plugin,
    BuiltIn,
    Mcp,
}

impl SourceGroup {
    pub const ORDER: [SourceGroup; 6] = [
        Self::Project,
        Self::User,
        Self::Managed,
        Self::Plugin,
        Self::BuiltIn,
        Self::Mcp,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Project => "Project",
            Self::User => "User",
            Self::Managed => "Managed",
            Self::Plugin => "Plugin",
            Self::BuiltIn => "Built-in",
            Self::Mcp => "MCP",
        }
    }
}

/// Map a free-form source string (memory `project_config`, skill
/// `user:/path`, agent `built_in`, …) onto a display group. Unknown sources
/// fall back to `BuiltIn` rather than being silently dropped (TS drops
/// `Local`/`Flag`; coco-rs keeps every row visible).
pub fn source_group(source: &str) -> SourceGroup {
    let s = source.to_ascii_lowercase();
    if s.starts_with("project") {
        SourceGroup::Project
    } else if s.starts_with("user") || s.starts_with("local") {
        SourceGroup::User
    } else if s.starts_with("managed") || s.starts_with("policy") {
        SourceGroup::Managed
    } else if s.starts_with("plugin") {
        SourceGroup::Plugin
    } else if s.starts_with("mcp") {
        SourceGroup::Mcp
    } else {
        SourceGroup::BuiltIn
    }
}

/// Group items by source in `SourceGroup::ORDER`, sorted tokens-descending
/// within each group. Empty groups are omitted. Returns borrowed slices so
/// the caller renders without cloning.
pub fn group_by_source<T>(
    items: &[T],
    source_of: impl Fn(&T) -> &str,
    tokens_of: impl Fn(&T) -> i64,
) -> Vec<(SourceGroup, Vec<&T>)> {
    let mut out = Vec::new();
    for group in SourceGroup::ORDER {
        let mut members: Vec<&T> = items
            .iter()
            .filter(|item| source_group(source_of(item)) == group)
            .collect();
        if members.is_empty() {
            continue;
        }
        members.sort_by_key(|m| std::cmp::Reverse(tokens_of(m)));
        out.push((group, members));
    }
    out
}

/// Severity of a context suggestion. `Warning` sorts before `Info`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionSeverity {
    Warning,
    Info,
}

/// An actionable suggestion shown under the `/context` view.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextSuggestion {
    pub severity: SuggestionSeverity,
    pub title: String,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub savings_tokens: Option<i64>,
}

// Thresholds mirror TS `utils/contextSuggestions.ts` constants verbatim.
const LARGE_TOOL_RESULT_PERCENT: f64 = 15.0;
const LARGE_TOOL_RESULT_TOKENS: i64 = 10_000;
const READ_BLOAT_PERCENT: f64 = 5.0;
const NEAR_CAPACITY_PERCENT: f64 = 80.0;
const MEMORY_HIGH_PERCENT: f64 = 5.0;
const MEMORY_HIGH_TOKENS: i64 = 5_000;

/// Compute actionable suggestions from a usage report. Pure over the wire
/// result so both SDK and TUI render the same guidance. Sorted
/// warnings-first, then by savings descending.
pub fn build_suggestions(report: &ContextUsageResult) -> Vec<ContextSuggestion> {
    let raw_max = report.raw_max_tokens.max(1);
    let mut out = Vec::new();

    check_near_capacity(report, &mut out);
    check_large_tool_results(report, raw_max, &mut out);
    check_read_result_bloat(report, raw_max, &mut out);
    check_memory_bloat(report, raw_max, &mut out);
    check_autocompact_disabled(report, &mut out);

    out.sort_by(|a, b| {
        let sev = severity_rank(a.severity).cmp(&severity_rank(b.severity));
        sev.then(
            b.savings_tokens
                .unwrap_or(0)
                .cmp(&a.savings_tokens.unwrap_or(0)),
        )
    });
    out
}

fn severity_rank(s: SuggestionSeverity) -> u8 {
    match s {
        SuggestionSeverity::Warning => 0,
        SuggestionSeverity::Info => 1,
    }
}

fn pct_of(part: i64, whole: i64) -> f64 {
    part as f64 / whole.max(1) as f64 * 100.0
}

fn check_near_capacity(report: &ContextUsageResult, out: &mut Vec<ContextSuggestion>) {
    if report.percentage < NEAR_CAPACITY_PERCENT {
        return;
    }
    let detail = if report.is_auto_compact_enabled {
        "Autocompact will trigger soon, which discards older messages. Use /compact now to control what gets kept."
    } else {
        "Autocompact is disabled. Use /compact to free space, or enable autocompact in /config."
    };
    out.push(ContextSuggestion {
        severity: SuggestionSeverity::Warning,
        title: format!("Context is {:.0}% full", report.percentage),
        detail: detail.to_string(),
        savings_tokens: None,
    });
}

fn check_large_tool_results(
    report: &ContextUsageResult,
    raw_max: i64,
    out: &mut Vec<ContextSuggestion>,
) {
    let Some(breakdown) = &report.message_breakdown else {
        return;
    };
    for tool in &breakdown.tool_calls_by_type {
        let total = tool.call_tokens + tool.result_tokens;
        let pct = pct_of(total, raw_max);
        if pct < LARGE_TOOL_RESULT_PERCENT || total < LARGE_TOOL_RESULT_TOKENS {
            continue;
        }
        let tokens = fmt_token_compact(total);
        let (severity, savings_mult, detail) = match tool.name.as_str() {
            "Bash" => (
                SuggestionSeverity::Warning,
                0.5,
                "Pipe output through head, tail, or grep to reduce result size. Avoid cat on large files — use Read with offset/limit instead.",
            ),
            "Read" => (
                SuggestionSeverity::Info,
                0.3,
                "Use offset and limit parameters to read only the sections you need. Avoid re-reading entire files when you only need a few lines.",
            ),
            "Grep" => (
                SuggestionSeverity::Info,
                0.3,
                "Add more specific patterns or use the glob or type parameter to narrow file types. Consider Glob for file discovery instead of Grep.",
            ),
            "WebFetch" => (
                SuggestionSeverity::Info,
                0.4,
                "Web page content can be very large. Consider extracting only the specific information needed.",
            ),
            _ => {
                if pct < 20.0 {
                    continue;
                }
                out.push(ContextSuggestion {
                    severity: SuggestionSeverity::Info,
                    title: format!("{} using {tokens} tok ({pct:.0}%)", tool.name),
                    detail: "This tool is consuming a significant portion of context.".to_string(),
                    savings_tokens: Some((total as f64 * 0.2) as i64),
                });
                continue;
            }
        };
        out.push(ContextSuggestion {
            severity,
            title: format!("{} results using {tokens} tok ({pct:.0}%)", tool.name),
            detail: detail.to_string(),
            savings_tokens: Some((total as f64 * savings_mult) as i64),
        });
    }
}

fn check_read_result_bloat(
    report: &ContextUsageResult,
    raw_max: i64,
    out: &mut Vec<ContextSuggestion>,
) {
    let Some(breakdown) = &report.message_breakdown else {
        return;
    };
    let Some(read) = breakdown
        .tool_calls_by_type
        .iter()
        .find(|t| t.name == "Read")
    else {
        return;
    };
    // Skip if Read already triggered the large-result band above.
    let combined = read.call_tokens + read.result_tokens;
    if pct_of(combined, raw_max) >= LARGE_TOOL_RESULT_PERCENT
        && combined >= LARGE_TOOL_RESULT_TOKENS
    {
        return;
    }
    let result = read.result_tokens;
    let pct = pct_of(result, raw_max);
    if pct < READ_BLOAT_PERCENT || result < LARGE_TOOL_RESULT_TOKENS {
        return;
    }
    out.push(ContextSuggestion {
        severity: SuggestionSeverity::Info,
        title: format!("File reads using {} tok ({pct:.0}%)", fmt_token_compact(result)),
        detail: "If you are re-reading files, consider referencing earlier reads. Use offset/limit for large files.".to_string(),
        savings_tokens: Some((result as f64 * 0.3) as i64),
    });
}

fn check_memory_bloat(report: &ContextUsageResult, raw_max: i64, out: &mut Vec<ContextSuggestion>) {
    if report.memory_files.is_empty() {
        return;
    }
    let total: i64 = report.memory_files.iter().map(|m| m.tokens).sum();
    let pct = pct_of(total, raw_max);
    if pct < MEMORY_HIGH_PERCENT || total < MEMORY_HIGH_TOKENS {
        return;
    }
    let mut largest: Vec<_> = report.memory_files.iter().collect();
    largest.sort_by_key(|m| std::cmp::Reverse(m.tokens));
    let top = largest
        .iter()
        .take(3)
        .map(|m| format!("{} ({})", short_name(&m.path), fmt_token_compact(m.tokens)))
        .collect::<Vec<_>>()
        .join(", ");
    out.push(ContextSuggestion {
        severity: SuggestionSeverity::Info,
        title: format!(
            "Memory files using {} tok ({pct:.0}%)",
            fmt_token_compact(total)
        ),
        detail: format!("Largest: {top}. Use /memory to review and prune stale entries."),
        savings_tokens: Some((total as f64 * 0.3) as i64),
    });
}

fn check_autocompact_disabled(report: &ContextUsageResult, out: &mut Vec<ContextSuggestion>) {
    if report.is_auto_compact_enabled {
        return;
    }
    if !(50.0..NEAR_CAPACITY_PERCENT).contains(&report.percentage) {
        return;
    }
    out.push(ContextSuggestion {
        severity: SuggestionSeverity::Info,
        title: "Autocompact is disabled".to_string(),
        detail: "Without autocompact, you will hit context limits and lose the conversation. Enable it in /config or use /compact manually.".to_string(),
        savings_tokens: None,
    });
}

/// Last path component, for compact memory-file display in suggestions.
fn short_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// Compact token count with a `k`/`m` suffix (one decimal, trailing `.0`
/// dropped): `506 → "506"`, `13_500 → "13.5k"`, `1_300_000 → "1.3m"`.
/// Shared by the SDK text form and the TUI.
pub fn fmt_token_compact(n: i64) -> String {
    if n < 1000 {
        n.to_string()
    } else if n < 1_000_000 {
        format!("{}k", trim_dot_zero(n as f64 / 1000.0))
    } else {
        format!("{}m", trim_dot_zero(n as f64 / 1_000_000.0))
    }
}

fn trim_dot_zero(value: f64) -> String {
    let s = format!("{value:.1}");
    s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
}

#[cfg(test)]
#[path = "context_usage.test.rs"]
mod tests;
