use ratatui::text::Line;

use super::background_tasks_lines;
use super::copy_picker_lines;
use super::export_lines;
use super::global_search_lines;
use super::memory_dialog_lines;
use super::quick_open_lines;
use super::session_browser_lines;
use crate::i18n::locale_test_guard;
use crate::state::CopyPickerCodeBlock;
use crate::state::CopyPickerSelection;
use crate::state::CopyPickerState;
use crate::state::ExportFormat;
use crate::state::ExportState;
use crate::state::GlobalSearchState;
use crate::state::MemoryDialogEntry;
use crate::state::MemoryDialogRowKind;
use crate::state::MemoryDialogScope;
use crate::state::MemoryDialogState;
use crate::state::QuickOpenState;
use crate::state::SearchResult;
use crate::state::SessionBrowserState;
use crate::state::SessionOption;
use coco_tui_ui::style::UiStyles;
use coco_tui_ui::theme::Theme;

fn line_text(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

fn joined(lines: &[Line<'_>]) -> String {
    lines.iter().map(line_text).collect::<Vec<_>>().join("\n")
}

fn cursor_row<'a>(lines: &'a [Line<'a>], needle: &str) -> String {
    lines
        .iter()
        .map(line_text)
        .find(|t| t.contains(needle))
        .unwrap_or_else(|| panic!("row containing {needle:?} not found"))
}

#[test]
fn export_lines_mark_selected_format_with_cursor() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let state = ExportState {
        formats: vec![
            ExportFormat::Markdown,
            ExportFormat::Json,
            ExportFormat::Text,
        ],
        selected: 1,
    };

    let (title, lines, border) = export_lines(&state, styles, 60);
    let texts: Vec<String> = lines.iter().map(line_text).collect();
    let joined = texts.join("\n");

    assert_eq!(title, " Export Transcript ");
    assert_eq!(border, theme.primary);
    assert!(joined.contains("Select format:"), "{joined}");
    // The reusable select list renders a `❯` cursor on the selected row.
    let json_row = texts
        .iter()
        .find(|t| t.contains("JSON (.json)"))
        .expect("json row");
    assert!(
        json_row.starts_with("❯ "),
        "selected row missing cursor: {json_row}"
    );
    let md_row = texts
        .iter()
        .find(|t| t.contains("Markdown (.md)"))
        .expect("md row");
    assert!(
        md_row.starts_with("  "),
        "unselected row has cursor: {md_row}"
    );
}

#[test]
fn memory_lines_render_tags_cursor_and_empty_state() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);

    let (_, empty, _) = memory_dialog_lines(
        &MemoryDialogState {
            entries: Vec::new(),
            selected: 0,
        },
        styles,
        60,
    );
    assert!(joined(&empty).contains("No memory locations resolved."));

    let state = MemoryDialogState {
        entries: vec![
            MemoryDialogEntry {
                path: "CLAUDE.md".into(),
                label: "Project memory".to_string(),
                scope: MemoryDialogScope::Project,
                row_kind: MemoryDialogRowKind::File {
                    exists: true,
                    read_only: false,
                },
            },
            MemoryDialogEntry {
                path: ".claude/local/CLAUDE.md".into(),
                label: "Local memory".to_string(),
                scope: MemoryDialogScope::ProjectLocal,
                row_kind: MemoryDialogRowKind::File {
                    exists: false,
                    read_only: false,
                },
            },
        ],
        selected: 1,
    };
    let (_, lines, _) = memory_dialog_lines(&state, styles, 60);
    assert!(cursor_row(&lines, "Project memory").starts_with("  "));
    let sel = cursor_row(&lines, "Local memory");
    assert!(sel.starts_with("❯ "), "{sel}");
    assert!(sel.contains("[file:new] [project-local] Local memory"));
}

#[test]
fn quick_open_lines_window_to_budget_with_filter() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let state = QuickOpenState {
        filter: "src".to_string(),
        files: (0..20).map(|i| format!("file-{i}.rs")).collect(),
        selected: 2,
    };
    // Budget 10 → the widget windows the full 20-file list to 10 rows around
    // the selection (no pre-truncation), keeping the focused row visible.
    let (_, lines, _) = quick_open_lines(&state, styles, 10);
    let j = joined(&lines);
    assert!(j.contains("Open: src"));
    assert!(cursor_row(&lines, "file-2.rs").starts_with("❯ "));
    assert!(j.contains("file-9.rs"));
    assert!(
        !j.contains("file-10.rs"),
        "window should cap at the budget: {j}"
    );
}

#[test]
fn quick_open_lines_scroll_keeps_far_selection_cursored() {
    // Regression: a selection past the window must scroll into view, not
    // falsely highlight the last visible row.
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let state = QuickOpenState {
        filter: String::new(),
        files: (0..20).map(|i| format!("file-{i}.rs")).collect(),
        selected: 18,
    };
    let (_, lines, _) = quick_open_lines(&state, styles, 10);
    let j = joined(&lines);
    assert!(cursor_row(&lines, "file-18.rs").starts_with("❯ "), "{j}");
    assert!(
        !j.contains("file-2.rs"),
        "window should have scrolled past the start: {j}"
    );
}

#[test]
fn session_browser_lines_filter_and_empty() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);

    let (_, empty, _) = session_browser_lines(
        &SessionBrowserState {
            sessions: Vec::new(),
            filter: String::new(),
            selected: 0,
        },
        styles,
        60,
    );
    assert!(joined(&empty).contains("No saved sessions"));

    let state = SessionBrowserState {
        sessions: vec![SessionOption {
            id: "s1".to_string(),
            label: "Morning".to_string(),
            message_count: 7,
            created_at: "2026-05-14".to_string(),
        }],
        filter: String::new(),
        selected: 0,
    };
    let (_, lines, _) = session_browser_lines(&state, styles, 60);
    assert!(joined(&lines).contains("Type to filter sessions..."));
    assert!(cursor_row(&lines, "Morning").contains("Morning — 7 msgs — 2026-05-14"));
}

#[test]
fn global_search_lines_cap_results_and_states() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let state = GlobalSearchState {
        query: "needle".to_string(),
        results: (0..25)
            .map(|i| SearchResult {
                file: format!("src/file_{i}.rs"),
                line_number: i,
                content: format!("  result {i}  "),
            })
            .collect(),
        selected: 2,
        is_searching: false,
    };
    let (_, lines, _) = global_search_lines(&state, styles, 10);
    let j = joined(&lines);
    assert!(j.contains("Search: needle"));
    assert!(cursor_row(&lines, "src/file_2.rs:2 result 2").starts_with("❯ "));
    assert!(j.contains("src/file_9.rs"));
    assert!(
        !j.contains("src/file_10.rs"),
        "window should cap at the budget: {j}"
    );

    let (_, searching, _) = global_search_lines(
        &GlobalSearchState {
            results: Vec::new(),
            is_searching: true,
            ..state.clone()
        },
        styles,
        10,
    );
    assert!(joined(&searching).contains("Searching..."));
}

fn running_shell(id: &str, cmd: &str, started_at_ms: i64) -> crate::state::session::TaskEntry {
    crate::state::session::TaskEntry {
        task_id: id.to_string(),
        description: cmd.to_string(),
        status: crate::state::session::TaskEntryStatus::Running,
        kind: crate::state::session::TaskEntryKind::Shell,
        started_at_ms,
    }
}

fn running_agent(id: &str, desc: &str, started_at_ms: i64) -> crate::state::session::TaskEntry {
    crate::state::session::TaskEntry {
        task_id: id.to_string(),
        description: desc.to_string(),
        status: crate::state::session::TaskEntryStatus::Running,
        kind: crate::state::session::TaskEntryKind::Agent,
        started_at_ms,
    }
}

fn subagent_for(
    agent_id: &str,
    agent_type: &str,
    activities: Vec<coco_types::TaskActivity>,
) -> crate::state::SubagentInstance {
    crate::state::SubagentInstance {
        kind: crate::state::SubagentKind::Subagent,
        agent_id: agent_id.to_string(),
        agent_type: agent_type.to_string(),
        description: String::new(),
        status: crate::state::SubagentStatus::Running,
        color: Some(coco_types::AgentColorName::Blue),
        team_name: None,
        started_at_ms: Some(0),
        last_tool_name: None,
        tool_count: 0,
        total_tokens: 0,
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        is_backgrounded: false,
        recent_activities: activities,
        final_message: None,
        completed_at_ms: None,
        cost_usd: 0.0,
    }
}

#[test]
fn background_tasks_lines_group_sections_with_cursor_and_agent_type() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let mut state = crate::state::AppState::default();
    // Mixed start order: a shell first, then an agent. The grouped ordering
    // must surface the agent section first (and selection index 0 → agent).
    state.session.active_tasks = vec![
        running_shell("s1", "cargo test", 0),
        running_agent("a1", "map workspace crates", 0),
    ];
    state.session.subagents = vec![subagent_for(
        "a1",
        "Explore",
        vec![coco_types::TaskActivity {
            tool_name: "Grep".to_string(),
            summary: Some("3 patterns".to_string()),
        }],
    )];
    let bt = crate::state::BackgroundTasksState {
        selected: 0,
        detail: None,
    };

    let (title, lines, _) = background_tasks_lines(&bt, &state, styles, 40);
    let j = joined(&lines);

    assert_eq!(title, " Background tasks ");
    assert!(j.contains("Agents"), "missing agents section: {j}");
    assert!(j.contains("Shells"), "missing shells section: {j}");
    // Index 0 is the agent (grouped first); it carries the cursor + type badge.
    let agent_row = cursor_row(&lines, "map workspace crates");
    assert!(agent_row.starts_with("❯ "), "{agent_row}");
    assert!(agent_row.contains("Explore"), "{agent_row}");
    assert!(agent_row.contains("3 patterns"), "{agent_row}");
    // The shell row is unselected.
    assert!(cursor_row(&lines, "cargo test").starts_with("  "));
    assert!(j.contains("Enter to view"));
}

#[test]
fn background_tasks_detail_shell_shows_status_runtime_command() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    // Pin the clock so runtime is deterministic: 1h 19m 32s after start.
    let now_ms = (3600 + 19 * 60 + 32) * 1000;
    let mut state = crate::state::AppState::with_clock(coco_tui_ui::clock::MockClock::arc(now_ms));
    state.session.active_tasks = vec![running_shell("s1", "sleep 100", 0)];
    let bt = crate::state::BackgroundTasksState {
        selected: 0,
        detail: Some("s1".to_string()),
    };

    let (title, lines, _) = background_tasks_lines(&bt, &state, styles, 40);
    let j = joined(&lines);

    assert_eq!(title, " Shell details ");
    assert!(j.contains("Status:   running"), "{j}");
    assert!(j.contains("Runtime:  1h 19m 32s"), "{j}");
    assert!(j.contains("Command:  sleep 100"), "{j}");
    assert!(j.contains("No output available"), "{j}");
    assert!(j.contains("to go back"), "{j}");
}

#[test]
fn background_tasks_detail_agent_shows_live_activity() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let mut state = crate::state::AppState::default();
    state.session.active_tasks = vec![running_agent("a1", "summarize crates", 0)];
    state.session.subagents = vec![subagent_for(
        "a1",
        "Explore",
        vec![coco_types::TaskActivity {
            tool_name: "Read".to_string(),
            summary: Some("CLAUDE.md".to_string()),
        }],
    )];
    let bt = crate::state::BackgroundTasksState {
        selected: 0,
        detail: Some("a1".to_string()),
    };

    let (title, lines, _) = background_tasks_lines(&bt, &state, styles, 40);
    let j = joined(&lines);

    assert_eq!(title, " Agent details ");
    assert!(j.contains("Recent activity"), "{j}");
    assert!(j.contains("Read"), "{j}");
    assert!(j.contains("CLAUDE.md"), "{j}");
    // The agent detail replaces the shell "no output" placeholder.
    assert!(!j.contains("No output available"), "{j}");
}

#[test]
fn copy_picker_lines_map_enum_selection_to_cursor() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let state = CopyPickerState {
        full_text: "hello\nworld".to_string(),
        code_blocks: vec![CopyPickerCodeBlock {
            code: "let x = 1;".to_string(),
            lang: Some("rust".to_string()),
        }],
        message_age: 0,
        selected: CopyPickerSelection::CodeBlock(0),
    };
    let (_, lines, _) = copy_picker_lines(&state, styles, 60);
    // The focused row is the (only) code block, not Full / Always.
    let block_row = cursor_row(&lines, "let x = 1;");
    assert!(block_row.starts_with("❯ "), "{block_row}");
}
