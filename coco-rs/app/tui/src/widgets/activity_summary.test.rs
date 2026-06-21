use super::ToolCategory;
use super::classify;
use super::format_active_summary;
use super::summarize_trailing;
use coco_types::TaskActivity;
use pretty_assertions::assert_eq;

fn act(name: &str) -> TaskActivity {
    TaskActivity {
        tool_name: name.into(),
        summary: None,
    }
}

fn act_with_summary(name: &str, summary: &str) -> TaskActivity {
    TaskActivity {
        tool_name: name.into(),
        summary: Some(summary.into()),
    }
}

#[test]
fn classify_canonical_tools() {
    assert_eq!(classify("Read"), ToolCategory::Read);
    assert_eq!(classify("NotebookRead"), ToolCategory::Read);
    assert_eq!(classify("Glob"), ToolCategory::Search);
    assert_eq!(classify("Grep"), ToolCategory::Search);
    assert_eq!(classify("Bash"), ToolCategory::Other);
    assert_eq!(classify("Edit"), ToolCategory::Other);
    assert_eq!(classify(""), ToolCategory::Other);
}

#[test]
fn empty_activities_returns_none() {
    assert_eq!(summarize_trailing(&[]), None);
}

#[test]
fn single_search_activity_uses_tool_name_fallback() {
    let activities = vec![act("Grep")];
    // One trailing search isn't enough for the collapse threshold (>=2);
    // falls back to the activity's tool name.
    assert_eq!(summarize_trailing(&activities), Some("Grep".to_string()));
}

#[test]
fn two_trailing_reads_collapse() {
    let activities = vec![act("Read"), act("Read")];
    assert_eq!(
        summarize_trailing(&activities),
        Some("Reading 2 files…".to_string())
    );
}

#[test]
fn search_then_read_emits_both_segments() {
    let activities = vec![act("Grep"), act("Read"), act("Read")];
    assert_eq!(
        summarize_trailing(&activities),
        Some("Searching for 1 pattern, reading 2 files…".to_string())
    );
}

#[test]
fn fallback_prefers_summary_over_tool_name() {
    // Walks backward for the first activity with a `summary`; when
    // present it must beat the raw tool name.
    let activities = vec![act_with_summary("Bash", "git log -5")];
    assert_eq!(
        summarize_trailing(&activities),
        Some("git log -5".to_string())
    );
}

#[test]
fn fallback_walks_backward_for_summary() {
    // Two activities, only the older one has a summary. Walks from the
    // tail; first match wins, so the result is the older summary.
    let activities = vec![act_with_summary("Bash", "cargo test"), act("Edit")];
    assert_eq!(
        summarize_trailing(&activities),
        Some("cargo test".to_string())
    );
}

#[test]
fn fallback_picks_most_recent_summary_when_multiple() {
    let activities = vec![
        act_with_summary("Bash", "old"),
        act_with_summary("Edit", "new"),
    ];
    assert_eq!(summarize_trailing(&activities), Some("new".to_string()));
}

#[test]
fn other_tool_breaks_the_trailing_run() {
    // Bash sits between two Reads — only the trailing Read counts, and
    // a single Read isn't enough to collapse, so we fall back to the
    // most recent activity's name.
    let activities = vec![act("Read"), act("Bash"), act("Read")];
    assert_eq!(summarize_trailing(&activities), Some("Read".to_string()));
}

#[test]
fn three_searches_pluralize_pattern() {
    let activities = vec![act("Grep"), act("Glob"), act("Grep")];
    assert_eq!(
        summarize_trailing(&activities),
        Some("Searching for 3 patterns…".to_string())
    );
}

#[test]
fn format_summary_handles_singular_and_plural() {
    assert_eq!(format_active_summary(1, 0), "Searching for 1 pattern…");
    assert_eq!(format_active_summary(2, 0), "Searching for 2 patterns…");
    assert_eq!(format_active_summary(0, 1), "Reading 1 file…");
    assert_eq!(format_active_summary(0, 3), "Reading 3 files…");
    assert_eq!(
        format_active_summary(2, 4),
        "Searching for 2 patterns, reading 4 files…"
    );
}
