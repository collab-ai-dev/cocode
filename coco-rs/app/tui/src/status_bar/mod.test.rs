use super::*;

use std::sync::Arc;

use coco_messages::AssistantContent;
use coco_messages::ReasoningContent;
use coco_messages::TextContent;
use coco_messages::create_assistant_message;
use coco_types::ModelRole;

use crate::i18n::locale_test_guard;
use crate::state::AppState;
use crate::state::session::TaskEntry;
use crate::state::session::TaskEntryKind;
use crate::state::session::TaskEntryStatus;
use crate::transcript::derive::test_helpers;

fn running_task(task_id: &str, kind: TaskEntryKind) -> TaskEntry {
    TaskEntry {
        task_id: task_id.into(),
        description: task_id.into(),
        status: TaskEntryStatus::Running,
        kind,
        started_at_ms: 0,
        workflow_name: None,
        workflow_progress: Vec::new(),
    }
}

#[test]
fn status_bar_view_renders_model_tokens_context_and_messages() {
    let _locale = locale_test_guard("en");
    let mut state = AppState::default();
    state.session.provider = "openai".into();
    state.session.model = "gpt-5.4".into();
    state.session.thinking_effort = coco_types::ReasoningEffort::High;
    state.session.session_usage = Some(coco_types::SessionUsageSnapshot {
        totals: coco_types::SessionUsageTotals {
            input_tokens: 1_500,
            output_tokens: 250,
            cache_read_input_tokens: 750,
            input_cost_usd: 0.0030,
            output_cost_usd: 0.0030,
            cache_read_cost_usd: 0.0012,
            total_cost_usd: 0.0072,
            ..Default::default()
        },
        ..Default::default()
    });
    state.session.token_usage.input_tokens = 1_500;
    state.session.token_usage.output_tokens = 250;
    state.session.token_usage.cache_read_tokens = 750;
    state.session.model_by_role.insert(
        ModelRole::Main,
        crate::state::ModelBinding {
            provider: "openai".into(),
            model_id: "gpt-5.4".into(),
            context_window: Some(100),
            effort: None,
        },
    );
    state
        .session
        .transcript
        .on_message_appended(Arc::new(create_assistant_message(
            vec![AssistantContent::Text(TextContent {
                text: "done".into(),
                provider_metadata: None,
            })],
            "gpt-5.4",
            coco_types::TokenUsage {
                input_tokens: coco_types::InputTokens {
                    total: 70,
                    ..Default::default()
                },
                output_tokens: coco_types::OutputTokens {
                    total: 10,
                    ..Default::default()
                },
            },
        )));

    let StatusBarView::BuiltIn { lines } = status_bar_view(&state) else {
        panic!("expected built-in status bar");
    };
    let spans: Vec<&StatusSpan> = lines.iter().flatten().collect();
    let text = spans
        .iter()
        .map(|span| span.text.as_str())
        .collect::<String>();

    assert!(text.contains(" openai/gpt-5.4"));
    assert!(text.contains("high * ctrl+t to cycle"));
    assert!(text.contains("↑1.5K/$0.0042 ↓250/$0.0030"));
    assert!(text.contains("cache 750/50.0%"));
    assert!(!text.contains("F2 to expand"));
    assert!(text.contains("ctx 80%"));
    assert!(text.contains("→0 ←1"));
    assert!(
        spans
            .iter()
            .any(|span| span.text == "ctx 80%" && span.tone == StatusTone::Warning)
    );
}

#[test]
fn status_bar_splits_permission_pill_and_directory_onto_dynamic_lines() {
    use coco_types::PermissionMode;

    let _locale = locale_test_guard("en");
    let mut state = AppState::default();
    state.session.provider = "openai".into();
    state.session.model = "gpt-5.4".into();
    state.session.permission_mode = PermissionMode::Auto;
    state.session.working_dir = Some("/home/user/codex".into());
    state.session.git_branch = Some("feat/automode".into());
    state.session.active_tasks = vec![
        running_task("a1", TaskEntryKind::Agent),
        running_task("s1", TaskEntryKind::Shell),
    ];

    assert_eq!(status_bar_height(&state), 3);
    let StatusBarView::BuiltIn { lines } = status_bar_view(&state) else {
        panic!("expected built-in status bar");
    };
    assert_eq!(lines.len(), 3);

    let line_text = |i: usize| {
        lines[i]
            .iter()
            .map(|span| span.text.as_str())
            .collect::<String>()
    };
    // Line 1 no longer carries the permission segment.
    assert!(line_text(0).contains("openai/gpt-5.4"));
    assert!(!line_text(0).contains("auto mode on"));
    // Line 2: permission symbol + label, then the TS-style pill.
    assert!(line_text(1).contains("▸▸ auto mode on"));
    assert!(line_text(1).contains("1 agent · 1 shell"));
    // Line 3: directory basename + git branch (zsh-prompt style).
    assert_eq!(line_text(2), " codex git:(feat/automode)");
}

#[test]
fn background_pill_label_counts_workflows() {
    let _locale = locale_test_guard("en");
    let mut state = AppState::default();
    state.session.active_tasks = vec![
        running_task("a1", TaskEntryKind::Agent),
        running_task("s1", TaskEntryKind::Shell),
        running_task("wf1", TaskEntryKind::Workflow),
        running_task("wf2", TaskEntryKind::Workflow),
    ];

    assert_eq!(
        background_pill_label(&state),
        Some("1 agent · 1 shell · 2 workflows".to_string())
    );
}

#[test]
fn background_pill_label_localizes_workflows() {
    let _locale = locale_test_guard("zh-CN");
    let mut state = AppState::default();
    state.session.active_tasks = vec![running_task("wf1", TaskEntryKind::Workflow)];

    assert_eq!(
        background_pill_label(&state),
        Some("1 个工作流".to_string())
    );
}

#[test]
fn status_bar_view_renders_active_goal_badge() {
    let _locale = locale_test_guard("en");
    let mut state = AppState::default();
    state.session.active_goal = Some(coco_types::ActiveGoal {
        condition: "finish tests".into(),
        iterations: 0,
        set_at_ms: 0,
        tokens_at_start: 0,
        last_reason: None,
    });

    let StatusBarView::BuiltIn { lines } = status_bar_view(&state) else {
        panic!("expected built-in status bar");
    };
    let text = lines
        .iter()
        .flatten()
        .map(|span| span.text.as_str())
        .collect::<String>();

    assert!(text.contains("/goal active"));
}

#[test]
fn status_bar_surfaces_ask_mode_and_cycle_hint_in_default_state() {
    let _locale = locale_test_guard("en");
    let state = AppState::default();
    // Model line + the baseline permission line. No working dir → no line 3.
    assert_eq!(status_bar_height(&state), 2);
    let StatusBarView::BuiltIn { lines } = status_bar_view(&state) else {
        panic!("expected built-in status bar");
    };
    assert_eq!(lines.len(), 2);
    let line2 = lines[1]
        .iter()
        .map(|span| span.text.as_str())
        .collect::<String>();
    // Baseline mode is surfaced as `⏯ ask mode on` (play glyph, like other
    // modes) plus the `·`-separated cycle hint shown uniformly across modes.
    assert_eq!(line2, " ⏯ ask mode on · shift+tab to cycle");
}

#[test]
fn status_bar_view_renders_lsp_badge() {
    let _locale = locale_test_guard("en");
    let mut state = AppState::default();
    state.session.lsp_active = true;

    let StatusBarView::BuiltIn { lines } = status_bar_view(&state) else {
        panic!("expected built-in status bar");
    };
    let spans: Vec<&StatusSpan> = lines.iter().flatten().collect();
    let text = spans
        .iter()
        .map(|span| span.text.as_str())
        .collect::<String>();

    assert!(text.contains("LSP"));
}

#[test]
fn status_bar_view_renders_unknown_context_without_assistant_usage() {
    let _locale = locale_test_guard("en");
    let state = AppState::default();

    let StatusBarView::BuiltIn { lines } = status_bar_view(&state) else {
        panic!("expected built-in status bar");
    };
    let spans: Vec<&StatusSpan> = lines.iter().flatten().collect();
    let text = spans
        .iter()
        .map(|span| span.text.as_str())
        .collect::<String>();

    assert!(text.contains("ctx --"));
}

#[test]
fn status_bar_view_renders_zero_cache_percent_without_decimal() {
    let _locale = locale_test_guard("en");
    let state = AppState::default();

    let StatusBarView::BuiltIn { lines } = status_bar_view(&state) else {
        panic!("expected built-in status bar");
    };
    let text = lines
        .iter()
        .flatten()
        .map(|span| span.text.as_str())
        .collect::<String>();

    assert!(text.contains("↑0 ↓0 · cache 0/0%"));
    assert!(!text.contains("cache 0/0.0%"));
}

#[test]
fn status_bar_view_renders_total_input_tokens_and_cache_breakdown() {
    let _locale = locale_test_guard("en");
    let mut state = AppState::default();
    state.session.token_usage.input_tokens = 5_020_000;
    state.session.token_usage.output_tokens = 14_800;
    state.session.token_usage.cache_read_tokens = 4_600_000;

    let StatusBarView::BuiltIn { lines } = status_bar_view(&state) else {
        panic!("expected built-in status bar");
    };
    let spans: Vec<&StatusSpan> = lines.iter().flatten().collect();
    let text = spans
        .iter()
        .map(|span| span.text.as_str())
        .collect::<String>();

    assert!(text.contains("↑5.0M ↓14.8K · cache 4.6M/91.6%"));
}

#[test]
fn status_bar_view_counts_transcript_messages_by_uuid_and_role() {
    let _locale = locale_test_guard("en");
    let mut state = AppState::default();
    test_helpers::push_user_text(&mut state.session, "u1", "hello");
    let assistant = create_assistant_message(
        vec![
            AssistantContent::Reasoning(ReasoningContent::new("thinking")),
            AssistantContent::Text(TextContent::new("answer")),
        ],
        "test-model",
        coco_types::TokenUsage::default(),
    );
    state
        .session
        .transcript
        .on_message_appended(Arc::new(assistant));
    test_helpers::push_tool_result(&mut state.session, "call-1", "Glob", "done", false);

    let StatusBarView::BuiltIn { lines } = status_bar_view(&state) else {
        panic!("expected built-in status bar");
    };
    let spans: Vec<&StatusSpan> = lines.iter().flatten().collect();
    let text = spans
        .iter()
        .map(|span| span.text.as_str())
        .collect::<String>();

    assert!(text.contains("→1 ←1 · tool 1"));
}

#[test]
fn status_bar_view_counts_survive_compact_history_replace() {
    let _locale = locale_test_guard("en");
    let mut state = AppState::default();
    test_helpers::push_user_text(&mut state.session, "u1", "hello");
    test_helpers::push_assistant_text(&mut state.session, "answer");

    // Compaction replaces the transcript wholesale; the status bar
    // counts are session-cumulative and must not collapse — the
    // summarizer call itself counts as one assistant message.
    state.session.transcript.replace_from_messages(
        &[Arc::new(coco_messages::create_user_message_with_uuid(
            uuid::Uuid::new_v4(),
            "This session is being continued…",
        ))],
        coco_types::HistoryReplaceReason::Compact,
    );

    let StatusBarView::BuiltIn { lines } = status_bar_view(&state) else {
        panic!("expected built-in status bar");
    };
    let text = lines
        .iter()
        .flatten()
        .map(|span| span.text.as_str())
        .collect::<String>();

    assert!(text.contains("→1 ←2"), "text: {text}");
}

#[test]
fn status_bar_view_renders_subagent_usage_segment_only_when_active() {
    let _locale = locale_test_guard("en");
    let mut state = AppState::default();

    let render = |state: &AppState| {
        let StatusBarView::BuiltIn { lines } = status_bar_view(state) else {
            panic!("expected built-in status bar");
        };
        lines
            .iter()
            .flatten()
            .map(|span| span.text.as_str())
            .collect::<String>()
    };

    assert!(
        !render(&state).contains("subagents"),
        "segment hidden while no subagent has reported usage"
    );

    state.session.subagent_usage = crate::state::session::SubagentUsageTotals {
        input_tokens: 68_100,
        output_tokens: 468,
        cache_read_tokens: 64_700,
        cost_usd: 0.18,
    };
    let text = render(&state);
    assert!(
        text.contains("subagents ↑68.1K ↓468 · $0.1800 · cache 64.7K/95.0%"),
        "text: {text}"
    );
}

#[test]
fn custom_status_line_replaces_built_in_segments() {
    let mut state = AppState::default();
    state.ui.display_settings.status_line = Some(coco_config::StatusLineSettings::Command(
        coco_config::StatusLineCommandSettings {
            command: "printf custom".to_string(),
            padding: 0,
        },
    ));
    state.ui.status_line.apply_update(StatusLineUpdate {
        generation: 0,
        output: Some("custom\nsecond".to_string()),
    });

    let StatusBarView::Custom { line } = status_bar_view(&state) else {
        panic!("expected custom status bar");
    };

    assert_eq!(line, "custom");
}

#[test]
fn exit_prompt_takes_precedence_over_custom_status_line() {
    let mut state = AppState::default();
    state.ui.display_settings.status_line = Some(coco_config::StatusLineSettings::Command(
        coco_config::StatusLineCommandSettings {
            command: "printf custom".to_string(),
            padding: 0,
        },
    ));
    state.ui.status_line.apply_update(StatusLineUpdate {
        generation: 0,
        output: Some("custom".to_string()),
    });
    state.ui.ctrl_c_tracker.poll((), std::time::Instant::now());

    let StatusBarView::ExitPrompt { key, text } = status_bar_view(&state) else {
        panic!("expected exit prompt");
    };

    assert_eq!(key, crate::state::ExitKey::CtrlC);
    assert!(text.contains("Ctrl-C"));
}

#[test]
fn built_in_status_preserves_pending_chord_hint() {
    use crate::keybinding_bridge::KeybindingContext;
    use crate::keybinding_resolver::ResolverResult;
    use crossterm::event::KeyCode;
    use crossterm::event::KeyEvent;
    use crossterm::event::KeyEventKind;
    use crossterm::event::KeyEventState;
    use crossterm::event::KeyModifiers;

    let state = AppState::default();
    let result = state.ui.kb_handle.resolve_key(
        KeyEvent {
            code: KeyCode::Char('x'),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        },
        KeybindingContext::Chat,
    );
    assert!(matches!(result, ResolverResult::Pending));

    let StatusBarView::BuiltIn { lines } = status_bar_view(&state) else {
        panic!("expected built-in status bar");
    };
    let spans: Vec<&StatusSpan> = lines.iter().flatten().collect();
    let text = spans
        .iter()
        .map(|span| span.text.as_str())
        .collect::<String>();

    assert!(text.contains("ctrl+x"));
    assert!(text.contains("…"));
}
