use super::*;
use crate::state::InputState;
use coco_tui_ui::theme::Theme;

fn input(text: &str) -> InputState {
    let mut input = InputState::new();
    input.set_text(text);
    input
}

#[test]
fn btw_trigger_highlight_matches_start_only_word_boundary() {
    assert_eq!(btw_trigger_len("/btw"), Some(4));
    assert_eq!(btw_trigger_len("/BTW why"), Some(4));
    assert_eq!(btw_trigger_len("/btw\twhy"), Some(4));
    assert_eq!(btw_trigger_len("/btwice"), None);
    assert_eq!(btw_trigger_len("ask /btw why"), None);
    assert_eq!(btw_trigger_len("/bt"), None);
}

#[test]
fn styled_display_text_spans_warns_for_btw_trigger_only() {
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let text_style = Style::default().fg(styles.text());

    let spans = styled_display_text_spans("/btw explain".to_string(), text_style, styles);

    assert_eq!(spans.len(), 2);
    assert_eq!(spans[0].content.as_ref(), "/btw");
    assert_eq!(spans[0].style.fg, Some(styles.warning()));
    assert_eq!(spans[1].content.as_ref(), " explain");
    assert_eq!(spans[1].style.fg, Some(styles.text()));
}

#[test]
fn styled_display_text_spans_leaves_non_btw_text_plain() {
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let text_style = Style::default().fg(styles.text());

    let spans = styled_display_text_spans("/btwice".to_string(), text_style, styles);

    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].content.as_ref(), "/btwice");
    assert_eq!(spans[0].style.fg, Some(styles.text()));
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
