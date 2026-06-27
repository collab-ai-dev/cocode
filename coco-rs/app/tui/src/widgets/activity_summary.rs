//! Recent-activity collapse + summary formatter.
//!
//! Algorithm: walk the activity list from the tail and count
//! consecutive search/read entries. When two or more such entries
//! trail the list, collapse them into "Searching for N patterns,
//! reading M files…". Otherwise walk backward for the most recent
//! `summary`, falling back to the last entry's `tool_name` when no
//! summary is present.
//!
//! DIVERGE: classify() only recognises the closed set of
//! canonical tool names (`Read`, `NotebookRead`, `Glob`, `Grep`).
//! Bash subcommands (`find`, `rg`, `ls`) and MCP tools are not
//! classified; non-canonical tools always return `Other` and break
//! the trailing run.

use coco_types::TaskActivity;

/// Category for collapse classification — there's no overlap (a tool is
/// search XOR read XOR other), so an enum is more honest than two booleans.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolCategory {
    Search,
    Read,
    Other,
}

/// Map a canonical tool name to its collapse category.
///
/// `NotebookRead` is treated as a read even though there is no explicit
/// `isSearchOrReadCommand` override — it's the obvious analogue and the
/// alternative is showing notebook reads as opaque blockers in the summary.
pub(crate) fn classify(tool_name: &str) -> ToolCategory {
    match tool_name {
        "Read" | "NotebookRead" => ToolCategory::Read,
        "Glob" | "Grep" => ToolCategory::Search,
        _ => ToolCategory::Other,
    }
}

/// Returns `None` when the list is empty. Returns the collapsed
/// summary when ≥2 trailing entries are search/read. Otherwise walks
/// backward for the most recent activity with a populated `summary`;
/// falls back to the last entry's `tool_name` only when no activity
/// carries a summary.
pub(crate) fn summarize_trailing(activities: &[TaskActivity]) -> Option<String> {
    if activities.is_empty() {
        return None;
    }
    let mut search_count: usize = 0;
    let mut read_count: usize = 0;
    for activity in activities.iter().rev() {
        match classify(&activity.tool_name) {
            ToolCategory::Search => search_count += 1,
            ToolCategory::Read => read_count += 1,
            ToolCategory::Other => break,
        }
    }
    if search_count + read_count >= 2 {
        return Some(format_active_summary(search_count, read_count));
    }
    activities
        .iter()
        .rev()
        .find_map(|a| a.summary.clone())
        .or_else(|| activities.last().map(|a| a.tool_name.clone()))
}

/// Formats the active-summary string for ≥2 trailing search/read activities.
/// coco-rs only renders this while a turn is running, never post-hoc.
pub(crate) fn format_active_summary(search_count: usize, read_count: usize) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(2);
    if search_count > 0 {
        let unit = if search_count == 1 {
            "pattern"
        } else {
            "patterns"
        };
        let verb = if parts.is_empty() {
            "Searching for"
        } else {
            "searching for"
        };
        parts.push(format!("{verb} {search_count} {unit}"));
    }
    if read_count > 0 {
        let unit = if read_count == 1 { "file" } else { "files" };
        let verb = if parts.is_empty() {
            "Reading"
        } else {
            "reading"
        };
        parts.push(format!("{verb} {read_count} {unit}"));
    }
    let text = parts.join(", ");
    format!("{text}…")
}

#[cfg(test)]
#[path = "activity_summary.test.rs"]
mod tests;
