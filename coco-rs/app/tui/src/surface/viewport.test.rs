use pretty_assertions::assert_eq;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::*;
use crate::state::ui::StreamingState;
use crate::surface::modal::HistorySurfaceMode;
use crate::transcript::derive::test_helpers;
use coco_tui_ui::engine::terminal::SurfaceTerminal;

#[test]
fn interactive_viewport_does_not_render_session_header() {
    let backend = TestBackend::new(48, 6);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 0, 48, 6));
    let state = AppState::new();
    let mut transcript_layout = crate::widgets::TranscriptLayoutIndex::default();

    terminal
        .draw_viewport(|frame| {
            render_interactive_viewport(frame, &state, native_plan(), &mut transcript_layout, None);
        })
        .expect("draw");

    let text = plain_buffer_lines(terminal.backend().buffer()).join("\n");
    assert!(!text.contains("COCO"));
    assert!(!text.contains("Type a message"));
    assert!(text.contains("❯"));
}

#[test]
fn interactive_viewport_desired_height_tracks_idle_composer() {
    let state = AppState::new();

    assert_eq!(
        interactive_viewport_desired_height(&state, 48, 12, native_plan(), None),
        5
    );
}

#[test]
fn interactive_viewport_desired_height_never_exceeds_cap() {
    let state = AppState::new();

    assert_eq!(
        interactive_viewport_desired_height(&state, 48, 2, native_plan(), None),
        2
    );
}

#[test]
fn interactive_viewport_popup_height_tracks_item_count() {
    // Content-sized popup (mirrors codex's `calculate_required_height`): the
    // reserved rows — and thus the composer's vertical shift — scale with the
    // item count, capped at DEFAULT_MAX_VISIBLE. A short list no longer
    // reserves the full budget. Layout: 3 fixed composer rows + N popup rows.
    let short = state_with_popup_items(2);
    let medium = state_with_popup_items(5);
    let full = state_with_popup_items(SuggestionPopup::DEFAULT_MAX_VISIBLE as usize);
    let over_cap = state_with_popup_items(SuggestionPopup::DEFAULT_MAX_VISIBLE as usize + 5);

    assert_eq!(
        interactive_viewport_desired_height(&short, 48, 24, native_plan(), None),
        5
    );
    assert_eq!(
        interactive_viewport_desired_height(&medium, 48, 24, native_plan(), None),
        8
    );
    assert_eq!(
        interactive_viewport_desired_height(&full, 48, 24, native_plan(), None),
        11
    );
    // Beyond the cap the reservation clamps to DEFAULT_MAX_VISIBLE.
    assert_eq!(
        interactive_viewport_desired_height(&over_cap, 48, 24, native_plan(), None),
        11
    );
}

#[test]
fn interactive_viewport_popup_height_is_capped_by_terminal_height() {
    let state = state_with_popup_items(SuggestionPopup::DEFAULT_MAX_VISIBLE as usize);

    assert_eq!(
        interactive_viewport_desired_height(&state, 48, 8, native_plan(), None),
        8
    );
}

#[test]
fn exit_plan_prompt_height_is_independent_of_plan_length() {
    let state = exit_plan_prompt_state(80);

    let height = interaction_prompt_height(&state, 96, 40);

    assert!(
        height <= 7,
        "prompt should reserve only decision rows, got {height}"
    );
}

#[test]
fn exit_plan_pending_plan_renders_in_live_tail() {
    let state = exit_plan_prompt_state(40);
    let styles = UiStyles::new(&state.ui.theme);

    let lines = build_live_tail_lines(&state, styles, 96, native_plan());
    let text = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(text.contains("Here is proposed plan:"), "{text}");
    assert!(text.contains("step 40"), "{text}");
    assert!(text.contains("Plan file: /tmp/plan.md"), "{text}");
    assert!(!text.contains("clear context"), "{text}");
}

#[test]
fn interactive_viewport_does_not_render_finalized_messages() {
    let backend = TestBackend::new(48, 8);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 0, 48, 8));
    let mut state = AppState::new();
    test_helpers::push_assistant_text(&mut state.session, "finalized history");
    let mut transcript_layout = crate::widgets::TranscriptLayoutIndex::default();

    terminal
        .draw_viewport(|frame| {
            render_interactive_viewport(frame, &state, native_plan(), &mut transcript_layout, None);
        })
        .expect("draw");

    let text = plain_buffer_lines(terminal.backend().buffer()).join("\n");
    assert!(!text.contains("finalized history"));
}

#[test]
fn interactive_viewport_renders_finalized_messages_in_viewport_history_mode() {
    let backend = TestBackend::new(48, 8);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 0, 48, 8));
    let mut state = AppState::new();
    test_helpers::push_assistant_text(&mut state.session, "fallback history");
    let mut transcript_layout = crate::widgets::TranscriptLayoutIndex::default();

    terminal
        .draw_viewport(|frame| {
            render_interactive_viewport(
                frame,
                &state,
                viewport_history_plan(),
                &mut transcript_layout,
                None,
            );
        })
        .expect("draw");

    let text = plain_buffer_lines(terminal.backend().buffer()).join("\n");
    assert!(text.contains("fallback history"));
}

#[test]
fn interactive_viewport_renders_active_streaming_tail() {
    let backend = TestBackend::new(48, 10);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 0, 48, 10));
    let mut state = AppState::new();
    let mut streaming = StreamingState::new();
    streaming.append_text("live response");
    streaming.reveal_all();
    state.ui.streaming = Some(streaming);
    let mut transcript_layout = crate::widgets::TranscriptLayoutIndex::default();

    terminal
        .draw_viewport(|frame| {
            render_interactive_viewport(frame, &state, native_plan(), &mut transcript_layout, None);
        })
        .expect("draw");

    let text = plain_buffer_lines(terminal.backend().buffer()).join("\n");
    assert!(text.contains("live response"));
}

#[test]
fn interactive_viewport_reports_input_rect_for_cursor_policy() {
    let backend = TestBackend::new(48, 8);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 0, 48, 8));
    let state = AppState::new();
    let mut layout = FrameLayout::default();
    let mut transcript_layout = crate::widgets::TranscriptLayoutIndex::default();

    terminal
        .draw_viewport(|frame| {
            layout = render_interactive_viewport(
                frame,
                &state,
                native_plan(),
                &mut transcript_layout,
                None,
            );
        })
        .expect("draw");

    assert_eq!(layout.input.height, 3);
    assert_eq!(layout.input.width, 48);
}

#[test]
fn question_prompt_sets_input_height_to_zero() {
    let backend = TestBackend::new(48, 10);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 0, 48, 10));
    let state = question_state(vec![question_item("Short", "Short?", 1)]);
    let mut layout = FrameLayout::default();
    let mut transcript_layout = crate::widgets::TranscriptLayoutIndex::default();

    terminal
        .draw_viewport(|frame| {
            layout = render_interactive_viewport(
                frame,
                &state,
                native_plan(),
                &mut transcript_layout,
                None,
            );
        })
        .expect("draw");

    assert_eq!(layout.input.height, 0);
}

#[test]
fn question_prompt_uses_full_viewport_width() {
    let backend = TestBackend::new(140, 14);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 0, 140, 14));
    let state = question_state(vec![question_item("Short", "Short?", 1)]);
    let mut layout = FrameLayout::default();
    let mut transcript_layout = crate::widgets::TranscriptLayoutIndex::default();

    terminal
        .draw_viewport(|frame| {
            layout = render_interactive_viewport(
                frame,
                &state,
                native_plan(),
                &mut transcript_layout,
                None,
            );
        })
        .expect("draw");

    assert_eq!(layout.question_prompt.width, 140);
}

#[test]
fn question_prompt_desired_height_uses_tallest_question_tab() {
    let state = question_state(vec![
        question_item("Short", "Short?", 1),
        question_item("Long", &"Long question ".repeat(20), 4),
    ]);
    let short_only = question_state(vec![question_item("Short", "Short?", 1)]);

    let with_tall_tab = interactive_viewport_desired_height(&state, 48, 24, native_plan(), None);
    let short_height =
        interactive_viewport_desired_height(&short_only, 48, 24, native_plan(), None);

    assert!(
        with_tall_tab > short_height,
        "question prompt should reserve tallest tab height"
    );
}

#[test]
fn compact_prompt_body_preserves_tail_action_block() {
    let body = "\
Execute shell command

Command:
  rm -rf /tmp/test

Risk:
  Removes files recursively

Actions:
▸ Yes, approve once
  Yes, always allow Bash for this session
  No, deny
↑/↓ Navigate  Enter Select  Y/N/A shortcuts";

    let compact = compact_prompt_body(body, 7);

    assert_eq!(
        compact,
        "\
Execute shell command
...
Actions:
▸ Yes, approve once
  Yes, always allow Bash for this session
  No, deny
↑/↓ Navigate  Enter Select  Y/N/A shortcuts"
    );
}

fn exit_plan_prompt_state(step_count: usize) -> AppState {
    let mut state = AppState::new();
    let plan: String = (1..=step_count).map(|i| format!("- step {i}\n")).collect();
    state
        .ui
        .push_prompt(crate::state::PanePromptState::Permission(
            crate::state::PermissionPromptState {
                request_id: "req-1".into(),
                tool_name: coco_types::ToolName::ExitPlanMode.as_str().into(),
                description: "Exit plan mode?".into(),
                detail: crate::state::PermissionDetail::ExitPlanMode {
                    outcome: coco_types::ExitPlanModeOutcome::ImplementationPlan,
                    plan: Some(plan),
                    plan_file_path: Some("/tmp/plan.md".into()),
                    allowed_prompts: vec![],
                },
                risk_level: None,
                show_always_allow: false,
                classifier_checking: false,
                classifier_auto_approved: None,
                choices: Some(vec![
                    coco_types::PermissionAskChoice {
                        value: coco_types::ExitPlanChoice::ClearAcceptEdits.as_str().into(),
                        label: "Yes, clear context and auto-accept edits".into(),
                        description: None,
                    },
                    coco_types::PermissionAskChoice {
                        value: coco_types::ExitPlanChoice::KeepDefault.as_str().into(),
                        label: "Yes, manually approve edits".into(),
                        description: None,
                    },
                    coco_types::PermissionAskChoice {
                        value: coco_types::ExitPlanChoice::No.as_str().into(),
                        label: "No, keep planning".into(),
                        description: None,
                    },
                ]),
                selected_choice: 0,
                display_input: coco_types::PermissionDisplayInput::Empty,
                original_input: None,
                cwd: None,
                permission_suggestions: vec![],
                worker_badge: None,
                explanation_visible: false,
                explanation: crate::state::ExplainerFetch::NotFetched,
                prefix_input: None,
            },
        ));
    state
}

fn native_plan() -> SurfaceFramePlan {
    SurfaceFramePlan {
        modal_placement: None,
        history_surface: HistorySurfaceMode::NativeScrollback,
        attention_requested: false,
    }
}

fn viewport_history_plan() -> SurfaceFramePlan {
    SurfaceFramePlan {
        history_surface: HistorySurfaceMode::Viewport,
        ..native_plan()
    }
}

fn question_state(items: Vec<crate::state::QuestionItem>) -> AppState {
    let mut state = AppState::new();
    state
        .ui
        .push_prompt(crate::state::PanePromptState::Question(
            crate::state::QuestionPromptState {
                request_id: "q".into(),
                original_input: serde_json::json!({}),
                questions: items,
                current_question: crate::state::QuestionPage::Question(0),
                focus_target: crate::state::QuestionFocusTarget::QuestionOption(0),
                is_in_plan_mode: false,
            },
        ));
    state
}

fn question_item(header: &str, question: &str, option_count: usize) -> crate::state::QuestionItem {
    crate::state::QuestionItem {
        header: header.into(),
        question: question.into(),
        options: (0..option_count)
            .map(|idx| crate::state::QuestionOption {
                label: format!("Option {}", idx + 1),
                description: "description".into(),
                preview: None,
            })
            .collect(),
        multi_select: false,
        selected: None,
        checked: Vec::new(),
        other_input: crate::state::OtherInputState::default(),
    }
}

fn state_with_popup_items(count: usize) -> AppState {
    let mut state = AppState::new();
    let items = (0..count)
        .map(|idx| crate::widgets::suggestion_popup::SuggestionItem {
            label: format!("src/{idx}.rs"),
            description: None,
            metadata: Some(crate::widgets::suggestion_popup::SuggestionMeta::Path {
                is_directory: false,
            }),
        })
        .collect::<Vec<_>>();
    state.ui.completion.set_active(
        crate::state::ActiveSuggestions {
            kind: crate::state::SuggestionKind::At,
            items,
            selected: 0,
            query: "s".into(),
            trigger_pos: 0,
        },
        0..2,
        "@s".into(),
    );
    state.ui.sync_popup_from_active_suggestions();
    state
}

fn plain_buffer_lines(buffer: &Buffer) -> Vec<String> {
    buffer
        .content
        .chunks(buffer.area.width as usize)
        .map(|cells| cells.iter().map(ratatui::buffer::Cell::symbol).collect())
        .collect()
}

#[test]
fn exit_plan_pending_live_tail_has_no_busy_spinner() {
    let mut state = exit_plan_prompt_state(3);
    // ToolUseQueued fires start_tool("ExitPlanMode"); the overlay-driven skip
    // keeps it out of the ledger, so no `⠋ Processing…` busy spinner appears
    // above the plan (the prior leak: active_transcript_cell saw it Queued).
    state.session.start_tool(
        "call-1".into(),
        "ExitPlanMode".into(),
        &serde_json::Value::Null,
    );
    let styles = UiStyles::new(&state.ui.theme);
    let lines = build_live_tail_lines(&state, styles, 96, native_plan());
    let text = lines
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!text.to_lowercase().contains("processing"), "{text}");
    assert!(text.contains("Ready to code?"), "{text}");
}

fn lines_of(text: &str) -> Vec<Line<'static>> {
    text.lines().map(|l| Line::from(l.to_string())).collect()
}

#[test]
fn split_body_and_actions_pins_trailing_block_after_last_blank() {
    // command body, blank separator, then two action rows.
    let lines = lines_of("$ run\nmore detail\n\nApprove\nDeny");
    let (body, actions) = split_body_and_actions(lines);
    // Body keeps the blank separator as its trailing row (layout parity).
    let body_text: Vec<String> = body
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect();
    let action_text: Vec<String> = actions
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect();
    assert_eq!(body_text, vec!["$ run", "more detail", ""]);
    assert_eq!(action_text, vec!["Approve", "Deny"]);
}

#[test]
fn split_body_and_actions_no_blank_is_all_body() {
    let lines = lines_of("$ run\nmore detail");
    let (body, actions) = split_body_and_actions(lines);
    assert_eq!(body.len(), 2);
    assert!(actions.is_empty());
}
