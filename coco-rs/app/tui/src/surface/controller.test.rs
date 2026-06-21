use pretty_assertions::assert_eq;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::*;
use crate::state::ui::StreamingState;
use crate::surface::modal::HistorySurfaceMode;
use crate::surface::modal::SurfaceFramePlan;
use crate::transcript::derive::test_helpers;

#[test]
fn native_draw_does_not_duplicate_header_across_streaming_redraws() {
    let backend = TestBackend::new(64, 14);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    apply_native_viewport(&mut terminal, Rect::new(0, 10, 64, 4));
    let mut state = AppState::new();
    state.session.provider = "deepseek-openai".to_string();
    state.session.model = "deepseek-v4-flash".to_string();
    let mut controller = NativeSurfaceController::default();
    let t0 = std::time::Instant::now();

    controller
        .draw_at(&mut terminal, &state, t0)
        .expect("startup draw");

    test_helpers::push_user_text(&mut state.session, "u1", "hello");
    let mut streaming = StreamingState::new();
    streaming.append_text("Hi");
    streaming.reveal_all();
    state.ui.streaming = Some(streaming);
    apply_native_viewport(&mut terminal, Rect::new(0, 8, 64, 6));
    controller
        .draw_at(
            &mut terminal,
            &state,
            t0 + std::time::Duration::from_millis(100),
        )
        .expect("first stream draw");

    let streaming = state.ui.streaming.as_mut().expect("streaming state");
    streaming.append_text(" there");
    streaming.reveal_all();
    apply_native_viewport(&mut terminal, Rect::new(0, 7, 64, 7));
    controller
        .draw_at(
            &mut terminal,
            &state,
            t0 + std::time::Duration::from_millis(200),
        )
        .expect("second stream draw");

    state.ui.streaming = None;
    test_helpers::push_assistant_text(&mut state.session, "Hi there");
    apply_native_viewport(&mut terminal, Rect::new(0, 10, 64, 4));
    controller
        .draw_at(
            &mut terminal,
            &state,
            t0 + std::time::Duration::from_millis(300),
        )
        .expect("final draw");

    let text = plain_terminal_text(&terminal);
    assert_eq!(text.matches("COCO").count(), 1, "{text}");
    assert_eq!(text.matches("❯ hello").count(), 1, "{text}");
    assert_eq!(text.matches("⏺ Hi there").count(), 1, "{text}");

    let lines = plain_terminal_lines(&terminal);
    let header_last = line_index(&lines, "╰─╯");
    let user = line_index(&lines, "❯ hello");
    let assistant = line_index(&lines, "⏺ Hi there");
    assert_eq!(user, header_last + 2, "{text}");
    assert!(lines[header_last + 1].trim().is_empty(), "{text}");
    assert_eq!(assistant, user + 2, "{text}");
    assert!(lines[user + 1].trim().is_empty(), "{text}");
}

#[test]
fn native_draw_emits_session_header_on_startup() {
    let backend = TestBackend::new(64, 8);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 4, 64, 4));
    let mut state = AppState::new();
    state.session.provider = "deepseek-openai".to_string();
    state.session.model = "deepseek-v4-flash".to_string();
    let mut controller = NativeSurfaceController::default();

    let outcome = controller.draw(&mut terminal, &state).expect("draw");

    assert_eq!(
        outcome.history,
        HistoryEmissionOutcome::Appended {
            start: 0,
            message_count: 0,
            rows: 4,
        }
    );
    let text = plain_terminal_text(&terminal);
    assert!(text.contains("COCO"));
    assert!(text.contains("deepseek-openai/deepseek-v4-flash"));
}

#[test]
fn native_draw_appends_finalized_history_and_keeps_live_tail_in_viewport() {
    // +1 row vs. the original 11/6 so the live tail keeps its size now that the
    // baseline status bar occupies 2 rows (mode label + cycle hint).
    let backend = TestBackend::new(48, 12);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 5, 48, 7));
    let mut state = AppState::new();
    test_helpers::push_assistant_text(&mut state.session, "finalized");
    let mut streaming = StreamingState::new();
    streaming.append_text("live response");
    streaming.reveal_all();
    state.ui.streaming = Some(streaming);
    let mut controller = NativeSurfaceController::default();

    let outcome = controller.draw(&mut terminal, &state).expect("draw");

    assert_eq!(
        outcome.history,
        HistoryEmissionOutcome::Appended {
            start: 0,
            message_count: 1,
            rows: 6,
        }
    );
    let text = plain_terminal_text(&terminal);
    assert!(text.contains("COCO"));
    assert!(text.contains("live response"), "{text}");
}

#[test]
fn native_draw_defers_parallel_tool_batch_until_all_results_arrive() {
    let backend = TestBackend::new(96, 28);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 22, 96, 6));
    let mut state = AppState::new();
    test_helpers::push_user_text(&mut state.session, "u1", "run tools");
    let mut controller = NativeSurfaceController::default();

    controller
        .draw(&mut terminal, &state)
        .expect("initial draw");

    push_parallel_tool_uses(&mut state);
    let unresolved = controller
        .draw(&mut terminal, &state)
        .expect("unresolved draw");
    assert_eq!(unresolved.history, HistoryEmissionOutcome::Noop);
    let unresolved_text = plain_terminal_text(&terminal);
    assert!(!unresolved_text.contains("Bash"), "{unresolved_text}");
    assert!(!unresolved_text.contains("Glob"), "{unresolved_text}");

    test_helpers::push_tool_result(
        &mut state.session,
        "bash-call",
        "Bash",
        "bash output",
        false,
    );
    let partial = controller
        .draw(&mut terminal, &state)
        .expect("partially resolved draw");
    assert_eq!(partial.history, HistoryEmissionOutcome::Noop);
    let partial_text = plain_terminal_text(&terminal);
    assert!(!partial_text.contains("Bash"), "{partial_text}");
    assert!(!partial_text.contains("bash output"), "{partial_text}");

    test_helpers::push_tool_result(
        &mut state.session,
        "glob-call",
        "Glob",
        "glob output",
        false,
    );
    let complete = controller
        .draw(&mut terminal, &state)
        .expect("completed draw");
    assert!(matches!(
        complete.history,
        HistoryEmissionOutcome::Appended { .. }
    ));
    assert_in_text_order(
        &plain_terminal_text(&terminal),
        &["Bash", "bash output", "Glob", "glob output"],
    );
}

#[test]
fn native_draw_streams_in_viewport_then_appends_final_message_once() {
    // +1 row vs. the original 18/6 to absorb the 2-row baseline status bar.
    let backend = TestBackend::new(64, 19);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 12, 64, 7));
    let mut state = AppState::new();
    let mut controller = NativeSurfaceController::default();
    controller
        .draw(&mut terminal, &state)
        .expect("initial draw");
    let history_rows_before_stream = terminal.visible_history_rows();

    let mut streaming = StreamingState::new();
    streaming.append_text("alpha\n\nbeta");
    streaming.reveal_all();
    state.ui.streaming = Some(streaming);
    controller.draw(&mut terminal, &state).expect("stream draw");

    assert!(
        terminal.visible_history_rows() > history_rows_before_stream,
        "stable stream rows should be inserted into native history"
    );
    let streaming_text = plain_terminal_text(&terminal);
    assert_eq!(
        streaming_text.matches("alpha").count(),
        1,
        "{streaming_text}"
    );
    assert_eq!(
        streaming_text.matches("beta").count(),
        1,
        "{streaming_text}"
    );

    state.ui.streaming = None;
    test_helpers::push_assistant_text(&mut state.session, "alpha\n\nbeta");
    controller
        .draw(&mut terminal, &state)
        .expect("final consolidation draw");

    let final_text = plain_terminal_text(&terminal);
    assert_eq!(final_text.matches("alpha").count(), 1, "{final_text}");
    assert_eq!(final_text.matches("beta").count(), 1, "{final_text}");
}

#[test]
fn native_draw_keeps_stream_visible_with_stable_prefix_in_native_history() {
    let backend = TestBackend::new(64, 12);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 2, 64, 10));
    let mut state = AppState::new();
    let mut controller = NativeSurfaceController::default();
    let plan = SurfaceFramePlan {
        modal_placement: None,
        history_surface: HistorySurfaceMode::NativeScrollback,
        attention_requested: false,
    };

    let mut streaming = StreamingState::new();
    streaming.append_text("alpha\n\nbeta");
    streaming.reveal_all();
    state.ui.streaming = Some(streaming);
    controller
        .draw_with_plan(&mut terminal, &state, plan, None)
        .expect("stream draw");

    assert!(terminal.visible_history_rows() > 0);
    let text = plain_terminal_text(&terminal);
    assert_eq!(text.matches("alpha").count(), 1, "{text}");
    assert_eq!(text.matches("beta").count(), 1, "{text}");
}

#[test]
fn native_draw_keeps_stream_visible_after_mid_stream_resize() {
    // +1 row vs. the original 18/6 to absorb the 2-row baseline status bar.
    let backend = TestBackend::new(64, 19);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 12, 64, 7));
    let mut state = AppState::new();
    let mut controller = NativeSurfaceController::default();
    controller
        .draw(&mut terminal, &state)
        .expect("initial draw");

    let mut streaming = StreamingState::new();
    streaming.append_text("alpha\n\nbeta\ngamma");
    streaming.reveal_all();
    state.ui.streaming = Some(streaming);
    controller.draw(&mut terminal, &state).expect("stream draw");

    terminal.set_viewport_area(Rect::new(0, 12, 32, 7));
    controller
        .draw(&mut terminal, &state)
        .expect("resize stream draw");

    let resized_text = plain_terminal_text(&terminal);
    assert_eq!(resized_text.matches("gamma").count(), 1, "{resized_text}");

    state.ui.streaming = None;
    test_helpers::push_assistant_text(&mut state.session, "alpha\n\nbeta\ngamma");
    let outcome = controller
        .draw(&mut terminal, &state)
        .expect("final append draw");

    assert!(matches!(
        outcome.history,
        HistoryEmissionOutcome::Replayed { .. } | HistoryEmissionOutcome::Appended { .. }
    ));
    let final_text = plain_terminal_text(&terminal);
    assert_eq!(final_text.matches("alpha").count(), 1, "{final_text}");
    assert_eq!(final_text.matches("beta").count(), 1, "{final_text}");
    assert_eq!(final_text.matches("gamma").count(), 1, "{final_text}");
}

#[test]
fn native_draw_finalizes_after_turn_end_shrink_without_full_replay() {
    let backend = TestBackend::new(64, 18);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 10, 64, 8));
    let mut state = AppState::new();
    let mut controller = NativeSurfaceController::default();
    controller
        .draw(&mut terminal, &state)
        .expect("initial draw");

    let mut streaming = StreamingState::new();
    streaming.append_text("alpha\n\nbeta");
    streaming.reveal_all();
    state.ui.streaming = Some(streaming);
    controller.draw(&mut terminal, &state).expect("stream draw");

    state.ui.streaming = None;
    test_helpers::push_assistant_text(&mut state.session, "alpha\n\nbeta");
    terminal.set_viewport_area(Rect::new(0, 14, 64, 4));
    let outcome = controller
        .draw(&mut terminal, &state)
        .expect("final shrink draw");

    assert!(
        !matches!(outcome.history, HistoryEmissionOutcome::Replayed { .. }),
        "ordinary turn-end shrink must finalize by appending the assistant cell: {:?}",
        outcome.history
    );
    let text = plain_terminal_text(&terminal);
    assert_eq!(text.matches("beta").count(), 1, "{text}");
}

#[test]
fn native_draw_replays_finalized_history_when_theme_changes_at_finalize() {
    let backend = TestBackend::new(64, 18);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 12, 64, 6));
    let mut state = AppState::new();
    let mut controller = NativeSurfaceController::default();
    controller
        .draw(&mut terminal, &state)
        .expect("initial draw");

    let mut streaming = StreamingState::new();
    streaming.append_text("alpha\n\nbeta");
    streaming.reveal_all();
    state.ui.streaming = Some(streaming);
    controller.draw(&mut terminal, &state).expect("stream draw");

    state.ui.theme.accent = ratatui::style::Color::LightRed;
    state.ui.streaming = None;
    test_helpers::push_assistant_text(&mut state.session, "alpha\n\nbeta");
    let outcome = controller
        .draw(&mut terminal, &state)
        .expect("final append draw");

    assert!(matches!(
        outcome.history,
        HistoryEmissionOutcome::Replayed { .. }
    ));
    let final_text = plain_terminal_text(&terminal);
    assert_eq!(final_text.matches("alpha").count(), 1, "{final_text}");
    assert_eq!(final_text.matches("beta").count(), 1, "{final_text}");
}

#[test]
fn native_draw_fast_noops_when_transcript_revision_is_unchanged() {
    let backend = TestBackend::new(48, 9);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 5, 48, 4));
    let mut state = AppState::new();
    test_helpers::push_assistant_text(&mut state.session, "stable");
    let mut controller = NativeSurfaceController::default();
    controller.draw(&mut terminal, &state).expect("first draw");
    let revision = state.session.transcript.revision();

    let outcome = controller.draw(&mut terminal, &state).expect("second draw");

    assert_eq!(
        outcome.history,
        HistoryEmissionOutcome::FastNoop { revision }
    );
}

#[test]
fn native_draw_replays_finalized_history_when_theme_changes() {
    let backend = TestBackend::new(48, 9);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 5, 48, 4));
    let mut state = AppState::new();
    test_helpers::push_assistant_text(&mut state.session, "stable");
    let mut controller = NativeSurfaceController::default();
    controller.draw(&mut terminal, &state).expect("first draw");

    state.ui.theme.accent = ratatui::style::Color::LightRed;
    let outcome = controller
        .draw(&mut terminal, &state)
        .expect("theme replay");

    assert!(matches!(
        outcome.history,
        HistoryEmissionOutcome::Replayed { .. }
    ));
}

#[test]
fn native_draw_appends_after_transcript_revision_changes() {
    let backend = TestBackend::new(48, 12);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 8, 48, 4));
    let mut state = AppState::new();
    test_helpers::push_assistant_text(&mut state.session, "one");
    let mut controller = NativeSurfaceController::default();
    controller.draw(&mut terminal, &state).expect("first draw");

    test_helpers::push_assistant_text(&mut state.session, "two");
    let outcome = controller.draw(&mut terminal, &state).expect("append draw");

    assert!(matches!(
        outcome.history,
        HistoryEmissionOutcome::Appended {
            message_count: 1,
            ..
        }
    ));
    let text = plain_terminal_text(&terminal);
    assert!(text.contains("one"), "{text}");
    assert!(text.contains("two"), "{text}");
}

#[test]
fn native_draw_keeps_stream_visible_after_width_replay() {
    // +1 row vs. the original 18/6 to absorb the 2-row baseline status bar.
    let backend = TestBackend::new(64, 19);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 12, 64, 7));
    let mut state = AppState::new();
    let mut controller = NativeSurfaceController::default();
    controller
        .draw(&mut terminal, &state)
        .expect("initial draw");

    let mut streaming = StreamingState::new();
    streaming.append_text("alpha\n\nbeta");
    streaming.reveal_all();
    state.ui.streaming = Some(streaming);
    controller.draw(&mut terminal, &state).expect("stream draw");

    // A width change (terminal resize) reflows committed history — immediately
    // if the resized buffer forces it, otherwise after the debounce. Streaming
    // text remains viewport-only and must stay visible exactly once.
    terminal.set_viewport_area(Rect::new(0, 12, 60, 7));
    let immediate = controller
        .draw(&mut terminal, &state)
        .expect("width change draw");
    let immediate_text = plain_terminal_text(&terminal);
    assert_eq!(
        immediate_text.matches("alpha").count(),
        1,
        "{immediate_text}"
    );
    assert_eq!(
        immediate_text.matches("beta").count(),
        1,
        "{immediate_text}"
    );
    let debounced = controller
        .draw_at(
            &mut terminal,
            &state,
            std::time::Instant::now() + std::time::Duration::from_millis(100),
        )
        .expect("width replay");

    assert!(
        matches!(immediate.history, HistoryEmissionOutcome::Replayed { .. })
            || matches!(debounced.history, HistoryEmissionOutcome::Replayed { .. }),
        "a width resize must replay history: {:?} then {:?}",
        immediate.history,
        debounced.history,
    );
    let text = plain_terminal_text(&terminal);
    assert_eq!(text.matches("alpha").count(), 1, "{text}");
    assert_eq!(text.matches("beta").count(), 1, "{text}");
}

#[test]
fn native_draw_replays_history_when_source_prefix_diverges() {
    let backend = TestBackend::new(48, 8);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 5, 48, 3));
    let mut state = AppState::new();
    test_helpers::push_assistant_text(&mut state.session, "one");
    let mut controller = NativeSurfaceController::default();
    controller.draw(&mut terminal, &state).expect("first draw");

    // Reset the engine-authoritative transcript so the prefix-divergence
    // path fires (the renderer reads cells).
    state.session.transcript.on_session_reset();
    test_helpers::push_assistant_text(&mut state.session, "two");
    let outcome = controller.draw(&mut terminal, &state).expect("replay");

    assert_eq!(
        outcome.history,
        HistoryEmissionOutcome::Replayed {
            message_count: 1,
            rows: 6,
        }
    );
    let text = plain_buffer_lines(terminal.backend().buffer()).join("\n");
    assert!(!text.contains("one"));
    assert!(text.contains("two"));
}

#[test]
fn native_draw_replays_history_when_thinking_display_changes() {
    let backend = TestBackend::new(64, 10);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 6, 64, 4));
    let mut state = AppState::new();
    test_helpers::push_assistant_thinking(&mut state.session, "Need to inspect files.", 1300, 15);
    let mut controller = NativeSurfaceController::default();
    controller.draw(&mut terminal, &state).expect("first draw");

    state.ui.show_thinking = true;
    let outcome = controller
        .draw(&mut terminal, &state)
        .expect("thinking replay");

    assert!(matches!(
        outcome.history,
        HistoryEmissionOutcome::Replayed {
            message_count: 1,
            ..
        }
    ));
    let text = plain_buffer_lines(terminal.backend().buffer()).join("\n");
    assert!(text.contains("Need to inspect files."));
}

#[test]
fn native_draw_bakes_reasoning_metadata_without_full_replay() {
    // A per-turn reasoning-metadata attach must NOT trigger `replay_all_capped`.
    // The engine emits `MessageAppended` + `ReasoningMetadataAttached` back to
    // back at turn finalize and the TUI coalesces them into one draw, so the
    // duration/tokens are in the side-cache when the assistant cell first
    // commits — they bake into its single append-only emit (mirrors how
    // claude-code-kim bakes the metadata footer into the finalized element).
    let backend = TestBackend::new(64, 10);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 6, 64, 4));
    let mut state = AppState::new();
    let msg = coco_messages::create_assistant_message(
        vec![coco_messages::AssistantContent::Reasoning(
            coco_messages::ReasoningContent::new("Need to inspect files."),
        )],
        "test-model",
        coco_types::TokenUsage::default(),
    );
    let uuid = match &msg {
        coco_messages::Message::Assistant(a) => a.uuid,
        _ => unreachable!("create_assistant_message yields Assistant"),
    };
    state
        .session
        .transcript
        .on_message_appended(std::sync::Arc::new(msg));
    // Metadata in the side-cache before the cell's first (and only) commit.
    state.session.insert_reasoning_metadata(
        uuid,
        crate::state::session::ReasoningMetadata {
            duration_ms: None,
            reasoning_tokens: 22,
        },
    );

    let mut controller = NativeSurfaceController::default();
    let outcome = controller
        .draw(&mut terminal, &state)
        .expect("finalize draw");

    // Append-only emit, never a full replay — that per-turn rewrite is the cost
    // this change removes.
    assert!(
        !matches!(outcome.history, HistoryEmissionOutcome::Replayed { .. }),
        "reasoning metadata must not force a full history replay: {:?}",
        outcome.history
    );
    let text = plain_buffer_lines(terminal.backend().buffer()).join("\n");
    assert!(
        text.contains("22 reasoning tok"),
        "metadata baked in: {text}"
    );
}

#[test]
fn native_draw_appends_after_resize_requested_during_stream_finishes() {
    let backend = TestBackend::new(48, 8);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 5, 20, 3));
    let mut state = AppState::new();
    state.ui.streaming = Some(StreamingState::new());
    let mut controller = NativeSurfaceController::default();

    controller
        .draw(&mut terminal, &state)
        .expect("initial draw");
    terminal.set_viewport_area(Rect::new(0, 5, 30, 3));
    controller.draw(&mut terminal, &state).expect("resize draw");

    state.ui.streaming = None;
    test_helpers::push_assistant_text(&mut state.session, "done");
    let outcome = controller.draw(&mut terminal, &state).expect("finish draw");

    assert!(
        !matches!(outcome.history, HistoryEmissionOutcome::Replayed { .. }),
        "stream finish must not force a replay before width reflow is due: {:?}",
        outcome.history
    );
    let text = plain_buffer_lines(terminal.backend().buffer()).join("\n");
    assert!(text.contains("done"));
}

#[test]
fn native_draw_stream_finish_append_does_not_leave_gap_before_input() {
    let backend = TestBackend::new(64, 30);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 26, 64, 4));
    let mut state = AppState::new();
    let mut controller = NativeSurfaceController::default();

    controller
        .draw(&mut terminal, &state)
        .expect("initial draw");

    test_helpers::push_user_text(&mut state.session, "u1", "hello");
    let mut streaming = StreamingState::new();
    streaming.append_text("short reply");
    streaming.reveal_all();
    state.ui.streaming = Some(streaming);
    // Resize the terminal *width* mid-stream. Reflow is still width-keyed, but
    // merely finishing the stream must not force an immediate full replay.
    apply_native_viewport(&mut terminal, Rect::new(0, 18, 60, 12));
    controller.draw(&mut terminal, &state).expect("stream draw");

    state.ui.streaming = None;
    test_helpers::push_assistant_text(&mut state.session, "short reply");
    apply_native_viewport(&mut terminal, Rect::new(0, 26, 64, 4));
    let outcome = controller.draw(&mut terminal, &state).expect("finish draw");

    assert!(
        !matches!(outcome.history, HistoryEmissionOutcome::Replayed { .. }),
        "stream finish must append residual history, not replay: {:?}",
        outcome.history
    );
    let text = plain_terminal_text(&terminal);
    assert_eq!(text.matches("⏺ short reply").count(), 1, "{text}");
}

#[test]
fn native_draw_keeps_input_bottom_stable_across_bottom_pinned_turn_states() {
    let backend = TestBackend::new(64, 24);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    let mut state = AppState::new();
    let mut controller = NativeSurfaceController::default();

    let mut streaming = StreamingState::new();
    streaming.append_text("first\n\nsecond\n\nthird");
    streaming.reveal_all();
    state.ui.streaming = Some(streaming);
    apply_bottom_pinned_viewport(&mut terminal, /*height*/ 12);
    let streaming = controller
        .draw(&mut terminal, &state)
        .expect("streaming draw");
    let input_bottom = streaming.layout.input.bottom();
    // 22 (not 23): the 2-row baseline status bar lifts the composer one row.
    assert_eq!(input_bottom, 22);

    state.ui.streaming = None;
    test_helpers::push_assistant_text(&mut state.session, "first\n\nsecond\n\nthird");
    apply_bottom_pinned_viewport(&mut terminal, /*height*/ 5);
    let turn_end = controller
        .draw(&mut terminal, &state)
        .expect("turn-end draw");
    assert_eq!(turn_end.layout.input.bottom(), input_bottom);

    state.session.prompt_suggestions = vec!["Run the focused surface tests".into()];
    apply_bottom_pinned_viewport(&mut terminal, /*height*/ 5);
    let prompt_suggestion = controller
        .draw(&mut terminal, &state)
        .expect("prompt suggestion draw");
    assert_eq!(prompt_suggestion.layout.input.bottom(), input_bottom);

    state.session.prompt_suggestions.clear();
    test_helpers::push_tool_use(&mut state.session, "call-1", "Read", "Cargo.toml");
    test_helpers::push_tool_result(
        &mut state.session,
        "call-1",
        "Read",
        "[workspace]\nmembers = []",
        false,
    );
    apply_bottom_pinned_viewport(&mut terminal, /*height*/ 5);
    let tool_result = controller
        .draw(&mut terminal, &state)
        .expect("tool-result draw");
    assert_eq!(tool_result.layout.input.bottom(), input_bottom);
}

#[test]
fn native_draw_does_not_replay_on_viewport_height_change() {
    // History re-wrap depends only on width. A viewport *height* change (the
    // live tail growing/shrinking during streaming) must NOT schedule a
    // full-history replay — that per-frame replay was the streaming flicker.
    let backend = TestBackend::new(48, 12);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 7, 48, 3));
    let mut state = AppState::new();
    test_helpers::push_assistant_text(&mut state.session, "height replay");
    let mut controller = NativeSurfaceController::default();
    controller
        .draw(&mut terminal, &state)
        .expect("initial draw");

    // Height-only change (width stays 48).
    terminal.set_viewport_area(Rect::new(0, 7, 48, 4));
    let height_change = controller
        .draw(&mut terminal, &state)
        .expect("height change draw");
    assert!(matches!(
        height_change.history,
        HistoryEmissionOutcome::FastNoop { .. }
    ));

    // Even past the reflow debounce window, no replay is scheduled.
    let outcome = controller
        .draw_at(
            &mut terminal,
            &state,
            std::time::Instant::now() + std::time::Duration::from_millis(100),
        )
        .expect("post-debounce draw");
    assert!(
        !matches!(outcome.history, HistoryEmissionOutcome::Replayed { .. }),
        "viewport height change must not trigger a history replay: {:?}",
        outcome.history
    );
}

#[test]
fn native_draw_defers_history_while_modal_is_open() {
    let backend = TestBackend::new(48, 8);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 5, 48, 3));
    let mut state = AppState::new();
    test_helpers::push_assistant_text(&mut state.session, "deferred");
    state.ui.show_modal(crate::state::ModalState::Help);
    let mut controller = NativeSurfaceController::default();

    let outcome = controller.draw(&mut terminal, &state).expect("draw");

    assert_eq!(outcome.history, HistoryEmissionOutcome::Noop);
    let text = plain_buffer_lines(terminal.backend().buffer()).join("\n");
    assert!(!text.contains("deferred"));
}

#[test]
fn native_draw_renders_finalized_history_in_viewport_when_terminal_is_incompatible() {
    let backend = TestBackend::new(48, 12);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 0, 48, 12));
    let mut state = AppState::new();
    test_helpers::push_assistant_text(&mut state.session, "zellij deferred");
    let mut controller = NativeSurfaceController::default();
    let plan = SurfaceFramePlan {
        modal_placement: None,
        history_surface: HistorySurfaceMode::Viewport,
        attention_requested: false,
    };

    let outcome = controller
        .draw_with_plan(&mut terminal, &state, plan, None)
        .expect("draw");

    assert_eq!(outcome.history, HistoryEmissionOutcome::Noop);
    let text = plain_buffer_lines(terminal.backend().buffer()).join("\n");
    assert!(text.contains("zellij deferred"));
}

fn plain_buffer_lines(buffer: &Buffer) -> Vec<String> {
    buffer
        .content
        .chunks(buffer.area.width as usize)
        .map(|cells| cells.iter().map(ratatui::buffer::Cell::symbol).collect())
        .collect()
}

fn plain_terminal_text(terminal: &SurfaceTerminal<TestBackend>) -> String {
    plain_terminal_lines(terminal).join("\n")
}

fn plain_terminal_lines(terminal: &SurfaceTerminal<TestBackend>) -> Vec<String> {
    let mut lines = plain_buffer_lines(terminal.backend().scrollback());
    lines.extend(plain_buffer_lines(terminal.backend().buffer()));
    lines
}

fn line_index(lines: &[String], needle: &str) -> usize {
    lines
        .iter()
        .position(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("missing {needle:?} in {lines:#?}"))
}

fn push_parallel_tool_uses(state: &mut AppState) {
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
    state
        .session
        .transcript
        .on_message_appended(std::sync::Arc::new(message));
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

fn apply_native_viewport(terminal: &mut SurfaceTerminal<TestBackend>, area: Rect) {
    terminal
        .apply_viewport_area(area, true)
        .expect("apply viewport area");
}

fn apply_bottom_pinned_viewport(terminal: &mut SurfaceTerminal<TestBackend>, height: u16) {
    let size = terminal.size().expect("test backend size");
    apply_native_viewport(
        terminal,
        Rect::new(0, size.height.saturating_sub(height), size.width, height),
    );
}
