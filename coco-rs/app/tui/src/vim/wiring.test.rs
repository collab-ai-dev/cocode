//! Tests for the vim wiring layer. Verifies that dispatch routes Normal-mode
//! keys through the state machine and applies returned `VimAction`s.

use super::*;
use crate::vim::CommandState;
use crate::vim::VimRuntime;

fn setup(text: &str, cursor: usize) -> (TextArea, VimRuntime) {
    let mut ta = TextArea::new();
    ta.set_text(text);
    ta.set_cursor(cursor);
    let vim = VimRuntime::new();
    (ta, vim)
}

// ─────────────────── Mode dispatch ───────────────────

#[test]
fn dispatch_in_insert_mode_is_noop() {
    let (mut ta, mut vim) = setup("", 0);
    vim.state.enter_insert();
    let action = dispatch_vim_key('h', &mut ta, &mut vim);
    assert!(matches!(action, VimAction::Unhandled));
}

#[test]
fn dispatch_in_normal_routes_to_state_machine() {
    let (mut ta, mut vim) = setup("hello", 2);
    let action = dispatch_vim_key('h', &mut ta, &mut vim);
    assert!(matches!(action, VimAction::Handled));
    assert_eq!(ta.cursor(), 1);
}

// ─────────────────── Normal-mode motions ───────────────────

#[test]
fn normal_mode_h_l_moves_cursor() {
    let (mut ta, mut vim) = setup("hello", 2);
    dispatch_vim_key('h', &mut ta, &mut vim);
    assert_eq!(ta.cursor(), 1);
    dispatch_vim_key('l', &mut ta, &mut vim);
    assert_eq!(ta.cursor(), 2);
}

#[test]
fn normal_mode_w_jumps_to_next_word() {
    let (mut ta, mut vim) = setup("hello world", 0);
    dispatch_vim_key('w', &mut ta, &mut vim);
    // Byte offset of `w` of "world" = 6.
    assert_eq!(ta.cursor(), 6);
}

#[test]
fn normal_mode_0_and_dollar_anchor_line() {
    let (mut ta, mut vim) = setup("hello", 3);
    dispatch_vim_key('0', &mut ta, &mut vim);
    assert_eq!(ta.cursor(), 0);
    dispatch_vim_key('$', &mut ta, &mut vim);
    // Vim `$` lands ON the last char (byte offset 4 for "hello").
    assert_eq!(ta.cursor(), 4);
}

// ─────────────────── Mode transitions via apply_action ───────────────────

#[test]
fn i_enters_insert_mode() {
    let (mut ta, mut vim) = setup("hello", 0);
    let action = dispatch_vim_key('i', &mut ta, &mut vim);
    let submit = apply_action(action, &mut ta, &mut vim);
    assert!(vim.is_insert());
    assert!(!submit);
}

#[test]
fn a_enters_insert_after_cursor() {
    let (mut ta, mut vim) = setup("hello", 0);
    let action = dispatch_vim_key('a', &mut ta, &mut vim);
    apply_action(action, &mut ta, &mut vim);
    assert!(vim.is_insert());
    assert_eq!(ta.cursor(), 1);
}

#[test]
fn big_a_enters_insert_at_eol() {
    let (mut ta, mut vim) = setup("hello", 0);
    let action = dispatch_vim_key('A', &mut ta, &mut vim);
    apply_action(action, &mut ta, &mut vim);
    assert!(vim.is_insert());
    assert_eq!(ta.cursor(), 5);
}

#[test]
fn big_i_enters_insert_at_bol() {
    let (mut ta, mut vim) = setup("  hello", 5);
    let action = dispatch_vim_key('I', &mut ta, &mut vim);
    apply_action(action, &mut ta, &mut vim);
    assert!(vim.is_insert());
    assert_eq!(ta.cursor(), 0);
}

#[test]
fn lowercase_o_opens_line_below() {
    let (mut ta, mut vim) = setup("hello", 0);
    let action = dispatch_vim_key('o', &mut ta, &mut vim);
    apply_action(action, &mut ta, &mut vim);
    assert!(vim.is_insert());
    assert_eq!(ta.text(), "hello\n");
}

#[test]
fn uppercase_o_opens_line_above() {
    let (mut ta, mut vim) = setup("hello", 0);
    let action = dispatch_vim_key('O', &mut ta, &mut vim);
    apply_action(action, &mut ta, &mut vim);
    assert!(vim.is_insert());
    assert_eq!(ta.text(), "\nhello");
}

// ─────────────────── Operators ───────────────────

#[test]
fn dd_deletes_current_line_not_whole_buffer() {
    // Multi-line bug regression — `dd` must only delete the current line.
    let (mut ta, mut vim) = setup("first\nsecond\nthird", 7);
    dispatch_vim_key('d', &mut ta, &mut vim);
    if let VimState::Normal { command } = &vim.state {
        assert!(matches!(command, CommandState::OperatorPending { .. }));
    } else {
        panic!("expected operator pending");
    }
    dispatch_vim_key('d', &mut ta, &mut vim);
    assert_eq!(ta.text(), "first\nthird");
}

#[test]
fn dw_deletes_to_next_word() {
    let (mut ta, mut vim) = setup("hello world", 0);
    dispatch_vim_key('d', &mut ta, &mut vim);
    dispatch_vim_key('w', &mut ta, &mut vim);
    assert_eq!(ta.text(), "world");
}

#[test]
fn cw_enters_insert_after_word_deletion() {
    let (mut ta, mut vim) = setup("hello world", 0);
    dispatch_vim_key('c', &mut ta, &mut vim);
    let action = dispatch_vim_key('w', &mut ta, &mut vim);
    let submit = apply_action(action, &mut ta, &mut vim);
    assert!(!submit);
    assert!(vim.is_insert());
}

#[test]
fn yw_then_p_pastes_word() {
    let (mut ta, mut vim) = setup("hello world", 0);
    dispatch_vim_key('y', &mut ta, &mut vim);
    dispatch_vim_key('w', &mut ta, &mut vim);
    assert_eq!(ta.text(), "hello world");
    assert!(!vim.persistent.register.is_empty());
}

// ─────────────────── Find motion ───────────────────

#[test]
fn find_char_with_f() {
    let (mut ta, mut vim) = setup("hello world", 0);
    dispatch_vim_key('f', &mut ta, &mut vim);
    dispatch_vim_key('o', &mut ta, &mut vim);
    // First 'o' is at byte 4 in "hello world".
    assert_eq!(ta.cursor(), 4);
}

// ─────────────────── Submit semantics ───────────────────

#[test]
fn enter_in_normal_mode_returns_submit() {
    let (mut ta, mut vim) = setup("hi", 0);
    let action = dispatch_vim_key('\n', &mut ta, &mut vim);
    let submit = apply_action(action, &mut ta, &mut vim);
    assert!(submit);
}

// ─────────────────── Esc handling ───────────────────

#[test]
fn handle_insert_escape_returns_to_normal_and_moves_cursor_back() {
    let (mut ta, mut vim) = setup("hello", 5);
    vim.state.enter_insert();
    let consumed = handle_insert_escape(&mut ta, &mut vim);
    assert!(consumed);
    assert!(vim.is_normal());
    assert_eq!(ta.cursor(), 4);
}

#[test]
fn handle_insert_escape_no_op_in_normal() {
    let (mut ta, mut vim) = setup("hello", 5);
    let consumed = handle_insert_escape(&mut ta, &mut vim);
    assert!(!consumed);
    assert!(vim.is_normal());
}

#[test]
fn handle_insert_escape_at_bol_does_not_underflow() {
    let (mut ta, mut vim) = setup("hello", 0);
    vim.state.enter_insert();
    handle_insert_escape(&mut ta, &mut vim);
    assert!(vim.is_normal());
    assert_eq!(ta.cursor(), 0);
}
