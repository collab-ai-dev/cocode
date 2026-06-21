use std::sync::Arc;
use std::time::Instant;

use pretty_assertions::assert_eq;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use uuid::Uuid;

use super::*;
use crate::state::AppState;
use crate::state::ui::StreamingState;
use crate::surface::modal::HistorySurfaceMode;
use crate::surface::modal::SurfaceFramePlan;
use crate::surface::stream::SurfaceStreamDriver;
use crate::theme::Theme;
use crate::transcript::derive::message_to_cells;
use crate::transcript::derive::test_helpers;
use crate::transcript::emission::HistoryEmissionOutcome;
use crate::transcript::render::HistoryReplayCachePolicy;
use coco_tui_ui::display::SyntaxHighlighting;
use coco_tui_ui::engine::history_reflow::HistoryViewportChange;
use coco_tui_ui::style::UiStyles;

#[test]
fn driver_emit_append_only_uses_finalized_transcript_renderer() {
    let theme = Theme::default();
    let backend = TestBackend::with_lines([
        "old0    ", "old1    ", "old2    ", "old3    ", "old4    ", "old5    ", "view    ",
    ]);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 6, 8, 1));
    let cells = vec![test_helpers::assistant_text_cell("hello")];
    let mut driver = SurfaceHistoryDriver::default();

    let outcome = driver
        .emit_append_only(&mut terminal, header(), &cells, 1, options(&theme, 8))
        .expect("emit");

    assert_eq!(
        outcome,
        HistoryEmissionOutcome::Appended {
            start: 0,
            message_count: 1,
            rows: 3,
        }
    );
    assert_eq!(
        plain_buffer_lines(terminal.backend().buffer()),
        vec![
            "old3    ",
            "old4    ",
            "old5    ",
            "header  ",
            "⏺ hello ",
            "        ",
            "view    "
        ]
    );
}

#[test]
fn driver_finalizes_verified_stream_prefix_by_appending_suffix_only() {
    let theme = Theme::default();
    let width = 40;
    let backend = TestBackend::new(width, 14);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 8, width, 6));
    let mut driver = initialized_driver(&mut terminal, &theme, width);
    let stream_append = prepared_stream_append("alpha\n\nbeta", width);

    let stream_outcome = driver
        .commit_stream_append(&mut terminal, &stream_append)
        .expect("commit stream append");
    assert!(matches!(
        stream_outcome,
        HistoryEmissionOutcome::Appended { .. }
    ));

    let cells = vec![test_helpers::assistant_text_cell("alpha\n\nbeta")];
    let final_outcome = driver
        .emit_after_stream_commit(
            &mut terminal,
            header(),
            &cells,
            2,
            options(&theme, width),
            Some(&stream_append.commit),
        )
        .expect("finalize assistant");

    assert!(
        matches!(final_outcome, HistoryEmissionOutcome::Appended { .. }),
        "{final_outcome:?}"
    );
    // The single commit lives on the stream driver; the row-level invariant is
    // that neither leading nor trailing rows duplicate after the suffix append.
    let text = plain_terminal_text(&terminal);
    assert_eq!(text.matches("alpha").count(), 1, "{text}");
    assert_eq!(text.matches("beta").count(), 1, "{text}");
}

#[test]
fn driver_requires_replay_when_stream_commit_source_mismatches_final_cell() {
    let theme = Theme::default();
    let width = 40;
    let backend = TestBackend::new(width, 14);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 8, width, 6));
    let mut driver = initialized_driver(&mut terminal, &theme, width);
    let stream_append = prepared_stream_append("alpha\n\nbeta", width);
    driver
        .commit_stream_append(&mut terminal, &stream_append)
        .expect("commit stream append");

    let cells = vec![test_helpers::assistant_text_cell("omega\n\nbeta")];
    let outcome = driver
        .emit_after_stream_commit(
            &mut terminal,
            header(),
            &cells,
            2,
            options(&theme, width),
            Some(&stream_append.commit),
        )
        .expect("finalize assistant");

    assert_eq!(outcome, HistoryEmissionOutcome::ReplayRequired);
    let text = plain_terminal_text(&terminal);
    assert_eq!(text.matches("alpha").count(), 1, "{text}");
    assert_eq!(text.matches("omega").count(), 0, "{text}");
}

#[test]
fn driver_finalizes_stream_prefix_for_thinking_text_turn_without_replay() {
    let theme = Theme::default();
    let width = 40;
    let backend = TestBackend::new(width, 14);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 8, width, 6));
    let mut driver = initialized_driver(&mut terminal, &theme, width);
    let stream_append = prepared_stream_append("alpha\n\nbeta", width);
    driver
        .commit_stream_append(&mut terminal, &stream_append)
        .expect("commit stream append");

    let cells = assistant_reasoning_text_cells("pondering deeply", "alpha\n\nbeta");
    let outcome = driver
        .emit_after_stream_commit(
            &mut terminal,
            header(),
            &cells,
            2,
            options(&theme, width),
            Some(&stream_append.commit),
        )
        .expect("finalize assistant");

    assert!(
        matches!(outcome, HistoryEmissionOutcome::Appended { .. }),
        "thinking+text turn must append the verified suffix, not replay: {outcome:?}"
    );
    let text = plain_terminal_text(&terminal);
    assert_eq!(text.matches("alpha").count(), 1, "{text}");
    assert_eq!(text.matches("beta").count(), 1, "{text}");
    let beta = text.find("beta").expect("beta visible");
    let thinking = text.find("Thinking").expect("thinking visible");
    assert!(
        beta < thinking,
        "leading thinking renders after the streamed text:\n{text}"
    );
}

#[test]
fn driver_stream_suffix_append_matches_full_replay_for_thinking_turn() {
    let theme = Theme::default();
    let width = 40;
    let cells = assistant_reasoning_text_cells("pondering deeply", "alpha\n\nbeta");

    // Incremental: header → mid-stream stable prefix → verified suffix append.
    let backend = TestBackend::new(width, 14);
    let mut incremental = SurfaceTerminal::new(backend).expect("terminal");
    incremental.set_viewport_area(Rect::new(0, 8, width, 6));
    let mut driver = initialized_driver(&mut incremental, &theme, width);
    let stream_append = prepared_stream_append("alpha\n\nbeta", width);
    driver
        .commit_stream_append(&mut incremental, &stream_append)
        .expect("commit stream append");
    let outcome = driver
        .emit_after_stream_commit(
            &mut incremental,
            header(),
            &cells,
            2,
            options(&theme, width),
            Some(&stream_append.commit),
        )
        .expect("finalize assistant");
    assert!(
        matches!(outcome, HistoryEmissionOutcome::Appended { .. }),
        "{outcome:?}"
    );

    // Replay: the same transcript rendered from source in one pass.
    let backend = TestBackend::new(width, 14);
    let mut replayed = SurfaceTerminal::new(backend).expect("terminal");
    replayed.set_viewport_area(Rect::new(0, 8, width, 6));
    let mut replay_driver = SurfaceHistoryDriver::default();
    replay_driver
        .replay_all_capped(
            &mut replayed,
            header(),
            &cells,
            2,
            options(&theme, width),
            HistoryReplayMode {
                stream_active: false,
                cause: "test_parity",
            },
        )
        .expect("replay");

    assert_eq!(
        plain_buffer_lines(incremental.backend().buffer()),
        plain_buffer_lines(replayed.backend().buffer()),
        "incremental stream-suffix append must be row-identical to a full replay"
    );
}

#[test]
fn driver_requires_replay_when_thinking_run_lacks_same_message_text() {
    let theme = Theme::default();
    let width = 40;
    let backend = TestBackend::new(width, 14);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 8, width, 6));
    let mut driver = initialized_driver(&mut terminal, &theme, width);
    let stream_append = prepared_stream_append("alpha\n\nbeta", width);
    driver
        .commit_stream_append(&mut terminal, &stream_append)
        .expect("commit stream append");

    // Thinking-only message followed by a DIFFERENT message's text: the
    // presentation renders this shape unreordered, so the streamed text rows
    // cannot be the group's leading rows — full replay required.
    let mut cells = message_to_cells(Arc::new(coco_messages::create_assistant_message(
        vec![coco_messages::AssistantContent::Reasoning(
            coco_messages::ReasoningContent::new("solo thinking"),
        )],
        "test-model",
        coco_types::TokenUsage::default(),
    )));
    cells.push(test_helpers::assistant_text_cell("alpha\n\nbeta"));

    let outcome = driver
        .emit_after_stream_commit(
            &mut terminal,
            header(),
            &cells,
            2,
            options(&theme, width),
            Some(&stream_append.commit),
        )
        .expect("finalize");
    assert_eq!(outcome, HistoryEmissionOutcome::ReplayRequired);
}

#[test]
fn driver_defers_parallel_tool_batch_until_all_results_arrive() {
    let theme = Theme::default();
    let width = 96;
    let backend = TestBackend::new(width, 40);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 34, width, 6));
    let mut driver = SurfaceHistoryDriver::default();

    let user = test_helpers::user_text_cell(Uuid::new_v4(), "run tools");
    let assistant = parallel_tool_use_cells();
    let bash_result = tool_result_cell("bash-call", "Bash", "bash output");
    let glob_result = tool_result_cell("glob-call", "Glob", "glob output");

    let user_cells = vec![user.clone()];
    assert!(matches!(
        driver
            .emit_append_only(
                &mut terminal,
                header(),
                &user_cells,
                1,
                options(&theme, width)
            )
            .expect("emit user"),
        HistoryEmissionOutcome::Appended { .. }
    ));

    let unresolved = cells_from_parts(&[std::slice::from_ref(&user), &assistant]);
    let unresolved_outcome = driver
        .emit_append_only(
            &mut terminal,
            header(),
            &unresolved,
            2,
            options(&theme, width),
        )
        .expect("emit unresolved tools");
    assert_eq!(unresolved_outcome, HistoryEmissionOutcome::Noop);
    let unresolved_text = plain_buffer_lines(terminal.backend().buffer()).join("\n");
    assert!(!unresolved_text.contains("Bash"), "{unresolved_text}");
    assert!(!unresolved_text.contains("Glob"), "{unresolved_text}");

    let bash_only = cells_from_parts(&[
        std::slice::from_ref(&user),
        &assistant,
        std::slice::from_ref(&bash_result),
    ]);
    let bash_only_outcome = driver
        .emit_append_only(
            &mut terminal,
            header(),
            &bash_only,
            3,
            options(&theme, width),
        )
        .expect("emit partially resolved tools");
    assert_eq!(bash_only_outcome, HistoryEmissionOutcome::Noop);
    let bash_only_text = plain_buffer_lines(terminal.backend().buffer()).join("\n");
    assert!(!bash_only_text.contains("Bash"), "{bash_only_text}");
    assert!(!bash_only_text.contains("bash output"), "{bash_only_text}");

    let complete = cells_from_parts(&[&[user], &assistant, &[bash_result, glob_result]]);
    let complete_outcome = driver
        .emit_append_only(
            &mut terminal,
            header(),
            &complete,
            4,
            options(&theme, width),
        )
        .expect("emit completed tools");
    assert!(matches!(
        complete_outcome,
        HistoryEmissionOutcome::Appended { .. }
    ));
    assert_in_text_order(
        &plain_buffer_lines(terminal.backend().buffer()).join("\n"),
        &["Bash", "bash output", "Glob", "glob output"],
    );
}

#[test]
fn driver_does_not_resolve_tool_use_from_prior_orphan_result() {
    let theme = Theme::default();
    let width = 96;
    let backend = TestBackend::new(width, 40);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 34, width, 6));
    let mut driver = SurfaceHistoryDriver::default();

    let user = test_helpers::user_text_cell(Uuid::new_v4(), "run tool");
    let orphan_result = tool_result_cell("shared-call", "Bash", "orphan output");
    let assistant = single_tool_use_cells("shared-call", "Bash", "echo unresolved");
    let real_result = tool_result_cell("shared-call", "Bash", "real output");

    let user_cells = vec![user.clone()];
    assert!(matches!(
        driver
            .emit_append_only(
                &mut terminal,
                header(),
                &user_cells,
                1,
                options(&theme, width)
            )
            .expect("emit user"),
        HistoryEmissionOutcome::Appended { .. }
    ));

    let with_orphan = cells_from_parts(&[
        std::slice::from_ref(&user),
        std::slice::from_ref(&orphan_result),
    ]);
    assert!(matches!(
        driver
            .emit_append_only(
                &mut terminal,
                header(),
                &with_orphan,
                2,
                options(&theme, width),
            )
            .expect("emit orphan result"),
        HistoryEmissionOutcome::Appended { .. }
    ));

    let before_unresolved = plain_buffer_lines(terminal.backend().buffer());
    let unresolved = cells_from_parts(&[
        std::slice::from_ref(&user),
        std::slice::from_ref(&orphan_result),
        &assistant,
    ]);
    let unresolved_outcome = driver
        .emit_append_only(
            &mut terminal,
            header(),
            &unresolved,
            3,
            options(&theme, width),
        )
        .expect("emit unresolved tool");
    assert_eq!(unresolved_outcome, HistoryEmissionOutcome::Noop);
    assert_eq!(
        plain_buffer_lines(terminal.backend().buffer()),
        before_unresolved,
        "a prior orphan result must not commit the later unresolved tool use"
    );

    let complete = cells_from_parts(&[&[user, orphan_result], &assistant, &[real_result]]);
    let complete_outcome = driver
        .emit_append_only(
            &mut terminal,
            header(),
            &complete,
            4,
            options(&theme, width),
        )
        .expect("emit completed tool");
    assert!(matches!(
        complete_outcome,
        HistoryEmissionOutcome::Appended { .. }
    ));
    let complete_text = plain_buffer_lines(terminal.backend().buffer()).join("\n");
    assert!(complete_text.contains("real output"), "{complete_text}");
}

#[test]
fn driver_does_not_reuse_one_result_for_duplicate_call_ids() {
    let theme = Theme::default();
    let width = 96;
    let backend = TestBackend::new(width, 40);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 34, width, 6));
    let mut driver = SurfaceHistoryDriver::default();

    let user = test_helpers::user_text_cell(Uuid::new_v4(), "run duplicate tools");
    let assistant = duplicate_tool_use_cells();
    let first_result = tool_result_cell("duplicate-call", "Bash", "first output");
    let second_result = tool_result_cell("duplicate-call", "Bash", "second output");

    let user_cells = vec![user.clone()];
    assert!(matches!(
        driver
            .emit_append_only(
                &mut terminal,
                header(),
                &user_cells,
                1,
                options(&theme, width)
            )
            .expect("emit user"),
        HistoryEmissionOutcome::Appended { .. }
    ));

    let one_result = cells_from_parts(&[
        std::slice::from_ref(&user),
        &assistant,
        std::slice::from_ref(&first_result),
    ]);
    let one_result_outcome = driver
        .emit_append_only(
            &mut terminal,
            header(),
            &one_result,
            2,
            options(&theme, width),
        )
        .expect("emit duplicate tools with one result");
    assert_eq!(one_result_outcome, HistoryEmissionOutcome::Noop);
    let one_result_text = plain_buffer_lines(terminal.backend().buffer()).join("\n");
    assert!(
        !one_result_text.contains("first output"),
        "{one_result_text}"
    );

    let complete = cells_from_parts(&[&[user], &assistant, &[first_result, second_result]]);
    let complete_outcome = driver
        .emit_append_only(
            &mut terminal,
            header(),
            &complete,
            3,
            options(&theme, width),
        )
        .expect("emit duplicate tools with both results");
    assert!(matches!(
        complete_outcome,
        HistoryEmissionOutcome::Appended { .. }
    ));
    assert_in_text_order(
        &plain_buffer_lines(terminal.backend().buffer()).join("\n"),
        &["first output", "second output"],
    );
}

#[test]
fn driver_replay_does_not_mark_unresolved_parallel_tool_batch_emitted() {
    let theme = Theme::default();
    let width = 96;
    let backend = TestBackend::new(width, 40);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 34, width, 6));
    let mut driver = SurfaceHistoryDriver::default();

    let user = test_helpers::user_text_cell(Uuid::new_v4(), "run tools");
    let assistant = parallel_tool_use_cells();
    let bash_result = tool_result_cell("bash-call", "Bash", "bash output");
    let glob_result = tool_result_cell("glob-call", "Glob", "glob output");
    let unresolved = cells_from_parts(&[std::slice::from_ref(&user), &assistant]);

    let replay = driver
        .replay_all_capped(
            &mut terminal,
            header(),
            &unresolved,
            2,
            options(&theme, width),
            HistoryReplayMode {
                stream_active: false,
                cause: "test_unresolved_parallel_tools",
            },
        )
        .expect("replay unresolved tools");
    assert_eq!(
        replay,
        HistoryEmissionOutcome::Replayed {
            message_count: 1,
            rows: 3,
        }
    );
    let replay_text = plain_buffer_lines(terminal.backend().buffer()).join("\n");
    assert!(!replay_text.contains("Bash"), "{replay_text}");
    assert!(!replay_text.contains("Glob"), "{replay_text}");

    let complete = cells_from_parts(&[&[user], &assistant, &[bash_result, glob_result]]);
    let complete_outcome = driver
        .emit_append_only(
            &mut terminal,
            header(),
            &complete,
            3,
            options(&theme, width),
        )
        .expect("emit completed tools after replay");
    assert!(matches!(
        complete_outcome,
        HistoryEmissionOutcome::Appended { .. }
    ));
    assert_in_text_order(
        &plain_buffer_lines(terminal.backend().buffer()).join("\n"),
        &["Bash", "bash output", "Glob", "glob output"],
    );
}

#[test]
fn driver_replay_does_not_mark_duplicate_call_batch_emitted_until_fully_paired() {
    let theme = Theme::default();
    let width = 96;
    let backend = TestBackend::new(width, 40);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 34, width, 6));
    let mut driver = SurfaceHistoryDriver::default();

    let user = test_helpers::user_text_cell(Uuid::new_v4(), "run duplicate tools");
    let assistant = duplicate_tool_use_cells();
    let first_result = tool_result_cell("duplicate-call", "Bash", "first output");
    let second_result = tool_result_cell("duplicate-call", "Bash", "second output");
    let one_result = cells_from_parts(&[
        std::slice::from_ref(&user),
        &assistant,
        std::slice::from_ref(&first_result),
    ]);

    let replay = driver
        .replay_all_capped(
            &mut terminal,
            header(),
            &one_result,
            2,
            options(&theme, width),
            HistoryReplayMode {
                stream_active: false,
                cause: "test_duplicate_call_one_result",
            },
        )
        .expect("replay duplicate tools with one result");
    assert_eq!(
        replay,
        HistoryEmissionOutcome::Replayed {
            message_count: 1,
            rows: 3,
        }
    );
    let replay_text = plain_buffer_lines(terminal.backend().buffer()).join("\n");
    assert!(!replay_text.contains("first output"), "{replay_text}");

    let complete = cells_from_parts(&[&[user], &assistant, &[first_result, second_result]]);
    let complete_outcome = driver
        .emit_append_only(
            &mut terminal,
            header(),
            &complete,
            3,
            options(&theme, width),
        )
        .expect("emit duplicate tools with both results after replay");
    assert!(matches!(
        complete_outcome,
        HistoryEmissionOutcome::Appended { .. }
    ));
    assert_in_text_order(
        &plain_buffer_lines(terminal.backend().buffer()).join("\n"),
        &["first output", "second output"],
    );
}

#[test]
fn driver_consolidates_reasoning_text_message_without_replay() {
    let theme = Theme::default();
    let width = 32;
    let backend = TestBackend::new(width, 16);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 12, width, 4));
    let mut history = initialized_driver(&mut terminal, &theme, width);
    let final_cells = assistant_reasoning_text_cells("Need to inspect files.", "alpha\n\nbeta");

    let outcome = history
        .emit_append_only(
            &mut terminal,
            header(),
            &final_cells,
            2,
            options(&theme, width),
        )
        .expect("final append");

    assert!(
        matches!(outcome, HistoryEmissionOutcome::Appended { .. }),
        "reasoning+text finalize must append once, got {outcome:?}"
    );
    let text = plain_buffer_lines(terminal.backend().buffer()).join("\n");
    assert_eq!(text.matches("alpha").count(), 1, "{text}");
    assert_eq!(text.matches("beta").count(), 1, "{text}");
    assert!(text.contains("Thinking"), "{text}");

    let alpha = text.find("alpha").expect("alpha rendered");
    let thinking = text.find("Thinking").expect("thinking rendered");
    assert!(
        alpha < thinking,
        "native committed history keeps finalized assistant text first:\n{text}"
    );
}

#[test]
fn finalized_native_history_renders_text_before_leading_thinking() {
    let theme = Theme::default();
    let width = 32;
    let cells = assistant_reasoning_text_cells("Need to inspect files.", "answer");

    let lines = render_finalized_history_lines(&cells, options(&theme, width));
    let plain = plain_lines(&lines);

    let answer = plain
        .iter()
        .position(|line| line.contains("answer"))
        .expect("answer rendered");
    let thinking = plain
        .iter()
        .position(|line| line.contains("Thinking"))
        .expect("thinking rendered");
    assert!(
        answer < thinking,
        "native finalized presentation must be text-first: {plain:#?}"
    );
}

#[test]
fn finalized_native_history_renders_text_before_thinking_when_tools_follow() {
    let theme = Theme::default();
    let message = coco_messages::create_assistant_message(
        vec![
            coco_messages::AssistantContent::Reasoning(coco_messages::ReasoningContent::new(
                "Need to inspect files.",
            )),
            coco_messages::AssistantContent::Text(coco_messages::TextContent::new("answer")),
            coco_messages::AssistantContent::ToolCall(coco_messages::ToolCallContent::new(
                "bash-call",
                "Bash",
                serde_json::json!({ "command": "echo hi" }),
            )),
        ],
        "test-model",
        coco_types::TokenUsage::default(),
    );
    let cells = message_to_cells(Arc::new(message));

    let lines = render_finalized_history_lines(&cells, options(&theme, 48));
    let plain = plain_lines(&lines).join("\n");

    let answer = plain.find("answer").expect("answer rendered");
    let thinking = plain.find("Thinking").expect("thinking rendered");
    let tool = plain.find("Bash").expect("tool rendered");
    assert!(
        answer < thinking && thinking < tool,
        "leading thinking renders after text even when tool calls follow:\n{plain}"
    );
}

#[test]
fn driver_note_viewport_schedules_stream_replay_after_resize() {
    let mut driver = SurfaceHistoryDriver::default();

    assert_eq!(
        driver.note_viewport(80, false),
        HistoryViewportChange {
            initialized: true,
            changed: false,
        }
    );
    assert_eq!(
        driver.note_viewport(100, true),
        HistoryViewportChange {
            initialized: false,
            changed: true,
        }
    );
    assert!(!driver.replay_due(Instant::now()));

    driver.reflow.force_due_for_test();

    assert!(driver.replay_due(Instant::now()));
    assert_eq!(driver.reflow.pending_viewport(), Some(100));
}

#[test]
fn driver_replay_all_replaces_owned_history_and_marks_stream_replay() {
    let theme = Theme::default();
    let backend = TestBackend::with_lines([
        "old0    ", "old1    ", "old2    ", "old3    ", "old4    ", "old5    ", "view    ",
    ]);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 6, 8, 1));
    terminal.note_history_rows_inserted(6);
    let cells = vec![test_helpers::assistant_text_cell("world")];
    let mut driver = SurfaceHistoryDriver::default();

    let outcome = driver
        .replay_all_capped(
            &mut terminal,
            header(),
            &cells,
            1,
            options(&theme, 8),
            HistoryReplayMode {
                stream_active: true,
                cause: "test_stream_replay",
            },
        )
        .expect("replay");

    assert_eq!(
        outcome,
        HistoryEmissionOutcome::Replayed {
            message_count: 1,
            rows: 3,
        }
    );
    assert_eq!(terminal.visible_history_rows(), 3);
    // History shrank to 3 rows, so the previously bottom-pinned viewport reseats
    // flush under it (y == history_bottom_y) instead of staying latched at y=6.
    assert_eq!(terminal.viewport_area(), Rect::new(0, 3, 8, 1));
    assert_eq!(
        plain_buffer_lines(terminal.backend().buffer()),
        vec![
            "header  ",
            "⏺ world ",
            "        ",
            "        ",
            "        ",
            "        ",
            "        "
        ]
    );
}

#[test]
fn driver_replay_reseats_bottom_pinned_viewport_when_history_shrinks() {
    // Regression (bottom-pinned sibling of the /clear gap): a viewport pinned
    // at the screen bottom over a tall history, then replayed down to a short
    // history (reflow / display-toggle / rewind), must reseat flush against the
    // new (shorter) history. The old behavior kept the viewport latched at the
    // bottom, stranding a large unbacked gap until the next redraw.
    let theme = Theme::default();
    let backend = TestBackend::new(48, 30);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    // Bottom-pinned: viewport at y=26 (bottom 30 == screen), history fills 0..26.
    terminal.set_viewport_area(Rect::new(0, 26, 48, 4));
    terminal.note_history_rows_inserted(26);
    let cells = vec![
        test_helpers::user_text_cell(Uuid::new_v4(), "hello"),
        test_helpers::assistant_text_cell("short reply"),
    ];
    let mut driver = SurfaceHistoryDriver::default();

    let outcome = driver
        .replay_all_capped(
            &mut terminal,
            header(),
            &cells,
            1,
            options(&theme, 48),
            HistoryReplayMode {
                stream_active: false,
                cause: "test_bottom_pinned_shrink",
            },
        )
        .expect("replay");

    let HistoryEmissionOutcome::Replayed { rows, .. } = outcome else {
        panic!("expected replay outcome, got {outcome:?}");
    };
    assert!(rows > 0 && rows < 26, "replay must shrink history: {rows}");

    // Viewport sits flush on the new (short) history bottom — no unbacked gap —
    // and is reseated up from the stale pinned row (y=26).
    assert_eq!(
        terminal.viewport_area().top(),
        terminal.history_bottom_y(),
        "bottom-pinned viewport must reseat flush under the shrunken history:\n{}",
        plain_buffer_lines(terminal.backend().buffer()).join("\n"),
    );
    assert!(
        terminal.viewport_area().top() < 26,
        "viewport must move up from the stale pinned row"
    );
}

#[test]
fn driver_replay_reseats_flowing_viewport_to_shrunken_history() {
    // Regression: `/clear` / SessionResetForResume shrinks history within one
    // frame. `sync_surface_area` ran first off the stale (pre-clear) history
    // bottom, committing a viewport far below the new short history. The replay
    // must reseat the flowing viewport flush against the freshly inserted
    // history rather than reasserting the stale committed top (which left a
    // large blank gap until the next redraw event).
    let theme = Theme::default();
    let width = 48;
    let backend = TestBackend::new(width, 54);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    // Stale committed geometry: history filled rows 0..12, viewport pinned just
    // below at y=12 (flowing — viewport bottom 16 is well above screen 54).
    terminal.set_viewport_area(Rect::new(0, 12, width, 4));
    terminal.note_history_rows_inserted(12);
    assert_eq!(terminal.history_bottom_y(), 12);

    // After the reset only the header survives.
    let cells: Vec<RenderedCell> = vec![];
    let mut driver = SurfaceHistoryDriver::default();

    let outcome = driver
        .replay_all_capped(
            &mut terminal,
            header(),
            &cells,
            2,
            options(&theme, width),
            HistoryReplayMode {
                stream_active: false,
                cause: "test_flowing_shrink",
            },
        )
        .expect("replay");

    let HistoryEmissionOutcome::Replayed { rows, .. } = outcome else {
        panic!("expected replay outcome, got {outcome:?}");
    };
    assert_eq!(rows, 1, "header is a single row");
    assert_eq!(terminal.history_bottom_y(), 1);
    // Viewport must sit flush against the new 1-row history, not the stale y=12.
    assert_eq!(terminal.viewport_area(), Rect::new(0, 1, width, 4));
}

#[test]
fn driver_replay_capped_requires_replay_on_row_width_mismatch() {
    // Prod-only path (no analog in the deleted replay_lines mirror): replay_rows
    // assembles header rows (rendered at the viewport width) and message rows
    // (rendered at options.width) via try_copy_tail_from_slices, which returns
    // None on a width mismatch → ReplayRequired. The early-return must leave the
    // driver's emission bookkeeping untouched.
    let theme = Theme::default();
    let backend = TestBackend::new(48, 30);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 6, 48, 4)); // viewport width 48
    let cells = vec![test_helpers::assistant_text_cell("hello")];
    let mut driver = SurfaceHistoryDriver::default();

    // Render the message rows at a different width (24) than the viewport (48).
    let outcome = driver
        .replay_all_capped(
            &mut terminal,
            header(),
            &cells,
            1,
            options(&theme, 24),
            HistoryReplayMode {
                stream_active: false,
                cause: "test_width_mismatch",
            },
        )
        .expect("replay");

    assert_eq!(outcome, HistoryEmissionOutcome::ReplayRequired);
    // ReplayRequired leaves emission state untouched.
    assert_eq!(driver.emitted_history_rows, 0);
    assert!(driver.header_fingerprint.is_none());
    assert!(driver.emitted_transcript_revision.is_none());
}

fn header() -> Vec<Line<'static>> {
    vec![Line::from("header")]
}

fn initialized_driver(
    terminal: &mut SurfaceTerminal<TestBackend>,
    theme: &Theme,
    width: u16,
) -> SurfaceHistoryDriver {
    let mut driver = SurfaceHistoryDriver::default();
    driver
        .emit_append_only(terminal, header(), &[], 1, options(theme, width))
        .expect("emit header");
    driver
}

fn prepared_stream_append(
    source: &str,
    width: u16,
) -> crate::surface::stream::PreparedStreamAppend {
    let mut state = AppState::new();
    let mut streaming = StreamingState::new();
    streaming.append_text(source);
    streaming.reveal_all();
    state.ui.streaming = Some(streaming);
    state.ui.display_settings.syntax_highlighting = SyntaxHighlighting::Disabled;
    let prepared = SurfaceStreamDriver::default().prepare(
        &state,
        width,
        SurfaceFramePlan {
            modal_placement: None,
            history_surface: HistorySurfaceMode::NativeScrollback,
            attention_requested: false,
        },
    );
    prepared.stream_append.expect("stable stream append")
}

fn assistant_reasoning_text_cells(reasoning: &str, text: &str) -> Vec<RenderedCell> {
    let message = coco_messages::create_assistant_message(
        vec![
            coco_messages::AssistantContent::Reasoning(coco_messages::ReasoningContent::new(
                reasoning,
            )),
            coco_messages::AssistantContent::Text(coco_messages::TextContent::new(text)),
        ],
        "test-model",
        coco_types::TokenUsage {
            output_tokens: coco_types::OutputTokens {
                reasoning: 7,
                ..Default::default()
            },
            ..Default::default()
        },
    );
    crate::transcript::derive::message_to_cells(Arc::new(message))
}

fn parallel_tool_use_cells() -> Vec<RenderedCell> {
    let message = coco_messages::create_assistant_message(
        vec![
            coco_messages::AssistantContent::ToolCall(coco_messages::ToolCallContent::new(
                "bash-call",
                "Bash",
                serde_json::json!({ "command": "echo bash" }),
            )),
            coco_messages::AssistantContent::ToolCall(coco_messages::ToolCallContent::new(
                "glob-call",
                "Glob",
                serde_json::json!({ "pattern": "*.rs" }),
            )),
        ],
        "test-model",
        coco_types::TokenUsage::default(),
    );
    message_to_cells(Arc::new(message))
}

fn single_tool_use_cells(call_id: &str, tool_name: &str, command: &str) -> Vec<RenderedCell> {
    let message = coco_messages::create_assistant_message(
        vec![coco_messages::AssistantContent::ToolCall(
            coco_messages::ToolCallContent::new(
                call_id,
                tool_name,
                serde_json::json!({ "command": command }),
            ),
        )],
        "test-model",
        coco_types::TokenUsage::default(),
    );
    message_to_cells(Arc::new(message))
}

fn duplicate_tool_use_cells() -> Vec<RenderedCell> {
    let message = coco_messages::create_assistant_message(
        vec![
            coco_messages::AssistantContent::ToolCall(coco_messages::ToolCallContent::new(
                "duplicate-call",
                "Bash",
                serde_json::json!({ "command": "echo first" }),
            )),
            coco_messages::AssistantContent::ToolCall(coco_messages::ToolCallContent::new(
                "duplicate-call",
                "Bash",
                serde_json::json!({ "command": "echo second" }),
            )),
        ],
        "test-model",
        coco_types::TokenUsage::default(),
    );
    message_to_cells(Arc::new(message))
}

fn tool_result_cell(call_id: &str, tool_name: &str, output: &str) -> RenderedCell {
    let tool_id = tool_name.parse().expect("known tool id");
    let message =
        coco_messages::create_tool_result_message(call_id, tool_name, tool_id, output, false);
    message_to_cells(Arc::new(message))
        .into_iter()
        .next()
        .expect("tool result yields a cell")
}

fn cells_from_parts(parts: &[&[RenderedCell]]) -> Vec<RenderedCell> {
    parts.iter().flat_map(|part| part.iter().cloned()).collect()
}

fn assert_in_text_order(text: &str, needles: &[&str]) {
    let mut cursor = 0;
    for needle in needles {
        let Some(offset) = text[cursor..].find(needle) else {
            panic!("missing {needle:?} after byte {cursor}:\n{text}");
        };
        cursor += offset + needle.len();
    }
}

fn options(theme: &Theme, width: u16) -> HistoryLineRenderOptions<'_> {
    HistoryLineRenderOptions {
        styles: UiStyles::new(theme),
        width,
        syntax_highlighting: SyntaxHighlighting::Disabled,
        show_system_reminders: false,
        show_thinking: false,
        cwd: None,
        kb_handle: None,
        replay_cache_policy: HistoryReplayCachePolicy::default(),
        reasoning_metadata: None,
        subagent_summaries: None,
    }
}

fn plain_buffer_lines(buffer: &Buffer) -> Vec<String> {
    buffer
        .content
        .chunks(buffer.area.width as usize)
        .map(|cells| cells.iter().map(ratatui::buffer::Cell::symbol).collect())
        .collect()
}

fn plain_terminal_text(terminal: &SurfaceTerminal<TestBackend>) -> String {
    let mut lines = plain_buffer_lines(terminal.backend().scrollback());
    lines.extend(plain_buffer_lines(terminal.backend().buffer()));
    lines.join("\n")
}

fn plain_lines(lines: &[Line<'_>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect()
}
