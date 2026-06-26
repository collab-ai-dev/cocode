use super::*;

/// Build an `InputState` by typing `s` char-by-char (mirrors key handling).
fn typed(s: &str) -> InputState {
    let mut input = InputState::new();
    for c in s.chars() {
        input.insert(c);
    }
    input
}

#[test]
fn test_insert_cjk_advances_cursor_by_byte_width() {
    let input = typed("你好");
    assert_eq!(input.text, "你好");
    // 2 chars × 3 bytes; cursor lands on a char boundary at the end.
    assert_eq!(input.cursor, 6);
}

#[test]
fn test_backspace_removes_whole_cjk_char() {
    let mut input = typed("你好");
    input.backspace();
    assert_eq!(input.text, "你");
    assert_eq!(input.cursor, 3);
    input.backspace();
    assert_eq!(input.text, "");
    assert_eq!(input.cursor, 0);
    input.backspace(); // no-op at start, must not panic
    assert_eq!(input.cursor, 0);
}

#[test]
fn test_move_left_right_step_over_cjk() {
    let mut input = typed("a你b"); // bytes: a=1, 你=3, b=1 → len 5
    assert_eq!(input.cursor, 5);
    input.move_left();
    assert_eq!(input.cursor, 4); // before b
    input.move_left();
    assert_eq!(input.cursor, 1); // before 你
    input.move_left();
    assert_eq!(input.cursor, 0); // before a
    input.move_left();
    assert_eq!(input.cursor, 0); // no-op
    input.move_right();
    assert_eq!(input.cursor, 1); // after a
    input.move_right();
    assert_eq!(input.cursor, 4); // after 你
}

#[test]
fn test_insert_mid_string_at_cjk_boundary() {
    let mut input = typed("你好");
    input.home();
    input.move_right(); // after 你 → byte 3
    input.insert('x'); // 你x好
    assert_eq!(input.text, "你x好");
    assert_eq!(input.cursor, 4);
}

#[test]
fn test_kill_line_before_and_after_on_cjk_boundary() {
    let mut after = typed("你好世界");
    after.home();
    after.move_right();
    after.move_right(); // after 你好 → byte 6
    after.kill_line_after();
    assert_eq!(after.text, "你好");

    let mut before = typed("你好世界");
    before.home();
    before.move_right(); // after 你 → byte 3
    before.kill_line_before();
    assert_eq!(before.text, "好世界");
    assert_eq!(before.cursor, 0);
}

#[test]
fn test_move_word_over_cjk_does_not_panic() {
    // bytes: 你好=6, space=1, 世界=6 → len 13
    let mut input = typed("你好 世界");
    assert_eq!(input.cursor, 13);
    input.move_word_left();
    assert_eq!(input.cursor, 7); // start of 世界
    input.move_word_left();
    assert_eq!(input.cursor, 0); // start of 你好
    input.move_word_right();
    assert_eq!(input.cursor, 6); // end of 你好
    input.move_word_right();
    assert_eq!(input.cursor, 13); // end of 世界
}
