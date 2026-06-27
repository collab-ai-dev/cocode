//! Tests for the [`TextArea`] widget. Covers CJK / wide-char rendering,
//! grapheme-aware delete, kill ring, multi-line wrap, and word movement.

use ratatui::layout::Rect;

use super::*;

fn ta_with(text: &str, cursor: usize) -> TextArea {
    let mut ta = TextArea::new();
    ta.set_text(text);
    ta.set_cursor(cursor);
    ta
}

// ─────────────────────── Construction + access ──────────────────────

#[test]
fn empty_textarea_has_zero_cursor() {
    let ta = TextArea::new();
    assert!(ta.is_empty());
    assert_eq!(ta.cursor(), 0);
}

#[test]
fn set_text_clamps_cursor_into_range() {
    let mut ta = ta_with("hello world", 11);
    ta.set_text("hi");
    // Cursor must end at a valid char boundary inside the new text.
    assert!(ta.cursor() <= ta.text().len());
}

#[test]
fn take_text_returns_previous_buffer_and_clears() {
    let mut ta = ta_with("draft", 5);
    let taken = ta.take_text();
    assert_eq!(taken, "draft");
    assert!(ta.is_empty());
    assert_eq!(ta.cursor(), 0);
}

#[test]
fn cursor_lands_at_byte_boundary_in_cjk() {
    // "你好" is 2 chars, 6 bytes (3 each). Cursor at byte 6 is past "好".
    let ta = ta_with("你好", 6);
    assert_eq!(ta.cursor(), 6);
    // Setting to a non-boundary byte snaps to nearest boundary.
    let mut ta = TextArea::new();
    ta.set_text("你好世界");
    ta.set_cursor(7); // mid-grapheme
    assert!(ta.text().is_char_boundary(ta.cursor()));
}

// ─────────────────────────── Insertion ──────────────────────────────

#[test]
fn insert_str_advances_cursor_by_byte_len() {
    let mut ta = ta_with("hello", 5);
    ta.insert_str(" world");
    assert_eq!(ta.text(), "hello world");
    assert_eq!(ta.cursor(), 11);
}

#[test]
fn insert_str_at_does_not_move_cursor_when_inserting_after_cursor() {
    let mut ta = ta_with("hello world", 5);
    ta.insert_str_at(11, "!");
    assert_eq!(ta.text(), "hello world!");
    assert_eq!(ta.cursor(), 5);
}

#[test]
fn replace_range_moves_cursor_when_inside_range() {
    let mut ta = ta_with("hello world", 7);
    ta.replace_range(6..11, "rust");
    assert_eq!(ta.text(), "hello rust");
    // Cursor was at byte 7 (inside "world") → moves to end of replacement.
    assert_eq!(ta.cursor(), 6 + "rust".len());
}

// ─────────────────────────── Deletion ───────────────────────────────

#[test]
fn delete_backward_removes_one_ascii_char() {
    let mut ta = ta_with("abc", 3);
    ta.delete_backward(1);
    assert_eq!(ta.text(), "ab");
    assert_eq!(ta.cursor(), 2);
}

#[test]
fn delete_backward_removes_one_cjk_grapheme() {
    // Each CJK char is 3 bytes; backspace must remove the whole grapheme,
    // not a single byte (would yield invalid UTF-8).
    let mut ta = ta_with("你好", 6);
    ta.delete_backward(1);
    assert_eq!(ta.text(), "你");
    assert_eq!(ta.cursor(), 3);
}

#[test]
fn delete_forward_removes_one_grapheme() {
    let mut ta = ta_with("你好世界", 0);
    ta.delete_forward(1);
    assert_eq!(ta.text(), "好世界");
    assert_eq!(ta.cursor(), 0);
}

#[test]
fn delete_backward_word_strips_to_word_boundary() {
    let mut ta = ta_with("hello world", 11);
    ta.delete_backward_word();
    assert_eq!(ta.text(), "hello ");
}

#[test]
fn delete_forward_word_strips_to_next_word_boundary() {
    let mut ta = ta_with("hello world foo", 0);
    ta.delete_forward_word();
    // `end_of_next_word` skips leading whitespace then consumes the run,
    // so deletion includes "hello" and the trailing space is left intact.
    assert!(ta.text().starts_with(" world") || ta.text().starts_with("world"));
}

// ──────────────────────────── Kill ring ─────────────────────────────

#[test]
fn kill_to_end_then_yank_round_trips() {
    let mut ta = ta_with("hello world", 6);
    ta.kill_to_end_of_line();
    assert_eq!(ta.text(), "hello ");
    ta.yank();
    assert_eq!(ta.text(), "hello world");
}

#[test]
fn set_text_preserves_kill_buffer() {
    // Whole-buffer replacement intentionally keeps the kill buffer alive
    // (matches codex-rs semantics — Ctrl+Y still recovers after submit).
    let mut ta = ta_with("draft", 5);
    ta.kill_to_beginning_of_line();
    assert!(ta.text().is_empty());
    ta.set_text("");
    ta.yank();
    assert_eq!(ta.text(), "draft");
}

#[test]
fn kill_at_eol_with_trailing_newline_kills_newline() {
    let mut ta = ta_with("a\nb", 1);
    ta.kill_to_end_of_line();
    assert_eq!(ta.text(), "ab");
}

// ─────────────────────────── Movement ───────────────────────────────

#[test]
fn move_cursor_left_steps_one_grapheme() {
    let mut ta = ta_with("你好", 6);
    ta.move_cursor_left();
    assert_eq!(ta.cursor(), 3);
    ta.move_cursor_left();
    assert_eq!(ta.cursor(), 0);
    ta.move_cursor_left(); // clamped at 0
    assert_eq!(ta.cursor(), 0);
}

#[test]
fn move_cursor_right_steps_one_grapheme() {
    let mut ta = ta_with("你好", 0);
    ta.move_cursor_right();
    assert_eq!(ta.cursor(), 3);
    ta.move_cursor_right();
    assert_eq!(ta.cursor(), 6);
    ta.move_cursor_right(); // clamped at len
    assert_eq!(ta.cursor(), 6);
}

#[test]
fn move_cursor_to_beginning_of_line_jumps_home() {
    let mut ta = ta_with("hello", 3);
    ta.move_cursor_to_beginning_of_line(BolBehavior::StayPut);
    assert_eq!(ta.cursor(), 0);
}

#[test]
fn move_cursor_to_end_of_line_jumps_end() {
    let mut ta = ta_with("hello", 0);
    ta.move_cursor_to_end_of_line(EolBehavior::StayPut);
    assert_eq!(ta.cursor(), 5);
}

// ──────────────────────── Word boundaries ───────────────────────────

#[test]
fn beginning_of_previous_word() {
    let ta = ta_with("hello world", 11);
    assert_eq!(ta.beginning_of_previous_word(), 6); // start of "world"
}

#[test]
fn end_of_next_word() {
    let ta = ta_with("hello world", 0);
    assert_eq!(ta.end_of_next_word(), 5); // end of "hello"
}

// ───────────────────────── Rendering ────────────────────────────────

#[test]
fn cursor_pos_cjk_returns_display_column_not_char_index() {
    // "你好" is 2 chars but 4 display columns. Cursor at end → col 4.
    let ta = ta_with("你好", 6);
    let area = Rect::new(0, 0, 80, 1);
    let (col, row) = ta.cursor_pos(area).expect("cursor pos");
    assert_eq!(col, 4, "cursor at end of 你好 must be column 4");
    assert_eq!(row, 0);
}

#[test]
fn cursor_pos_ascii_returns_byte_offset() {
    let ta = ta_with("hello", 3);
    let area = Rect::new(0, 0, 80, 1);
    let (col, _) = ta.cursor_pos(area).expect("cursor pos");
    assert_eq!(col, 3);
}

#[test]
fn cursor_pos_empty_buffer_returns_origin() {
    let ta = TextArea::new();
    let area = Rect::new(2, 5, 80, 1);
    let (col, row) = ta.cursor_pos(area).expect("origin");
    assert_eq!((col, row), (area.x, area.y));
}

#[test]
fn wrapped_lines_split_on_newline() {
    let ta = ta_with("ab\ncd", 0);
    let lines = ta.wrapped_lines(80);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0], 0..2);
    assert_eq!(lines[1], 3..5);
}

#[test]
fn wrapped_lines_wrap_at_display_width() {
    // 6 ASCII chars at width=3 → 2 wrapped lines.
    let ta = ta_with("abcdef", 0);
    let lines = ta.wrapped_lines(3);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0], 0..3);
    assert_eq!(lines[1], 3..6);
}

#[test]
fn wrapped_lines_cjk_wraps_by_display_width() {
    // Each CJK char is 2 columns. width=4 fits exactly 2 CJK per line.
    let ta = ta_with("你好世界", 0);
    let lines = ta.wrapped_lines(4);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0], 0..6); // 你好 = 6 bytes
    assert_eq!(lines[1], 6..12); // 世界 = 6 bytes
}
