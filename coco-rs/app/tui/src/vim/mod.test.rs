//! Tests for vim mode state machine. All positions are UTF-8 byte
//! offsets — for the ASCII fixtures here they coincide with char indices.

use crate::vim::CommandState;
use crate::vim::Operator;
use crate::vim::PersistentState;
use crate::vim::TextObjScope;
use crate::vim::VimState;
use crate::vim::motions;
use crate::vim::operators;
use crate::vim::text_objects::{self};
use crate::vim::transitions::VimAction;
use crate::vim::transitions::{self};
use coco_tui_ui::widgets::TextArea;

fn textarea_with(text: &str, cursor: usize) -> TextArea {
    let mut ta = TextArea::new();
    ta.set_text(text);
    ta.set_cursor(cursor);
    ta
}

#[test]
fn test_word_forward() {
    assert_eq!(motions::word_forward("hello world", 0), 6);
    assert_eq!(motions::word_forward("hello world", 6), 11);
}

#[test]
fn test_word_backward() {
    assert_eq!(motions::word_backward("hello world", 11), 6);
    assert_eq!(motions::word_backward("hello world", 6), 0);
}

#[test]
fn test_word_end() {
    assert_eq!(motions::word_end("hello world", 0), 4);
    assert_eq!(motions::word_end("hello world", 5), 10);
}

#[test]
fn test_find_char() {
    assert_eq!(motions::find_char_forward("hello world", 0, 'o'), Some(4));
    assert_eq!(motions::find_char_forward("hello world", 5, 'o'), Some(7));
    assert_eq!(motions::find_char_backward("hello world", 7, 'l'), Some(3));
}

#[test]
fn test_text_object_word_inner() {
    // `iw` on "hello world" at byte 2 (inside "hello") returns the word's
    // byte range. End is byte AFTER the last char of "hello".
    let result = text_objects::word("hello world", 2, TextObjScope::Inner);
    assert_eq!(result, Some(0..5));
}

#[test]
fn test_text_object_word_around() {
    // `aw` includes the trailing whitespace run after the word.
    let result = text_objects::word("hello world", 2, TextObjScope::Around);
    assert_eq!(result, Some(0..6));
}

#[test]
fn test_text_object_quoted() {
    let text = r#"say "hello" world"#;
    let result = text_objects::quoted(text, 6, '"', TextObjScope::Inner);
    assert_eq!(result, Some(5..10));

    let result = text_objects::quoted(text, 6, '"', TextObjScope::Around);
    assert_eq!(result, Some(4..11));
}

#[test]
fn test_text_object_bracket() {
    let text = "fn(a, b)";
    let result = text_objects::bracket(text, 4, '(', ')', TextObjScope::Inner);
    assert_eq!(result, Some(3..7));

    let result = text_objects::bracket(text, 4, '(', ')', TextObjScope::Around);
    assert_eq!(result, Some(2..8));
}

#[test]
fn test_operator_delete_word() {
    let mut ta = textarea_with("hello world", 0);
    let mut persistent = PersistentState::default();
    // dw deletes [0..6) which is "hello ".
    let deleted = operators::apply_operator(&mut ta, Operator::Delete, 0..6, &mut persistent);
    assert_eq!(deleted, "hello ");
    assert_eq!(ta.text(), "world");
    assert_eq!(persistent.register, "hello ");
}

#[test]
fn test_operator_yank() {
    let mut ta = textarea_with("hello world", 0);
    let mut persistent = PersistentState::default();
    operators::apply_operator(&mut ta, Operator::Yank, 0..5, &mut persistent);
    assert_eq!(ta.text(), "hello world");
    assert_eq!(persistent.register, "hello");
}

#[test]
fn test_put_after_characterwise() {
    let mut ta = textarea_with("hello", 4);
    let persistent = PersistentState {
        register: " world".to_string(),
        register_is_linewise: false,
        ..Default::default()
    };
    operators::put_after(&mut ta, &persistent);
    assert_eq!(ta.text(), "hello world");
}

#[test]
fn test_replace_char() {
    let mut ta = textarea_with("hello", 0);
    operators::replace_char(&mut ta, 'H');
    assert_eq!(ta.text(), "Hello");
}

#[test]
fn test_normal_mode_motion_h_l() {
    let mut ta = textarea_with("hello", 2);
    let mut cmd = CommandState::Idle;
    let mut persistent = PersistentState::default();

    transitions::process_normal_key('h', &mut ta, &mut cmd, &mut persistent);
    assert_eq!(ta.cursor(), 1);

    transitions::process_normal_key('l', &mut ta, &mut cmd, &mut persistent);
    assert_eq!(ta.cursor(), 2);
}

#[test]
fn test_normal_mode_insert_keys() {
    let mut ta = textarea_with("hello", 0);
    let mut cmd = CommandState::Idle;
    let mut persistent = PersistentState::default();

    let action = transitions::process_normal_key('i', &mut ta, &mut cmd, &mut persistent);
    assert!(matches!(action, VimAction::EnterInsert));

    let action = transitions::process_normal_key('A', &mut ta, &mut cmd, &mut persistent);
    assert!(matches!(action, VimAction::EnterInsertEnd));
}

#[test]
fn test_vim_state_mode_label() {
    let state = VimState::new();
    assert_eq!(state.mode_label(), "NORMAL");

    let mut state = VimState::new();
    state.enter_insert();
    assert_eq!(state.mode_label(), "INSERT");
}

#[test]
fn test_delete_word_via_dw() {
    let mut ta = textarea_with("hello world", 0);
    let mut cmd = CommandState::Idle;
    let mut persistent = PersistentState::default();

    transitions::process_normal_key('d', &mut ta, &mut cmd, &mut persistent);
    assert!(matches!(cmd, CommandState::OperatorPending { .. }));

    transitions::process_normal_key('w', &mut ta, &mut cmd, &mut persistent);
    assert_eq!(ta.text(), "world");
    assert!(matches!(cmd, CommandState::Idle));
}

#[test]
fn dd_on_multiline_only_deletes_current_line() {
    // Multi-line bug regression: previously `dd` cleared the whole buffer.
    // It must only delete the current logical line + its trailing newline.
    let mut ta = textarea_with("first\nsecond\nthird", 7); // cursor on 's' of "second"
    let mut cmd = CommandState::Idle;
    let mut persistent = PersistentState::default();

    transitions::process_normal_key('d', &mut ta, &mut cmd, &mut persistent);
    transitions::process_normal_key('d', &mut ta, &mut cmd, &mut persistent);
    assert_eq!(ta.text(), "first\nthird");
    assert!(persistent.register_is_linewise);
    assert_eq!(persistent.register, "second");
}

#[test]
fn dd_on_last_line_eats_preceding_newline() {
    let mut ta = textarea_with("first\nsecond", 6); // cursor on 's' of "second"
    let mut cmd = CommandState::Idle;
    let mut persistent = PersistentState::default();

    transitions::process_normal_key('d', &mut ta, &mut cmd, &mut persistent);
    transitions::process_normal_key('d', &mut ta, &mut cmd, &mut persistent);
    assert_eq!(ta.text(), "first");
    assert_eq!(persistent.register, "second");
}

#[test]
fn dd_on_only_line_clears_buffer() {
    let mut ta = textarea_with("hello", 2);
    let mut cmd = CommandState::Idle;
    let mut persistent = PersistentState::default();

    transitions::process_normal_key('d', &mut ta, &mut cmd, &mut persistent);
    transitions::process_normal_key('d', &mut ta, &mut cmd, &mut persistent);
    assert_eq!(ta.text(), "");
    assert_eq!(persistent.register, "hello");
}

#[test]
fn delete_line_helper_directly() {
    let mut ta = textarea_with("a\nb\nc", 2); // cursor on 'b'
    let mut persistent = PersistentState::default();
    operators::delete_line(&mut ta, &mut persistent);
    assert_eq!(ta.text(), "a\nc");
    assert_eq!(persistent.register, "b");
    assert!(persistent.register_is_linewise);
}

#[test]
fn x_register_capture() {
    // `x` should stash the deleted char into the register so `p` works.
    let mut ta = textarea_with("hello", 0);
    let mut cmd = CommandState::Idle;
    let mut persistent = PersistentState::default();
    transitions::process_normal_key('x', &mut ta, &mut cmd, &mut persistent);
    assert_eq!(ta.text(), "ello");
    assert_eq!(persistent.register, "h");
}
