use super::*;
use crate::theme::Theme;
use coco_types::ContextAgent;
use coco_types::ContextMemoryFile;
use coco_types::ContextUsageCategory;
use coco_types::ContextUsageResult;
use coco_types::SuggestionSeverity;

fn flatten(lines: &[ratatui::prelude::Line<'static>]) -> String {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn cat(kind: ContextCategoryKind, tokens: i64) -> ContextUsageCategory {
    ContextUsageCategory { kind, tokens }
}

fn sample() -> ContextUsageResult {
    let mut r = ContextUsageResult {
        total_tokens: 50_000,
        max_tokens: 200_000,
        raw_max_tokens: 200_000,
        percentage: 25.0,
        model: "anthropic/claude".to_string(),
        categories: vec![
            cat(ContextCategoryKind::SystemPrompt, 5_000),
            cat(ContextCategoryKind::Tools, 3_000),
            cat(ContextCategoryKind::Free, 192_000),
        ],
        is_auto_compact_enabled: true,
        auto_compact_threshold: Some(180_000),
        message_breakdown: None,
        memory_files: vec![ContextMemoryFile {
            path: "/repo/CLAUDE.md".into(),
            source: "project".into(),
            tokens: 1_200,
        }],
        mcp_tools: Vec::new(),
        agents: vec![ContextAgent {
            agent_type: "reviewer".into(),
            source: "project".into(),
            tokens: 80,
        }],
        skills: Vec::new(),
        suggestions: Vec::new(),
    };
    r.suggestions = coco_types::build_suggestions(&r);
    r
}

#[test]
fn renders_headline_grid_legend_and_sections() {
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let report = sample();
    let lines = report_lines(&report, styles, Some("/repo"));
    let body = flatten(&lines);

    assert!(body.contains("anthropic/claude · 50k/200k tok (25%)"));
    assert!(body.contains(GLYPH_FULL)); // colored grid present
    assert!(body.contains("Estimated usage by category"));
    assert!(body.contains("System prompt: 5k tok"));
    // Free row drops the unit word.
    assert!(body.contains("Free space: "));
    // Source-affordance section headings.
    assert!(body.contains("Memory files · /memory"));
    // Path is shown project-relative to cwd:
    // `/repo/CLAUDE.md` under `/repo` collapses to `CLAUDE.md`.
    assert!(body.contains("└ CLAUDE.md: 1.2k tok"));
    assert!(body.contains("Custom agents · /agents"));
    // Agent/skill sizes are rough catalog estimates → `~N tok`.
    assert!(body.contains("└ reviewer: ~80 tok"));
}

#[test]
fn near_capacity_renders_a_suggestion() {
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let mut report = sample();
    report.percentage = 85.0;
    report.suggestions = coco_types::build_suggestions(&report);
    let lines = report_lines(&report, styles, None);
    let body = flatten(&lines);
    assert!(body.contains("Suggestions"));
    assert!(body.contains("Context is 85% full"));
    assert_eq!(report.suggestions[0].severity, SuggestionSeverity::Warning);
}

#[test]
fn estimate_tokens_floors_small_and_rounds_with_tilde() {
    assert_eq!(estimate_tokens(8), "< 20 tok");
    assert_eq!(estimate_tokens(19), "< 20 tok");
    assert_eq!(estimate_tokens(78), "~80 tok");
    assert_eq!(estimate_tokens(247), "~250 tok");
}

#[test]
fn display_path_prefers_cwd_relative() {
    assert_eq!(
        display_path("/repo/sub/CLAUDE.md", Some("/repo")),
        "sub/CLAUDE.md"
    );
    // Outside cwd falls through to the absolute path.
    assert_eq!(
        display_path("/other/AGENTS.md", Some("/repo")),
        "/other/AGENTS.md"
    );
}

#[test]
fn display_path_falls_back_to_process_cwd_when_unthreaded() {
    // The native-scrollback render path leaves `cwd` None; workspace
    // memory files must still collapse to a relative path via the process
    // cwd (regression: `/context` showed absolute memory paths).
    let abs = std::env::current_dir().unwrap().join("AGENTS.md");
    assert_eq!(display_path(&abs.to_string_lossy(), None), "AGENTS.md");
}
