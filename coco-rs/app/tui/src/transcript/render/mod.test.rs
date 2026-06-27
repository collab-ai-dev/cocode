use pretty_assertions::assert_eq;
use ratatui::style::Color;
use std::sync::Arc;
use uuid::Uuid;

use super::*;
use crate::theme::Theme;
use crate::transcript::cells::RenderedCell;
use crate::transcript::derive::message_to_cells;
use crate::transcript::derive::test_helpers;

const CACHE_TEST_MAX_ROWS: usize = 10_000;

#[test]
fn finalized_history_lines_render_committed_assistant_message() {
    let theme = Theme::default();
    let cells = vec![test_helpers::assistant_text_cell("hello")];

    let lines = render_finalized_history_lines(
        &cells,
        HistoryLineRenderOptions {
            styles: UiStyles::new(&theme),
            width: 40,
            syntax_highlighting: SyntaxHighlighting::Disabled,
            show_system_reminders: false,
            show_thinking: false,
            cwd: None,
            kb_handle: None,
            replay_cache_policy: HistoryReplayCachePolicy::default(),
            reasoning_metadata: None,
            subagent_summaries: None,
        },
    );

    assert_eq!(plain_lines(&lines), vec!["⏺ hello", ""]);
}

#[test]
fn committed_assistant_markdown_helper_matches_finalized_history_cell() {
    let theme = Theme::default();
    let render_options = options(&theme, 40);
    let direct = crate::transcript::render::assistant::render_committed_assistant_markdown(
        "hello",
        crate::transcript::render::assistant::CommittedAssistantMarkdownOptions {
            styles: render_options.styles,
            width: render_options.width,
            syntax_highlighting: render_options.syntax_highlighting,
        },
    );
    let cells = vec![test_helpers::assistant_text_cell("hello")];
    let history = render_finalized_history_lines(&cells, render_options);

    assert_eq!(plain_lines(&direct), vec!["⏺ hello"]);
    assert_eq!(plain_lines(&history[..direct.len()]), plain_lines(&direct));
}

#[test]
fn finalized_history_lines_do_not_emit_active_busy_tail() {
    let theme = Theme::default();
    let cells = vec![test_helpers::user_text_cell(Uuid::new_v4(), "hello")];

    let lines = render_finalized_history_lines(
        &cells,
        HistoryLineRenderOptions {
            styles: UiStyles::new(&theme),
            width: 40,
            syntax_highlighting: SyntaxHighlighting::Disabled,
            show_system_reminders: false,
            show_thinking: false,
            cwd: None,
            kb_handle: None,
            replay_cache_policy: HistoryReplayCachePolicy::default(),
            reasoning_metadata: None,
            subagent_summaries: None,
        },
    );

    assert_eq!(plain_lines(&lines), vec!["❯ hello", ""]);
}

#[test]
fn finalized_history_lines_hide_compact_boundary_and_summary_by_default() {
    let theme = Theme::default();
    let mut messages = vec![
        coco_messages::create_compact_boundary_message(797, 511),
        compact_summary_message("Summary:\nEarlier context details."),
    ];
    messages.extend(coco_messages::build_slash_command_messages(
        "compact",
        "",
        "Compacted (797 -> 511 tokens, saved 286 / 35.9%; Ctrl+O to see full summary)",
        false,
    ));
    let cells: Vec<RenderedCell> = messages
        .into_iter()
        .flat_map(|message| message_to_cells(Arc::new(message)))
        .collect();

    let lines = render_finalized_history_lines(
        &cells,
        HistoryLineRenderOptions {
            styles: UiStyles::new(&theme),
            width: 96,
            syntax_highlighting: SyntaxHighlighting::Disabled,
            show_system_reminders: false,
            show_thinking: false,
            cwd: None,
            kb_handle: None,
            replay_cache_policy: HistoryReplayCachePolicy::default(),
            reasoning_metadata: None,
            subagent_summaries: None,
        },
    );
    let body = plain_lines(&lines).join("\n");

    assert!(!body.contains("# [compact]"));
    assert!(!body.contains("Earlier context details."));
    assert!(body.contains("❯ /compact"));
    assert!(body.contains("Compacted (797 -> 511 tokens"));
}

#[test]
fn finalized_history_lines_collapse_meta_by_default() {
    let theme = Theme::default();
    let cells = vec![test_helpers::info_cell("system", "system reminder")];

    let lines = render_finalized_history_lines(
        &cells,
        HistoryLineRenderOptions {
            styles: UiStyles::new(&theme),
            width: 40,
            syntax_highlighting: SyntaxHighlighting::Disabled,
            show_system_reminders: false,
            show_thinking: false,
            cwd: None,
            kb_handle: None,
            replay_cache_policy: HistoryReplayCachePolicy::default(),
            reasoning_metadata: None,
            subagent_summaries: None,
        },
    );

    assert_eq!(
        plain_lines(&lines),
        vec!["  # [system] system: system reminder"]
    );
}

#[test]
fn finalized_history_lines_render_empty_title_info_as_markdown() {
    let theme = Theme::default();
    let cells = vec![test_helpers::info_cell(
        "",
        "## Context Window Usage\n\n| Category | Tokens |\n|---|---:|\n| Messages | 123 |",
    )];

    let lines = render_finalized_history_lines(
        &cells,
        HistoryLineRenderOptions {
            styles: UiStyles::new(&theme),
            width: 80,
            syntax_highlighting: SyntaxHighlighting::Disabled,
            show_system_reminders: false,
            show_thinking: false,
            cwd: None,
            kb_handle: None,
            replay_cache_policy: HistoryReplayCachePolicy::default(),
            reasoning_metadata: None,
            subagent_summaries: None,
        },
    );
    let rendered = plain_lines(&lines).join("\n");

    assert!(rendered.contains("Context Window Usage"), "{rendered}");
    assert!(
        !rendered.contains("# [system] ## Context Window Usage"),
        "{rendered}"
    );
}

#[test]
fn finalized_history_lines_show_collapsed_thinking_with_toggle_hint() {
    let theme = Theme::default();
    let kb_handle = crate::keybinding_resolver::KeybindingHandle::from_defaults();
    let (cell, meta) =
        test_helpers::assistant_thinking_cell_with_metadata("Need to inspect files.", 1300, 15);
    let mut reasoning_metadata: std::collections::HashMap<
        uuid::Uuid,
        crate::state::session::ReasoningMetadata,
    > = std::collections::HashMap::new();
    reasoning_metadata.insert(cell.message_uuid, meta);
    let cells = vec![cell];

    let lines = render_finalized_history_lines(
        &cells,
        HistoryLineRenderOptions {
            styles: UiStyles::new(&theme),
            width: 80,
            syntax_highlighting: SyntaxHighlighting::Disabled,
            show_system_reminders: false,
            show_thinking: false,
            cwd: None,
            kb_handle: Some(&kb_handle),
            replay_cache_policy: HistoryReplayCachePolicy::default(),
            reasoning_metadata: Some(&reasoning_metadata),
            subagent_summaries: None,
        },
    );

    assert_eq!(
        plain_lines(&lines),
        vec!["⏺ Thinking · 1.3s · 15 reasoning tok · F2 to expand", "",]
    );
}

#[test]
fn finalized_history_lines_render_reasoning_metadata_once_for_adjacent_parts() {
    let theme = Theme::default();
    let msg = coco_messages::create_assistant_message(
        vec![
            coco_messages::AssistantContent::Reasoning(coco_messages::ReasoningContent::new(
                "first",
            )),
            coco_messages::AssistantContent::Reasoning(coco_messages::ReasoningContent::new(
                "second",
            )),
        ],
        "test-model",
        coco_types::TokenUsage::default(),
    );
    let cells = message_to_cells(Arc::new(msg));
    let mut reasoning_metadata = std::collections::HashMap::new();
    reasoning_metadata.insert(
        cells[0].message_uuid,
        crate::state::session::ReasoningMetadata {
            duration_ms: Some(900),
            reasoning_tokens: 42,
        },
    );

    let mut render_options = options(&theme, 80);
    render_options.reasoning_metadata = Some(&reasoning_metadata);
    let lines = render_finalized_history_lines(&cells, render_options);
    let rendered = plain_lines(&lines).join("\n");

    assert_eq!(rendered.matches("reasoning tok").count(), 1);
}

#[test]
fn finalized_history_lines_show_thinking_expands_full_reasoning_body() {
    let theme = Theme::default();
    let text = (0..8)
        .map(|i| format!("line-{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let cells = vec![test_helpers::assistant_thinking_cell(&text)];

    let mut render_options = options(&theme, 80);
    render_options.show_thinking = true;
    let lines = render_finalized_history_lines(&cells, render_options);
    let rendered = plain_lines(&lines).join("\n");

    for i in 0..8 {
        assert!(rendered.contains(&format!("line-{i}")), "{rendered}");
    }
    assert!(!rendered.contains("\n  …"), "{rendered}");
}

#[test]
fn finalized_history_lines_render_metadata_only_on_first_reasoning_run() {
    let theme = Theme::default();
    let msg = coco_messages::create_assistant_message(
        vec![
            coco_messages::AssistantContent::Reasoning(coco_messages::ReasoningContent::new(
                "first",
            )),
            coco_messages::AssistantContent::Text(coco_messages::TextContent::new("answer")),
            coco_messages::AssistantContent::Reasoning(coco_messages::ReasoningContent::new(
                "second",
            )),
            coco_messages::AssistantContent::ToolCall(coco_messages::ToolCallContent::new(
                "call-1",
                "Read",
                serde_json::json!({}),
            )),
        ],
        "test-model",
        coco_types::TokenUsage::default(),
    );
    let cells = message_to_cells(Arc::new(msg));
    let thinking_cells = cells
        .iter()
        .filter(|cell| matches!(cell.kind, CellKind::AssistantThinking { .. }))
        .count();
    let mut reasoning_metadata = std::collections::HashMap::new();
    reasoning_metadata.insert(
        cells[0].message_uuid,
        crate::state::session::ReasoningMetadata {
            duration_ms: Some(1100),
            reasoning_tokens: 31,
        },
    );

    let mut render_options = options(&theme, 80);
    render_options.reasoning_metadata = Some(&reasoning_metadata);
    let lines = render_finalized_history_lines(&cells, render_options);
    let rendered = plain_lines(&lines).join("\n");

    assert_eq!(thinking_cells, 2);
    assert_eq!(rendered.matches("reasoning tok").count(), 1);
}

#[test]
fn replay_history_lines_keeps_all_rows_under_cap() {
    let theme = Theme::default();
    let cells = vec![test_helpers::assistant_text_cell("hello")];

    let replay = render_replay_history_lines(&cells, options(&theme, 40), 4);

    assert_eq!(plain_lines(&replay.lines), vec!["⏺ hello", ""]);
    assert_eq!(replay.omitted_messages, 0);
}

#[test]
fn replay_history_lines_caps_wrapped_rendered_rows() {
    let theme = Theme::default();
    let cells = vec![test_helpers::assistant_text_cell(&"wrapped ".repeat(200))];

    let replay = render_replay_history_lines(&cells, options(&theme, 24), 6);

    assert_eq!(replay.omitted_messages, 1);
    assert!(
        replay.rows.height() <= 6,
        "rendered rows exceeded cap: {}",
        replay.rows.height()
    );
}

#[test]
fn replay_history_lines_truncates_at_message_boundaries_with_marker() {
    let theme = Theme::default();
    let cells = vec![
        test_helpers::assistant_text_cell("one"),
        test_helpers::assistant_text_cell("two"),
        test_helpers::assistant_text_cell("three"),
    ];

    let replay = render_replay_history_lines(&cells, options(&theme, 80), 5);

    assert_eq!(replay.omitted_messages, 2);
    assert!(replay.rows.height() <= 5);
    assert_eq!(
        plain_lines(&replay.lines),
        vec![
            "... 2 older messages retained in transcript, not replayed",
            "    open transcript pager for full history",
            "",
            "⏺ three",
            "",
        ]
    );
}

#[test]
fn replay_history_lines_binary_search_picks_smallest_fitting_suffix() {
    let theme = Theme::default();
    // 50 single-line assistant messages; each renders "⏺ msgN" + "" (2 rows).
    let cells: Vec<_> = (0..50)
        .map(|i| test_helpers::assistant_text_cell(&format!("msg{i}")))
        .collect();

    // marker = 3 rows, each message = 2 rows. Budget 13 ⇒ keep 5 messages
    // (msg45..msg49) ⇒ omit 45. Exercises the binary search over boundaries.
    let replay = render_replay_history_lines(&cells, options(&theme, 80), 13);

    assert_eq!(replay.omitted_messages, 45);
    assert!(replay.rows.height() <= 13);
    let rendered = plain_lines(&replay.lines);
    assert_eq!(
        rendered.first().map(String::as_str),
        Some("... 45 older messages retained in transcript, not replayed")
    );
    // First content row after the 3-row marker is the retained-suffix head.
    assert_eq!(rendered.get(3).map(String::as_str), Some("⏺ msg45"));
}

#[test]
fn replay_cache_hit_returns_shared_lines() {
    let theme = Theme::default();
    let cells = cacheable_cells("cached");
    let mut cache = HistoryReplayCache::default();

    let first = render_replay_history_lines_cached(
        &cells,
        options(&theme, 40),
        CACHE_TEST_MAX_ROWS,
        &mut cache,
    );
    let second = render_replay_history_lines_cached(
        &cells,
        options(&theme, 40),
        CACHE_TEST_MAX_ROWS,
        &mut cache,
    );

    assert!(!first.stats.cache_hit);
    assert!(first.stats.cacheable);
    assert_eq!(first.stats.cache_lookup, HistoryReplayCacheLookup::Miss);
    assert!(first.stats.cache_admitted);
    assert!(second.stats.cache_hit);
    assert_eq!(second.stats.cache_lookup, HistoryReplayCacheLookup::Hit);
    assert!(std::sync::Arc::ptr_eq(&first.lines, &second.lines));
    assert!(std::sync::Arc::ptr_eq(&first.rows, &second.rows));
    assert_eq!(second.stats.finalized_render_calls, 0);
    assert!(
        plain_lines(&second.lines)
            .iter()
            .any(|line| line.contains("cached 31"))
    );
    assert!(
        plain_history_rows(&second.rows)
            .iter()
            .any(|line| line.contains("cached 31"))
    );
}

#[test]
fn replay_cache_invalidates_on_width_display_theme_and_content_changes() {
    let theme = Theme::default();
    let alternate_theme = Theme {
        assistant_message: Color::Red,
        ..Theme::default()
    };
    let cells = cacheable_cells("base");
    let content_changed = cacheable_cells("changed");
    let mut model_changed = cells.clone();
    if let CellKind::AssistantText { model, .. } = &mut model_changed[0].kind {
        *model = "other-model".to_string();
    }
    let mut cache = HistoryReplayCache::default();

    let _ = render_replay_history_lines_cached(
        &cells,
        options(&theme, 40),
        CACHE_TEST_MAX_ROWS,
        &mut cache,
    );
    let same = render_replay_history_lines_cached(
        &cells,
        options(&theme, 40),
        CACHE_TEST_MAX_ROWS,
        &mut cache,
    );
    let width_changed = render_replay_history_lines_cached(
        &cells,
        options(&theme, 60),
        CACHE_TEST_MAX_ROWS,
        &mut cache,
    );
    let syntax_changed = render_replay_history_lines_cached(
        &cells,
        options_with_syntax(&theme, 40, SyntaxHighlighting::Enabled),
        CACHE_TEST_MAX_ROWS,
        &mut cache,
    );
    let theme_changed = render_replay_history_lines_cached(
        &cells,
        options(&alternate_theme, 40),
        CACHE_TEST_MAX_ROWS,
        &mut cache,
    );
    let max_rows_changed = render_replay_history_lines_cached(
        &cells,
        options(&theme, 40),
        CACHE_TEST_MAX_ROWS + 1,
        &mut cache,
    );
    let mut show_thinking_options = options(&theme, 40);
    show_thinking_options.show_thinking = true;
    let show_thinking_changed = render_replay_history_lines_cached(
        &cells,
        show_thinking_options,
        CACHE_TEST_MAX_ROWS,
        &mut cache,
    );
    let mut show_system_options = options(&theme, 40);
    show_system_options.show_system_reminders = true;
    let show_system_changed = render_replay_history_lines_cached(
        &cells,
        show_system_options,
        CACHE_TEST_MAX_ROWS,
        &mut cache,
    );
    let content_changed = render_replay_history_lines_cached(
        &content_changed,
        options(&theme, 40),
        CACHE_TEST_MAX_ROWS,
        &mut cache,
    );
    let model_changed = render_replay_history_lines_cached(
        &model_changed,
        options(&theme, 40),
        CACHE_TEST_MAX_ROWS,
        &mut cache,
    );

    assert!(same.stats.cache_hit);
    assert!(!width_changed.stats.cache_hit);
    assert!(!syntax_changed.stats.cache_hit);
    assert!(!theme_changed.stats.cache_hit);
    assert!(!max_rows_changed.stats.cache_hit);
    assert!(!show_thinking_changed.stats.cache_hit);
    assert!(!show_system_changed.stats.cache_hit);
    assert!(!content_changed.stats.cache_hit);
    assert!(!model_changed.stats.cache_hit);
}

#[test]
fn replay_cache_invalidates_on_system_payload_changes() {
    let theme = Theme::default();
    let cells = info_cells("same");
    let changed = info_cells("different");
    let mut cache = HistoryReplayCache::default();

    let _ = render_replay_history_lines_cached(
        &cells,
        options(&theme, 40),
        CACHE_TEST_MAX_ROWS,
        &mut cache,
    );
    let same = render_replay_history_lines_cached(
        &cells,
        options(&theme, 40),
        CACHE_TEST_MAX_ROWS,
        &mut cache,
    );
    let changed = render_replay_history_lines_cached(
        &changed,
        options(&theme, 40),
        CACHE_TEST_MAX_ROWS,
        &mut cache,
    );

    assert!(same.stats.cache_hit);
    assert!(!changed.stats.cache_hit);
}

#[test]
fn replay_cache_entry_limit_evicts_oldest_deterministically() {
    let theme = Theme::default();
    let policy = policy_with_limits(2, usize::MAX);
    let mut cache = HistoryReplayCache::default();
    let first = cacheable_cells("one");
    let second = cacheable_cells("two");
    let third = cacheable_cells("three");

    let _ = render_replay_history_lines_cached(
        &first,
        options_with_policy(&theme, 40, policy),
        CACHE_TEST_MAX_ROWS,
        &mut cache,
    );
    let _ = render_replay_history_lines_cached(
        &second,
        options_with_policy(&theme, 40, policy),
        CACHE_TEST_MAX_ROWS,
        &mut cache,
    );
    let third_replay = render_replay_history_lines_cached(
        &third,
        options_with_policy(&theme, 40, policy),
        CACHE_TEST_MAX_ROWS,
        &mut cache,
    );
    let second_again = render_replay_history_lines_cached(
        &second,
        options_with_policy(&theme, 40, policy),
        CACHE_TEST_MAX_ROWS,
        &mut cache,
    );
    let first_again = render_replay_history_lines_cached(
        &first,
        options_with_policy(&theme, 40, policy),
        CACHE_TEST_MAX_ROWS,
        &mut cache,
    );

    assert_eq!(third_replay.stats.cache_evictions, 1);
    assert!(second_again.stats.cache_hit);
    assert!(!first_again.stats.cache_hit);
}

#[test]
fn replay_cache_byte_limit_evicts_oldest_deterministically() {
    let theme = Theme::default();
    let first = cacheable_cells("one");
    let second = cacheable_cells("two");
    let sizing_policy = policy_with_limits(10, usize::MAX);
    let mut sizing_cache = HistoryReplayCache::default();
    let first_sized = render_replay_history_lines_cached(
        &first,
        options_with_policy(&theme, 40, sizing_policy),
        CACHE_TEST_MAX_ROWS,
        &mut sizing_cache,
    );
    let byte_limit = first_sized.stats.replay_estimated_bytes + 128;
    let policy = policy_with_limits(10, byte_limit);
    let mut cache = HistoryReplayCache::default();

    let _ = render_replay_history_lines_cached(
        &first,
        options_with_policy(&theme, 40, policy),
        CACHE_TEST_MAX_ROWS,
        &mut cache,
    );
    let second_replay = render_replay_history_lines_cached(
        &second,
        options_with_policy(&theme, 40, policy),
        CACHE_TEST_MAX_ROWS,
        &mut cache,
    );
    let second_again = render_replay_history_lines_cached(
        &second,
        options_with_policy(&theme, 40, policy),
        CACHE_TEST_MAX_ROWS,
        &mut cache,
    );
    let first_again = render_replay_history_lines_cached(
        &first,
        options_with_policy(&theme, 40, policy),
        CACHE_TEST_MAX_ROWS,
        &mut cache,
    );

    assert_eq!(second_replay.stats.cache_evictions, 1);
    assert!(second_again.stats.cache_hit);
    assert!(!first_again.stats.cache_hit);
}

#[test]
fn replay_cache_excludes_kind_source_mismatched_cells() {
    // Thinking/tool cells are cacheable now — their side inputs are hashed
    // into the key (see `test_replay_cache_key_covers_tool_thinking_and_
    // attachment_cells`). The only cells left on the uncached path are
    // defensive kind/source MISMATCHES, which `message_to_cells` never
    // produces but must not poison the cache if they ever appear.
    let theme = Theme::default();
    let mut cells: Vec<_> = (0..32)
        .map(|i| test_helpers::assistant_thinking_cell(&format!("dynamic {i}")))
        .collect();
    cells.push(RenderedCell {
        kind: CellKind::System(SystemCellKind::Informational),
        ..test_helpers::user_text_cell(Uuid::new_v4(), "not a system message")
    });
    let mut cache = HistoryReplayCache::default();

    let first = render_replay_history_lines_cached(
        &cells,
        options(&theme, 40),
        CACHE_TEST_MAX_ROWS,
        &mut cache,
    );
    let second = render_replay_history_lines_cached(
        &cells,
        options(&theme, 40),
        CACHE_TEST_MAX_ROWS,
        &mut cache,
    );

    assert!(!first.stats.cacheable);
    assert_eq!(
        first.stats.cache_skip_reason,
        Some(HistoryReplayCacheSkipReason::UnsupportedCell)
    );
    assert!(!second.stats.cacheable);
    assert!(!second.stats.cache_hit);
}

#[test]
fn replay_cache_skips_small_replay_without_lookup_or_admission() {
    let theme = Theme::default();
    let cells = vec![test_helpers::assistant_text_cell("small")];
    let mut cache = HistoryReplayCache::default();

    let first = render_replay_history_lines_cached(&cells, options(&theme, 40), 10, &mut cache);
    let second = render_replay_history_lines_cached(&cells, options(&theme, 40), 10, &mut cache);

    assert_eq!(first.stats.cache_lookup, HistoryReplayCacheLookup::Skipped);
    assert_eq!(
        first.stats.cache_skip_reason,
        Some(HistoryReplayCacheSkipReason::BelowReplayThreshold)
    );
    assert_eq!(first.stats.cache_entries, 0);
    assert_eq!(second.stats.cache_lookup, HistoryReplayCacheLookup::Skipped);
    assert!(!second.stats.cache_hit);
}

#[test]
fn replay_cache_allows_low_cell_count_when_content_is_large() {
    let theme = Theme::default();
    let cells = vec![test_helpers::assistant_text_cell(&"large ".repeat(2_000))];
    let policy = HistoryReplayCachePolicy {
        admit_min_result_bytes: 1,
        ..HistoryReplayCachePolicy::default()
    };
    let mut cache = HistoryReplayCache::default();

    let first = render_replay_history_lines_cached(
        &cells,
        options_with_policy(&theme, 80, policy),
        CACHE_TEST_MAX_ROWS,
        &mut cache,
    );
    let second = render_replay_history_lines_cached(
        &cells,
        options_with_policy(&theme, 80, policy),
        CACHE_TEST_MAX_ROWS,
        &mut cache,
    );

    assert_eq!(first.stats.cache_lookup, HistoryReplayCacheLookup::Miss);
    assert!(first.stats.cell_content_estimated_bytes >= 8 * 1024);
    assert!(first.stats.cache_admitted);
    assert!(second.stats.cache_hit);
}

#[test]
fn replay_cache_policy_can_disable_lookup_and_admission() {
    let theme = Theme::default();
    let cells = cacheable_cells("disabled");
    let policy = HistoryReplayCachePolicy {
        enabled: false,
        ..HistoryReplayCachePolicy::default()
    };
    let mut cache = HistoryReplayCache::default();

    let replay = render_replay_history_lines_cached(
        &cells,
        options_with_policy(&theme, 40, policy),
        CACHE_TEST_MAX_ROWS,
        &mut cache,
    );

    assert_eq!(replay.stats.cache_lookup, HistoryReplayCacheLookup::Skipped);
    assert_eq!(
        replay.stats.cache_skip_reason,
        Some(HistoryReplayCacheSkipReason::CacheDisabled)
    );
    assert_eq!(replay.stats.cache_entries, 0);
}

#[test]
fn replay_cache_includes_compact_boundary_with_shortcut_in_key() {
    let theme = Theme::default();
    let mut cells = cacheable_cells("compact-boundary");
    cells.push(compact_boundary_cell(50_000, 20_000));
    let mut cache = HistoryReplayCache::default();

    let first = render_replay_history_lines_cached(
        &cells,
        options(&theme, 80),
        CACHE_TEST_MAX_ROWS,
        &mut cache,
    );
    let second = render_replay_history_lines_cached(
        &cells,
        options(&theme, 80),
        CACHE_TEST_MAX_ROWS,
        &mut cache,
    );

    assert_eq!(first.stats.cache_lookup, HistoryReplayCacheLookup::Miss);
    assert!(first.stats.cache_admitted);
    assert!(second.stats.cache_hit);
}

#[test]
fn replay_cache_stats_track_entry_bytes_and_oversize_skip() {
    let theme = Theme::default();
    let cells = cacheable_cells("oversize");
    let policy = policy_with_limits(10, 1);
    let mut cache = HistoryReplayCache::default();

    let replay = render_replay_history_lines_cached(
        &cells,
        options_with_policy(&theme, 40, policy),
        CACHE_TEST_MAX_ROWS,
        &mut cache,
    );

    assert_eq!(replay.stats.cache_lookup, HistoryReplayCacheLookup::Miss);
    assert_eq!(
        replay.stats.cache_skip_reason,
        Some(HistoryReplayCacheSkipReason::EntryTooLarge)
    );
    assert!(replay.stats.replay_estimated_bytes > 1);
    assert_eq!(replay.stats.cache_entries, 0);
    assert_eq!(replay.stats.cache_estimated_bytes, 0);
}

#[test]
fn replay_cache_entry_bytes_include_lines_and_rows() {
    let theme = Theme::default();
    let cells = cacheable_cells("entry-size");
    let mut cache = HistoryReplayCache::default();

    let replay = render_replay_history_lines_cached(
        &cells,
        options(&theme, 40),
        CACHE_TEST_MAX_ROWS,
        &mut cache,
    );

    assert!(replay.stats.cache_admitted);
    assert!(replay.stats.replay_estimated_bytes > replay.rows.estimated_bytes());
    assert_eq!(
        replay.stats.cache_estimated_bytes,
        replay.stats.replay_estimated_bytes
    );
}

#[test]
fn replay_cached_output_matches_uncached_output() {
    let theme = Theme::default();
    let cells = cacheable_cells("same-output");
    let mut cache = HistoryReplayCache::default();

    let uncached = render_replay_history_lines(&cells, options(&theme, 72), 100);
    let first = render_replay_history_lines_cached(&cells, options(&theme, 72), 100, &mut cache);
    let cached = render_replay_history_lines_cached(&cells, options(&theme, 72), 100, &mut cache);

    assert!(cached.stats.cache_hit);
    assert_eq!(plain_lines(&first.lines), plain_lines(&uncached.lines));
    assert_eq!(plain_lines(&cached.lines), plain_lines(&uncached.lines));
}

#[test]
fn replay_syntax_highlighting_toggle_output_remains_correct() {
    let theme = Theme::default();
    let cells = syntax_cells();
    let mut cache = HistoryReplayCache::default();

    let highlighted = render_replay_history_lines_cached(
        &cells,
        options_with_syntax(&theme, 80, SyntaxHighlighting::Enabled),
        200,
        &mut cache,
    );
    let plain = render_replay_history_lines_cached(&cells, options(&theme, 80), 200, &mut cache);

    assert!(!highlighted.stats.cache_hit);
    assert!(!plain.stats.cache_hit);
    assert!(
        plain_lines(&highlighted.lines)
            .iter()
            .any(|line| line.contains("fn replay"))
    );
    assert!(
        plain_lines(&plain.lines)
            .iter()
            .any(|line| line.contains("fn replay"))
    );
}

#[test]
fn replay_finalized_mermaid_fence_renders_as_diagram() {
    let theme = Theme::default();
    let cells = mermaid_cells();

    let replay = render_replay_history_lines(
        &cells,
        options_with_syntax(&theme, 96, SyntaxHighlighting::Enabled),
        500,
    );
    let output = plain_lines(&replay.lines).join("\n");

    assert!(output.contains("Start"));
    assert!(output.contains("Finish"));
    assert!(!output.contains("flowchart LR"));
}

fn options(theme: &Theme, width: u16) -> HistoryLineRenderOptions<'_> {
    options_with_syntax(theme, width, SyntaxHighlighting::Disabled)
}

fn options_with_policy(
    theme: &Theme,
    width: u16,
    replay_cache_policy: HistoryReplayCachePolicy,
) -> HistoryLineRenderOptions<'_> {
    HistoryLineRenderOptions {
        replay_cache_policy,
        ..options(theme, width)
    }
}

fn options_with_syntax(
    theme: &Theme,
    width: u16,
    syntax_highlighting: SyntaxHighlighting,
) -> HistoryLineRenderOptions<'_> {
    HistoryLineRenderOptions {
        styles: UiStyles::new(theme),
        width,
        syntax_highlighting,
        show_system_reminders: false,
        show_thinking: false,
        cwd: None,
        kb_handle: None,
        replay_cache_policy: HistoryReplayCachePolicy::default(),
        reasoning_metadata: None,
        subagent_summaries: None,
    }
}

fn policy_with_limits(max_entries: usize, max_estimated_bytes: usize) -> HistoryReplayCachePolicy {
    HistoryReplayCachePolicy {
        max_entries,
        max_estimated_bytes,
        ..HistoryReplayCachePolicy::default()
    }
}

fn compact_boundary_cell(tokens_before: i64, tokens_after: i64) -> RenderedCell {
    message_to_cells(Arc::new(coco_messages::create_compact_boundary_message(
        tokens_before,
        tokens_after,
    )))
    .into_iter()
    .next()
    .expect("compact boundary message yields a cell")
}

fn compact_summary_message(text: &str) -> coco_messages::Message {
    coco_messages::Message::User(coco_messages::UserMessage {
        message: coco_messages::LlmMessage::user_text(text),
        uuid: Uuid::new_v4(),
        timestamp: String::new(),
        is_visible_in_transcript_only: true,
        is_virtual: false,
        is_compact_summary: true,
        permission_mode: None,
        origin: None,
        parent_tool_use_id: None,
    })
}

fn plain_lines(lines: &[Line<'_>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

fn plain_history_rows(rows: &coco_tui_ui::engine::history_insert::HistoryRows) -> Vec<String> {
    let buffer = rows.buffer();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

fn cacheable_cells(label: &str) -> Vec<RenderedCell> {
    (0..32)
        .map(|i| {
            test_helpers::assistant_text_cell(&format!(
                "{label} {i}: {}",
                "cache admission payload ".repeat(64)
            ))
        })
        .collect()
}

fn info_cells(message: &str) -> Vec<RenderedCell> {
    (0..32)
        .map(|i| {
            test_helpers::info_cell(
                "notice",
                &format!("{message} {i}: {}", "system cache payload ".repeat(64)),
            )
        })
        .collect()
}

fn syntax_cells() -> Vec<RenderedCell> {
    (0..32)
        .map(|i| {
            test_helpers::assistant_text_cell(&format!(
                "```rust\nfn replay_{i}() -> i32 {{\n    {i}\n}}\n```"
            ))
        })
        .collect()
}

fn mermaid_cells() -> Vec<RenderedCell> {
    let mut cells = cacheable_cells("before-mermaid");
    cells.push(test_helpers::assistant_text_cell(
        "```mermaid\nflowchart LR\n  A[Start] --> B[Finish]\n```",
    ));
    cells
}

#[test]
fn test_replay_cache_key_covers_tool_thinking_and_attachment_cells() {
    // Structural guard for the replay cache's coverage: tool / thinking /
    // attachment cells used to bail the whole transcript out of the cache
    // (`replay_cache_key -> None`), making it dead for virtually every real
    // session. The key must exist for the dominant agent-session shape...
    let theme = Theme::default();
    let mut cells = Vec::new();
    cells.push(test_helpers::user_text_cell(
        Uuid::new_v4(),
        "grep the repo",
    ));
    cells.push(test_helpers::assistant_thinking_cell("planning the grep"));
    cells.push(test_helpers::assistant_text_cell("Running the search."));
    cells.push(test_helpers::tool_use_cell(
        "call-1",
        "Grep",
        serde_json::json!({"pattern": "fn main"}),
    ));
    cells.push(test_helpers::tool_result_cell(
        "call-1",
        "Grep",
        "src/main.rs:1:fn main() {",
    ));
    let key = replay_cache_key(&cells, options(&Theme::default(), 80), 9_000);
    assert!(
        key.is_some(),
        "tool/thinking-bearing transcripts must stay replay-cacheable"
    );

    // ...and must change when a render-affecting side input changes: the
    // reasoning badge is read from the side-cache, not the cell content.
    let mut metadata = std::collections::HashMap::new();
    metadata.insert(
        cells[1].message_uuid,
        crate::state::session::ReasoningMetadata {
            duration_ms: Some(1_300),
            reasoning_tokens: 15,
        },
    );
    let with_badge = replay_cache_key(
        &cells,
        HistoryLineRenderOptions {
            reasoning_metadata: Some(&metadata),
            subagent_summaries: None,
            ..options(&theme, 80)
        },
        9_000,
    );
    assert_ne!(
        key, with_badge,
        "reasoning side-cache state must be part of the key"
    );
}

#[test]
fn test_replay_cached_render_hits_for_tool_heavy_transcript() {
    // End-to-end guard: a tool-heavy transcript rendered twice through the
    // cached replay path must hit on the second pass (this was the
    // every-real-session cache miss).
    let theme = Theme::default();
    let mut cells = Vec::new();
    for i in 0..40 {
        cells.push(test_helpers::user_text_cell(
            Uuid::new_v4(),
            &format!("inspect case {i}"),
        ));
        cells.push(test_helpers::assistant_text_cell("Inspecting."));
        cells.push(test_helpers::tool_use_cell(
            &format!("call-{i}"),
            "Grep",
            serde_json::json!({"pattern": format!("case_{i}")}),
        ));
        cells.push(test_helpers::tool_result_cell(
            &format!("call-{i}"),
            "Grep",
            &format!("src/lib.rs:{i}: fn case_{i}() {{}}"),
        ));
    }
    let mut cache = HistoryReplayCache::default();
    let first = render_replay_history_lines_cached(&cells, options(&theme, 80), 9_000, &mut cache);
    assert!(!first.stats.cache_hit);
    assert!(first.stats.cacheable, "tool-heavy replay must be cacheable");
    let second = render_replay_history_lines_cached(&cells, options(&theme, 80), 9_000, &mut cache);
    assert!(second.stats.cache_hit, "second replay must hit the cache");
    assert_eq!(second.stats.finalized_render_calls, 0);
    assert_eq!(first.lines, second.lines);
}
