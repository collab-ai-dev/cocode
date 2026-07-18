use super::*;
use crate::state::InputState;
use coco_tui_ui::theme::Theme;

fn input(text: &str) -> InputState {
    let mut input = InputState::new();
    input.set_text(text);
    input
}

// ─── Composer soft wrap (plan item C2) ────────────────────────────────────

/// Render the composer into a `width × height` slot and return its rows.
fn render_composer(state: &InputState, width: u16, height: u16) -> Vec<String> {
    use ratatui::buffer::Buffer;
    use ratatui::widgets::Widget;

    let theme = Theme::default();
    let area = Rect::new(0, 0, width, height);
    let mut buffer = Buffer::empty(area);
    InputWidget::new(state, UiStyles::new(&theme))
        .focused(true)
        .render(area, &mut buffer);
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

#[test]
fn a_long_single_line_prompt_wraps_instead_of_being_clipped() {
    // The bug this fixes: the composer rendered an unwrapped `Paragraph`, so a
    // long prompt with no newline ran past the right edge and the tail was
    // simply invisible — the user could not see what they had typed or pasted.
    let text = "the quick brown fox jumps over the lazy dog and keeps running";
    let state = input(text);
    let rows = render_composer(&state, 30, 6);
    let painted: String = rows.join(" ");

    for word in ["quick", "jumps", "keeps", "running"] {
        assert!(
            painted.contains(word),
            "{word:?} must be visible somewhere in the composer, got {rows:?}"
        );
    }
}

#[test]
fn wrapped_composer_rows_break_at_words_and_align_under_the_gutter() {
    let state = input("the quick brown fox jumps");
    // Row 0 is the block's top border; content starts at row 1.
    let rows = render_composer(&state, 16, 6);

    assert!(
        rows[1].starts_with('❯'),
        "the first content row wears the indicator: {rows:?}"
    );
    // Continuation rows indent under the indicator instead of re-wearing it.
    assert!(
        rows[2].starts_with("  ") && !rows[2].contains('❯'),
        "continuation rows align under the gutter: {rows:?}"
    );
    // Word-boundary wrapping: no row ends mid-word, and nothing is lost.
    let content: Vec<String> = rows[1..]
        .iter()
        .take_while(|row| !row.is_empty())
        .map(|row| row.trim_start_matches('❯').trim().to_string())
        .collect();
    assert_eq!(
        content.join(" "),
        "the quick brown fox jumps",
        "the wrapped rows must reconstruct the prompt: {rows:?}"
    );
}

#[test]
fn a_short_prompt_still_uses_the_single_row_path() {
    // The rich single-row path carries the inline ghost / hint affordances, so
    // short input must keep taking it.
    let state = input("hi");
    let rows = render_composer(&state, 40, 3);
    assert!(rows[1].contains("hi"), "{rows:?}");
    assert!(
        rows[2].is_empty() || !rows[2].contains("hi"),
        "short input must not spill onto a second row: {rows:?}"
    );
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

    let model = InputRenderModel::build(&input, false, None, false, None, /*width*/ 80);

    assert_eq!(model.prompt_mode, PromptMode::Bash);
    assert_eq!(model.prefix_consumed, 2);
    assert_eq!(model.display_text, "cargo test");
    assert_eq!(model.title, " Bash Mode ");
    assert!(!model.is_placeholder);
}

#[test]
fn input_render_model_empty_default_has_no_placeholder_text() {
    let input = InputState::new();

    let model = InputRenderModel::build(&input, false, None, false, None, /*width*/ 80);

    assert_eq!(model.display_text, "");
    assert!(!model.is_placeholder);
}

#[test]
fn input_render_model_queued_placeholder_wins_over_suggestion() {
    // Empty composer + editable queue → the "press up to edit" hint, even when
    // a prompt suggestion is also present.
    let input = InputState::new();

    let model = InputRenderModel::build(
        &input,
        false,
        Some("Try this prompt"),
        true,
        None,
        /*width*/ 80,
    );

    assert_eq!(model.display_text, "Press up to edit queued messages");
    assert!(model.is_placeholder);
}

#[test]
fn input_render_model_command_palette_filter_wins_over_placeholder() {
    let input = InputState::new();

    let model = InputRenderModel::build(
        &input,
        false,
        Some("ignored"),
        false,
        Some("config"),
        /*width*/ 80,
    );

    assert_eq!(model.display_text, "/config");
    assert_eq!(model.command_palette_filter.as_deref(), Some("config"));
    assert!(!model.is_placeholder);
}

#[test]
fn input_render_model_streaming_forces_normal_prompt_and_no_title() {
    let input = input("! cargo test");

    let model = InputRenderModel::build(&input, true, None, false, None, /*width*/ 80);

    assert_eq!(model.prompt_mode, PromptMode::Normal);
    assert_eq!(model.prefix_consumed, 0);
    assert_eq!(model.display_text, "! cargo test");
    // Streaming no longer labels the box with a "Queue Input" title — the
    // input stays clean; queued items surface via the footer strip.
    assert_eq!(model.title, "");
}

#[test]
fn input_render_model_appends_inline_hint_after_text() {
    let mut input = input("/add-dir ");
    input.set_inline_hint(" <path>");

    let model = InputRenderModel::build(&input, false, None, false, None, /*width*/ 80);

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

    let model = InputRenderModel::build(&input, false, None, false, None, /*width*/ 80);

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

    let model = InputRenderModel::build(&input, false, None, false, None, /*width*/ 80);

    assert!(model.inline_ghost.is_none());
}

#[test]
fn input_render_model_multiline_tracks_cursor_row_and_col() {
    let mut input = input("ab\ncde");
    input.textarea.set_cursor(input.text().len());

    let model = InputRenderModel::build(&input, false, None, false, None, /*width*/ 80);

    assert_eq!(model.display_text, "ab\ncde");
    assert_eq!(model.cursor_row, 1, "cursor is on the second line");
    assert_eq!(model.cursor_col, 3, "cursor is after 'cde' (3 cols)");
}

#[test]
fn input_render_model_multiline_cursor_on_first_line() {
    let mut input = input("ab\ncde");
    input.textarea.set_cursor(1); // after 'a'

    let model = InputRenderModel::build(&input, false, None, false, None, /*width*/ 80);

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
