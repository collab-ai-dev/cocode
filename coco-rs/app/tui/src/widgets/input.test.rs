use super::*;
use crate::state::InputState;

fn input(text: &str) -> InputState {
    let mut input = InputState::new();
    input.set_text(text);
    input
}

#[test]
fn input_render_model_strips_bash_prefix_for_display() {
    let input = input("! cargo test");

    let model = InputRenderModel::build(&input, false, None, false, None);

    assert_eq!(model.prompt_mode, PromptMode::Bash);
    assert_eq!(model.prefix_consumed, 2);
    assert_eq!(model.display_text, "cargo test");
    assert_eq!(model.title, " Bash Mode ");
    assert!(!model.is_placeholder);
}

#[test]
fn input_render_model_empty_default_has_no_placeholder_text() {
    let input = InputState::new();

    let model = InputRenderModel::build(&input, false, None, false, None);

    assert_eq!(model.display_text, "");
    assert!(!model.is_placeholder);
}

#[test]
fn input_render_model_queued_placeholder_wins_over_suggestion() {
    // Empty composer + editable queue → the "press up to edit" hint, even when
    // a prompt suggestion is also present (mirrors TS `usePromptInputPlaceholder`).
    let input = InputState::new();

    let model = InputRenderModel::build(&input, false, Some("Try this prompt"), true, None);

    assert_eq!(model.display_text, "Press up to edit queued messages");
    assert!(model.is_placeholder);
}

#[test]
fn input_render_model_command_palette_filter_wins_over_placeholder() {
    let input = InputState::new();

    let model = InputRenderModel::build(&input, false, Some("ignored"), false, Some("config"));

    assert_eq!(model.display_text, "/config");
    assert_eq!(model.command_palette_filter.as_deref(), Some("config"));
    assert!(!model.is_placeholder);
}

#[test]
fn input_render_model_streaming_forces_normal_prompt_and_no_title() {
    let input = input("! cargo test");

    let model = InputRenderModel::build(&input, true, None, false, None);

    assert_eq!(model.prompt_mode, PromptMode::Normal);
    assert_eq!(model.prefix_consumed, 0);
    assert_eq!(model.display_text, "! cargo test");
    // Streaming no longer labels the box with a "Queue Input" title — the
    // input stays clean (TS parity); queued items surface via the footer strip.
    assert_eq!(model.title, "");
}

#[test]
fn input_render_model_appends_inline_hint_after_text() {
    let mut input = input("/add-dir ");
    input.set_inline_hint(" <path>");

    let model = InputRenderModel::build(&input, false, None, false, None);

    assert_eq!(model.display_text, "/add-dir ");
    assert_eq!(model.inline_hint.as_deref(), Some(" <path>"));
    assert!(!model.is_placeholder);
}

#[test]
fn input_render_model_places_inline_ghost_at_cursor() {
    let mut input = input("abc xyz");
    input.textarea.set_cursor(3);
    input.set_inline_ghost(crate::state::InlineGhost {
        text: "def".into(),
        insert_position: 3,
        replace_start: 3,
        replace_end: 3,
        replacement: "def".into(),
        cursor_after_accept: 6,
    });

    let model = InputRenderModel::build(&input, false, None, false, None);

    assert_eq!(model.display_text, "abc xyz");
    let ghost = model.inline_ghost.expect("rendered ghost");
    assert_eq!(ghost.byte_pos, 3);
    assert_eq!(ghost.text, "def");
}

#[test]
fn input_render_model_hides_stale_inline_ghost() {
    let mut input = input("abc");
    input.textarea.set_cursor(2);
    input.set_inline_ghost(crate::state::InlineGhost {
        text: "d".into(),
        insert_position: 3,
        replace_start: 3,
        replace_end: 3,
        replacement: "d".into(),
        cursor_after_accept: 4,
    });

    let model = InputRenderModel::build(&input, false, None, false, None);

    assert!(model.inline_ghost.is_none());
}

#[test]
fn input_render_model_multiline_tracks_cursor_row_and_col() {
    let mut input = input("ab\ncde");
    input.textarea.set_cursor(input.text().len());

    let model = InputRenderModel::build(&input, false, None, false, None);

    assert_eq!(model.display_text, "ab\ncde");
    assert_eq!(model.cursor_row, 1, "cursor is on the second line");
    assert_eq!(model.cursor_col, 3, "cursor is after 'cde' (3 cols)");
}

#[test]
fn input_render_model_multiline_cursor_on_first_line() {
    let mut input = input("ab\ncde");
    input.textarea.set_cursor(1); // after 'a'

    let model = InputRenderModel::build(&input, false, None, false, None);

    assert_eq!(model.cursor_row, 0);
    assert_eq!(model.cursor_col, 1);
}

#[test]
fn scroll_offset_keeps_cursor_within_window() {
    // Everything fits → no scroll.
    assert_eq!(super::scroll_offset(0, 3, 5), 0);
    assert_eq!(super::scroll_offset(2, 3, 5), 0);
    // Cursor past the window → scroll so the cursor is the last visible row.
    assert_eq!(super::scroll_offset(4, 10, 3), 2);
    // Clamp to the max scroll (can't scroll past the last full window).
    assert_eq!(super::scroll_offset(9, 10, 3), 7);
}
