use super::*;
use crate::server_request::ContextMemoryFile;
use crate::server_request::MessageBreakdown;
use crate::server_request::ToolTypeBreakdown;
use pretty_assertions::assert_eq;

fn report(percentage: f64, auto_compact: bool) -> ContextUsageResult {
    ContextUsageResult {
        total_tokens: 0,
        max_tokens: 200_000,
        raw_max_tokens: 200_000,
        percentage,
        model: "p/m".to_string(),
        categories: Vec::new(),
        is_auto_compact_enabled: auto_compact,
        auto_compact_threshold: None,
        message_breakdown: None,
        memory_files: Vec::new(),
        mcp_tools: Vec::new(),
        agents: Vec::new(),
        skills: Vec::new(),
        suggestions: Vec::new(),
    }
}

fn tool(name: &str, call: i64, result: i64) -> ToolTypeBreakdown {
    ToolTypeBreakdown {
        name: name.to_string(),
        call_tokens: call,
        result_tokens: result,
    }
}

#[test]
fn build_grid_tiles_exactly_and_places_reserved_last() {
    let cats = [
        (ContextCategoryKind::SystemPrompt, 20_000),
        (ContextCategoryKind::Messages, 20_000),
    ];
    let cells = build_grid(&cats, 200_000, 10_000, 10, 10);
    assert_eq!(cells.len(), 100, "grid must fill cols*rows exactly");
    // Reserved buffer (10k of 200k = 5 cells) sits at the very end.
    assert_eq!(cells.last().unwrap().kind, GridCellKind::Reserved);
    let reserved = cells
        .iter()
        .filter(|c| c.kind == GridCellKind::Reserved)
        .count();
    assert_eq!(reserved, 5);
    // Categories come first.
    assert_eq!(
        cells[0].kind,
        GridCellKind::Category(ContextCategoryKind::SystemPrompt)
    );
}

#[test]
fn build_grid_marks_fractional_boundary_cell() {
    // 5_000 / 200_000 * 100 = 2.5 squares → 3 cells, boundary cell at index 2.
    let cells = build_grid(&[(ContextCategoryKind::Skills, 5_000)], 200_000, 0, 10, 10);
    let cat: Vec<_> = cells
        .iter()
        .filter(|c| matches!(c.kind, GridCellKind::Category(_)))
        .collect();
    assert_eq!(cat.len(), 3);
    assert_eq!(cat[0].fullness, 1.0);
    assert_eq!(cat[1].fullness, 1.0);
    assert!(
        (cat[2].fullness - 0.5).abs() < 1e-9,
        "boundary cell half full"
    );
}

#[test]
fn group_by_source_orders_groups_and_sorts_within() {
    let rows = vec![
        ("plugin:a".to_string(), 100i64),
        ("project".to_string(), 50),
        ("user:/x".to_string(), 999),
        ("project".to_string(), 200),
    ];
    let grouped = group_by_source(&rows, |r| r.0.as_str(), |r| r.1);
    let order: Vec<_> = grouped.iter().map(|(g, _)| *g).collect();
    assert_eq!(
        order,
        vec![SourceGroup::Project, SourceGroup::User, SourceGroup::Plugin]
    );
    // Project group sorted tokens-desc.
    assert_eq!(grouped[0].1[0].1, 200);
    assert_eq!(grouped[0].1[1].1, 50);
}

#[test]
fn near_capacity_warns_with_compact_advice() {
    let mut r = report(85.0, true);
    let s = build_suggestions(&r);
    assert_eq!(s[0].severity, SuggestionSeverity::Warning);
    assert_eq!(s[0].title, "Context is 85% full");
    assert!(s[0].detail.contains("Autocompact will trigger soon"));

    r.is_auto_compact_enabled = false;
    let s = build_suggestions(&r);
    assert!(
        s.iter()
            .any(|x| x.detail.contains("Autocompact is disabled"))
    );
}

#[test]
fn large_bash_result_is_warning_with_savings() {
    let mut r = report(40.0, true);
    r.message_breakdown = Some(MessageBreakdown {
        tool_call_tokens: 0,
        tool_result_tokens: 0,
        attachment_tokens: 0,
        assistant_message_tokens: 0,
        user_message_tokens: 0,
        tool_calls_by_type: vec![tool("Bash", 2_000, 40_000)],
        attachments_by_type: Vec::new(),
    });
    let s = build_suggestions(&r);
    let bash = s
        .iter()
        .find(|x| x.title.starts_with("Bash results"))
        .unwrap();
    assert_eq!(bash.severity, SuggestionSeverity::Warning);
    assert_eq!(bash.savings_tokens, Some(21_000)); // (42k)*0.5
    assert!(bash.title.contains("42k tok (21%)"));
}

#[test]
fn read_bloat_triggers_when_below_large_band() {
    let mut r = report(40.0, true);
    // 12k result, 5 % of 200k = 6 % → bloat band, but combined < 15 %.
    r.message_breakdown = Some(MessageBreakdown {
        tool_call_tokens: 0,
        tool_result_tokens: 0,
        attachment_tokens: 0,
        assistant_message_tokens: 0,
        user_message_tokens: 0,
        tool_calls_by_type: vec![tool("Read", 500, 12_000)],
        attachments_by_type: Vec::new(),
    });
    let s = build_suggestions(&r);
    assert!(s.iter().any(|x| x.title.starts_with("File reads using")));
}

#[test]
fn memory_bloat_lists_largest_three() {
    let mut r = report(20.0, true);
    // 12k of 200k = 6% (≥5%) and ≥5k → bloat suggestion fires.
    r.memory_files = vec![
        ContextMemoryFile {
            path: "/a/CLAUDE.md".into(),
            source: "project".into(),
            tokens: 8_000,
        },
        ContextMemoryFile {
            path: "/b/MEMORY.md".into(),
            source: "user".into(),
            tokens: 4_000,
        },
    ];
    let s = build_suggestions(&r);
    let mem = s
        .iter()
        .find(|x| x.title.starts_with("Memory files using"))
        .unwrap();
    assert!(mem.detail.contains("CLAUDE.md (8k)"));
    assert_eq!(mem.savings_tokens, Some(3_600)); // 12k * 0.3
}

#[test]
fn fmt_token_compact_matches_ts_format_tokens() {
    assert_eq!(fmt_token_compact(506), "506");
    assert_eq!(fmt_token_compact(1_000), "1k");
    assert_eq!(fmt_token_compact(13_500), "13.5k");
    assert_eq!(fmt_token_compact(1_300_000), "1.3m");
}
