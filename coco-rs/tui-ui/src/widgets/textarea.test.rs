//! Tests for the [`TextArea`] widget. Covers CJK / wide-char rendering,
//! grapheme-aware delete, kill ring, multi-line wrap, and word movement.

use super::*;
use crate::widgets::wrap_ranges_with_elements;
use ratatui::layout::Rect;

fn ta_with(text: &str, cursor: usize) -> TextArea {
    let mut ta = TextArea::new();
    ta.set_text(text);
    ta.set_cursor(cursor);
    ta
}

// ─── Undo / redo (plan item C1) ────────────────────────────────────────────
//
// The batching rule table. Undo granularity is the whole point: one entry per
// keystroke makes Ctrl+Z useless on a prose composer, and one entry per
// buffer-state makes a `dw` take five presses to reverse.

/// Type `text` one grapheme at a time, exactly as the key bridge does.
fn type_text(ta: &mut TextArea, text: &str) {
    for grapheme in text.graphemes(true) {
        ta.insert_str(grapheme);
    }
}

#[test]
fn typing_one_word_is_one_undo_step() {
    let mut ta = TextArea::new();
    type_text(&mut ta, "hello");
    assert_eq!(ta.text(), "hello");

    assert!(ta.undo());
    assert_eq!(ta.text(), "", "a typed word must undo in one step");
    assert!(!ta.undo(), "the run must have left exactly one entry");
}

#[test]
fn typing_two_words_is_two_undo_steps() {
    // Whitespace closes the run, so undo lands on word boundaries rather than
    // wiping the whole line.
    let mut ta = TextArea::new();
    type_text(&mut ta, "hello world");

    assert!(ta.undo());
    assert_eq!(
        ta.text(),
        "hello ",
        "the first undo drops only the last word"
    );
    assert!(ta.undo());
    assert_eq!(ta.text(), "");
    assert!(!ta.undo());
}

#[test]
fn a_backspace_run_is_one_undo_step() {
    let mut ta = ta_with("hello", 5);
    for _ in 0..3 {
        ta.delete_backward(1);
    }
    assert_eq!(ta.text(), "he");

    assert!(ta.undo());
    assert_eq!(ta.text(), "hello", "a backspace run must undo in one step");
}

#[test]
fn a_cursor_move_breaks_the_typing_run() {
    // The run is contiguous only while edits continue where the last one ended.
    // Once the user moves the cursor, they started a different edit.
    let mut ta = TextArea::new();
    type_text(&mut ta, "ab");
    ta.move_cursor_left();
    type_text(&mut ta, "X");
    assert_eq!(ta.text(), "aXb");

    assert!(ta.undo());
    assert_eq!(ta.text(), "ab", "the post-move edit is its own step");
    assert!(ta.undo());
    assert_eq!(ta.text(), "");
}

#[test]
fn switching_edit_kind_breaks_the_run() {
    let mut ta = TextArea::new();
    type_text(&mut ta, "abc");
    ta.delete_backward(1);
    assert_eq!(ta.text(), "ab");

    assert!(ta.undo());
    assert_eq!(ta.text(), "abc", "the delete is its own step");
    assert!(ta.undo());
    assert_eq!(ta.text(), "");
}

#[test]
fn a_paste_is_its_own_undo_step() {
    // One deliberate action, one undo — never merged into surrounding typing.
    let mut ta = TextArea::new();
    type_text(&mut ta, "ab");
    ta.insert_str("PASTED");
    type_text(&mut ta, "cd");
    assert_eq!(ta.text(), "abPASTEDcd");

    assert!(ta.undo());
    assert_eq!(ta.text(), "abPASTED");
    assert!(ta.undo());
    assert_eq!(ta.text(), "ab", "the paste undoes as one unit");
    assert!(ta.undo());
    assert_eq!(ta.text(), "");
}

#[test]
fn a_word_kill_is_its_own_undo_step() {
    let mut ta = ta_with("alpha beta", 10);
    ta.delete_backward_word();
    assert_eq!(ta.text(), "alpha ");

    assert!(ta.undo());
    assert_eq!(ta.text(), "alpha beta");
}

#[test]
fn undo_group_collapses_a_compound_edit() {
    let mut ta = ta_with("hello", 5);
    ta.undo_group(|ta| {
        ta.delete_backward(1);
        ta.delete_backward(1);
        ta.insert_str("XY");
    });
    assert_eq!(ta.text(), "helXY");

    assert!(ta.undo());
    assert_eq!(ta.text(), "hello", "the whole group must undo at once");
    assert!(!ta.undo(), "a group leaves exactly one entry");
}

#[test]
fn an_undo_group_that_changes_nothing_leaves_no_entry() {
    let mut ta = ta_with("hello", 5);
    ta.undo_group(|ta| {
        ta.move_cursor_left();
    });
    assert!(
        !ta.undo(),
        "a group that only moved the cursor must not be undoable"
    );
}

#[test]
fn nested_undo_groups_commit_once() {
    let mut ta = ta_with("hello", 5);
    ta.undo_group(|ta| {
        ta.insert_str("A");
        ta.undo_group(|ta| ta.insert_str("B"));
    });
    assert_eq!(ta.text(), "helloAB");

    assert!(ta.undo());
    assert_eq!(ta.text(), "hello");
    assert!(!ta.undo(), "only the outermost group may commit");
}

#[test]
fn redo_replays_an_undone_edit() {
    let mut ta = TextArea::new();
    type_text(&mut ta, "hello");
    assert!(ta.undo());
    assert_eq!(ta.text(), "");

    assert!(ta.redo());
    assert_eq!(ta.text(), "hello");
    assert!(!ta.redo(), "nothing left to redo");
}

#[test]
fn redo_restores_the_cursor_too() {
    let mut ta = TextArea::new();
    type_text(&mut ta, "hi");
    assert!(ta.undo());
    assert!(ta.redo());
    assert_eq!(ta.text(), "hi");
    assert_eq!(ta.cursor(), 2, "redo must restore where the edit left off");
}

#[test]
fn a_new_edit_after_undo_kills_the_redo_branch() {
    // Redo must never resurrect text from a branch the user abandoned.
    let mut ta = TextArea::new();
    type_text(&mut ta, "hello");
    assert!(ta.undo());
    assert_eq!(ta.text(), "");

    type_text(&mut ta, "world");
    assert!(
        !ta.redo(),
        "typing after an undo must discard the redo branch"
    );
    assert_eq!(ta.text(), "world");
}

#[test]
fn set_text_after_undo_kills_the_redo_branch() {
    // Regression: `set_text` (history recall, reverse-search, stash) bypasses
    // `pre_mutate`, so it once left `redo_stack` intact — a later Redo then
    // resurrected the pre-recall text over the recalled buffer.
    let mut ta = TextArea::new();
    type_text(&mut ta, "hello");
    assert!(ta.undo());
    assert_eq!(ta.text(), "");

    ta.set_text("recalled command");
    assert!(
        !ta.redo(),
        "a buffer swap must discard the redo branch, not restore the old draft"
    );
    assert_eq!(ta.text(), "recalled command");
}

#[test]
fn set_text_is_an_edit_history_boundary_for_undo_too() {
    // Undo must not cross a recall/swap back into the prior buffer.
    let mut ta = TextArea::new();
    type_text(&mut ta, "typed draft");
    ta.set_text("recalled command");

    assert!(
        !ta.undo(),
        "undo must not reach across a buffer swap into the pre-recall draft"
    );
    assert_eq!(ta.text(), "recalled command");
}

#[test]
fn undo_group_restores_depth_when_the_closure_panics() {
    // A recovered panic inside a group must not leave `group_depth` stuck above
    // zero — that would silently suppress every future checkpoint. The group
    // re-raises the panic, so catch it and prove a LATER group still commits
    // (which only happens when depth is back at zero).
    let mut ta = ta_with("seed", 4);
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        ta.undo_group(|ta| {
            ta.insert_str("X");
            panic!("boom");
        });
    }));
    assert!(caught.is_err(), "the panic must propagate out of the group");

    let before = ta.text().to_string();
    ta.undo_group(|ta| ta.insert_str("YY"));
    assert_ne!(ta.text(), before, "the later group must have edited");
    assert!(
        ta.undo(),
        "a later group must still commit an undo entry — proving group_depth \
         was restored to zero after the panic"
    );
    assert_eq!(ta.text(), before, "and it must undo atomically");
}

#[test]
fn undo_and_redo_round_trip_repeatedly() {
    let mut ta = TextArea::new();
    type_text(&mut ta, "one two");
    let full = ta.text().to_string();

    assert!(ta.undo());
    assert!(ta.undo());
    assert_eq!(ta.text(), "");
    assert!(ta.redo());
    assert!(ta.redo());
    assert_eq!(ta.text(), full, "redo must walk back up the same path");
}

#[test]
fn undo_on_an_untouched_buffer_is_a_no_op() {
    let mut ta = TextArea::new();
    assert!(!ta.undo());
    assert!(!ta.redo());
}

#[test]
fn the_undo_stack_stays_bounded() {
    let mut ta = TextArea::new();
    // Each word is its own entry, so this pushes well past the cap.
    for i in 0..(UNDO_STACK_CAP * 2) {
        type_text(&mut ta, &format!("w{i} "));
    }
    assert!(
        ta.undo_stack.len() <= UNDO_STACK_CAP,
        "undo stack must stay bounded, got {}",
        ta.undo_stack.len()
    );
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

// ───────────────── Atomic text elements (plan item C3) ─────────────

fn insert_test_element(ta: &mut TextArea, source: &str, display: &str) -> ElementId {
    ta.insert_element(
        source,
        ElementKind::Paste,
        ElementDisplay::new(display, ratatui::style::Style::default().cyan()),
    )
    .expect("element ID available")
}

#[test]
fn cursor_motion_treats_an_element_as_one_unit() {
    let mut ta = ta_with("ab", 1);
    let id = insert_test_element(&mut ta, "[Pasted text #1]", "Pasted #1");
    let element = ta
        .elements()
        .iter()
        .find(|element| element.id == id)
        .expect("inserted element");
    let start = element.range.start;
    let end = element.range.end;

    assert_eq!(ta.cursor(), end);
    ta.move_cursor_left();
    assert_eq!(ta.cursor(), start);
    ta.move_cursor_right();
    assert_eq!(ta.cursor(), end);
}

#[test]
fn elements_reject_empty_or_multiline_content() {
    let mut ta = TextArea::new();
    assert!(
        ta.insert_element(
            "",
            ElementKind::Paste,
            ElementDisplay::new("chip", ratatui::style::Style::default()),
        )
        .is_err()
    );
    assert!(
        ta.insert_element(
            "two\nlines",
            ElementKind::Paste,
            ElementDisplay::new("chip", ratatui::style::Style::default()),
        )
        .is_err()
    );
    assert!(
        ta.insert_element(
            "token",
            ElementKind::Paste,
            ElementDisplay::new("two\nlines", ratatui::style::Style::default()),
        )
        .is_err()
    );
    assert!(ta.text().is_empty());
    assert!(ta.elements().is_empty());
}

#[test]
fn backspace_and_delete_remove_a_whole_element() {
    let mut backward = ta_with("x", 1);
    insert_test_element(&mut backward, "[Pasted text #1]", "Pasted #1");
    backward.delete_backward(1);
    assert_eq!(backward.text(), "x");
    assert!(backward.elements().is_empty());

    let mut forward = ta_with("x", 0);
    insert_test_element(&mut forward, "[Pasted text #1]", "Pasted #1");
    forward.set_cursor(0);
    forward.delete_forward(1);
    assert_eq!(forward.text(), "x");
    assert!(forward.elements().is_empty());
}

#[test]
fn adjacent_elements_remain_independent_atomic_cursor_steps() {
    let mut ta = TextArea::new();
    ta.insert_str("你");
    insert_test_element(&mut ta, "[Image #1]", "Image #1");
    insert_test_element(&mut ta, "[Pasted text #2]", "Pasted #2");
    ta.insert_str("界");
    let ranges: Vec<_> = ta
        .elements()
        .iter()
        .map(|element| element.range().clone())
        .collect();

    ta.set_cursor(ranges[0].start);
    ta.move_cursor_right();
    assert_eq!(ta.cursor(), ranges[0].end);
    ta.move_cursor_right();
    assert_eq!(ta.cursor(), ranges[1].end);
    ta.move_cursor_left();
    assert_eq!(ta.cursor(), ranges[1].start);
    ta.move_cursor_left();
    assert_eq!(ta.cursor(), ranges[0].start);
}

#[test]
fn unicode_edit_across_adjacent_elements_removes_each_payload_atomically() {
    let mut ta = TextArea::new();
    ta.insert_str("你");
    insert_test_element(&mut ta, "[Image #1]", "Image #1");
    insert_test_element(&mut ta, "[Pasted text #2]", "Pasted #2");
    ta.insert_str("界");
    let first_start = ta.elements()[0].range().start;
    let second_end = ta.elements()[1].range().end;

    ta.replace_range(first_start + 1..second_end - 1, "🙂");

    assert_eq!(ta.text(), "你🙂界");
    assert!(ta.elements().is_empty());
    assert!(ta.undo());
    assert_eq!(ta.elements().len(), 2);
}

#[test]
fn insertion_and_replacement_cannot_split_an_element() {
    let mut ta = TextArea::new();
    insert_test_element(&mut ta, "[Pasted text #1]", "Pasted #1");
    let range = ta.elements()[0].range.clone();

    ta.insert_str_at(range.start + 2, "before");
    assert!(ta.text().starts_with("before[Pasted text #1]"));
    assert_eq!(ta.elements()[0].range.start, "before".len());

    let range = ta.elements()[0].range.clone();
    ta.replace_range(range.start + 1..range.start + 2, "expanded");
    assert_eq!(ta.text(), "beforeexpanded");
    assert!(ta.elements().is_empty());
}

#[test]
fn reversed_replacement_ranges_still_expand_over_elements() {
    let mut ta = ta_with("xy", 1);
    insert_test_element(&mut ta, "[Pasted text #1]", "Pasted #1");
    ta.set_cursor(ta.text().len());
    let element = ta.elements()[0].range().clone();
    ta.replace_range(ta.text().len()..element.start + 1, "z");
    assert_eq!(ta.text(), "xz");
    assert!(ta.elements().is_empty());
}

#[test]
fn undo_and_redo_restore_element_metadata_atomically() {
    let mut ta = TextArea::new();
    let id = insert_test_element(&mut ta, "[Pasted text #1]", "Pasted #1");
    assert!(ta.undo());
    assert!(ta.text().is_empty());
    assert!(ta.elements().is_empty());

    assert!(ta.redo());
    assert_eq!(ta.text(), "[Pasted text #1]");
    assert_eq!(ta.elements()[0].id, id);
}

#[test]
fn replacing_an_element_range_is_one_undo_step() {
    let mut ta = TextArea::new();
    let id = insert_test_element(&mut ta, "[Pasted text #1]", "Pasted #1");
    let range = ta.elements()[0].range().clone();
    ta.replace_range(range, "full pasted payload");
    assert_eq!(ta.text(), "full pasted payload");
    assert!(ta.elements().is_empty());

    assert!(ta.undo());
    assert_eq!(ta.text(), "[Pasted text #1]");
    assert_eq!(ta.elements()[0].id, id);
}

#[test]
fn projection_and_wrap_use_element_display_width() {
    let mut ta = ta_with("ab cd", 3);
    insert_test_element(&mut ta, "[a deliberately long source token]", "chip");
    let projection = ta.display_projection_with_width(0..ta.text().len(), u16::MAX);
    assert_eq!(projection.text(), "ab chipcd");
    assert_eq!(projection.elements()[0].range(), &(3..7));

    let rows = wrap_ranges_with_elements(projection.text(), 7, projection.elements());
    let rendered = rows
        .iter()
        .filter_map(|range| projection.text().get(range.clone()).map(str::to_string))
        .collect::<Vec<_>>();
    assert_eq!(rendered, vec!["ab ", "chipcd"]);
}

#[test]
fn narrow_projection_fits_an_atomic_label_without_splitting_it() {
    let mut ta = TextArea::new();
    insert_test_element(
        &mut ta,
        "[a deliberately long source token]",
        "long chip label",
    );
    let projection = ta.display_projection_with_width(0..ta.text().len(), 5);
    assert_eq!(projection.text(), "long…");
    assert_eq!(projection.elements().len(), 1);
    assert_eq!(projection.elements()[0].range(), &(0.."long…".len()));
    assert_eq!(unicode_width::UnicodeWidthStr::width(projection.text()), 5);
    assert_eq!(
        wrap_ranges_with_elements(projection.text(), 5, projection.elements()),
        vec![0.."long…".len()]
    );
}

#[test]
fn width_aware_vertical_motion_works_before_any_render() {
    let mut ta = TextArea::new();
    insert_test_element(&mut ta, "[long source token]", "long chip label");
    ta.insert_str("abcdef");
    ta.set_cursor(ta.text().len());

    ta.move_cursor_up_at_width(5);
    assert_eq!(ta.cursor(), "[long source token]".len() + 1);
    ta.move_cursor_up_at_width(5);
    assert_eq!(ta.cursor(), 0, "the first visual row starts at the chip");
    ta.move_cursor_down_at_width(5);
    assert!(ta.cursor() >= "[long source token]".len());
    assert!(ta.text().is_char_boundary(ta.cursor()));
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
fn plain_text_kill_ring_rejects_atomic_elements() {
    let mut ta = ta_with("seed", 4);
    ta.kill_to_beginning_of_line();
    ta.set_text("a");
    ta.set_cursor(1);
    insert_test_element(&mut ta, "[Image #1]", "Image #1");
    ta.insert_str("b");
    ta.set_cursor(0);
    let before = ta.text().to_string();

    ta.kill_to_end_of_line();
    assert_eq!(ta.text(), before);
    assert_eq!(ta.elements().len(), 1);

    ta.set_cursor(ta.text().len());
    ta.yank();
    assert!(ta.text().ends_with("seed"));
    assert_eq!(ta.elements().len(), 1);
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

// ─── Word-boundary soft wrap (plan item C2) ───────────────────────────────

/// The wrapped rows as text, for readable assertions.
fn wrapped_text(ta: &TextArea, width: u16) -> Vec<String> {
    ta.wrapped_lines(width)
        .iter()
        .map(|range| ta.text()[range.clone()].to_string())
        .collect()
}

/// Every byte of the source must land in exactly one row, in order — the
/// contract `cursor_pos` maps a byte offset through.
fn assert_rows_tile_the_text(ta: &TextArea, width: u16) {
    let lines = ta.wrapped_lines(width);
    let mut expected_start = 0usize;
    for (row, range) in lines.iter().enumerate() {
        // Rows are separated either by a newline (which no row covers) or by
        // nothing at all (a soft wrap).
        let gap = range.start - expected_start;
        assert!(
            gap <= 1,
            "row {row} skipped {gap} bytes at {expected_start}: rows must tile the text"
        );
        expected_start = range.end;
    }
    assert_eq!(
        expected_start,
        ta.text().len(),
        "the rows must cover the text through its end"
    );
}

#[test]
fn wrapped_lines_break_at_word_boundaries() {
    // The defect: width-based wrapping cut words in half ("hello wo" / "rld").
    let ta = ta_with("hello world", 8);
    assert_eq!(wrapped_text(&ta, 8), vec!["hello ", "world"]);
    assert_rows_tile_the_text(&ta, 8);
}

#[test]
fn wrapped_lines_break_a_word_too_long_for_the_row() {
    // A word wider than the row has no boundary to break at; a mid-word break
    // is the only way to show it at all.
    let ta = ta_with("supercalifragilistic", 0);
    let rows = wrapped_text(&ta, 6);
    assert!(rows.len() > 1);
    assert!(
        rows.iter().all(|row| row.chars().count() <= 6),
        "no row may exceed the width: {rows:?}"
    );
    assert_eq!(rows.concat(), "supercalifragilistic");
    assert_rows_tile_the_text(&ta, 6);
}

#[test]
fn wrapped_lines_break_a_long_word_after_a_short_one() {
    // The word boundary is taken first, then the overlong word hard-breaks.
    let ta = ta_with("hi supercalifragilistic", 0);
    let rows = wrapped_text(&ta, 6);
    assert_eq!(rows[0], "hi ");
    assert!(
        rows[1..].iter().all(|row| row.chars().count() <= 6),
        "the long word must still be broken to fit: {rows:?}"
    );
    assert_eq!(rows.concat(), "hi supercalifragilistic");
    assert_rows_tile_the_text(&ta, 6);
}

#[test]
fn wrapped_lines_keep_multiple_words_per_row() {
    // Rows pack greedily up to the width: "ccc dddd" is exactly 8 columns.
    // The break lands on the space that would have overflowed row 1, which
    // also keeps the continuation row free of a leading space.
    let ta = ta_with("a bb ccc dddd", 0);
    assert_eq!(wrapped_text(&ta, 8), vec!["a bb ", "ccc dddd"]);
    assert_rows_tile_the_text(&ta, 8);
}

#[test]
fn no_wrapped_row_exceeds_the_width() {
    // The other half of the render contract: a row wider than the viewport
    // would be clipped, which is the invisible-text bug in a different guise.
    let ta = ta_with("the quick brown fox jumps over the lazy dog 你好世界", 0);
    for width in 2..=30u16 {
        for range in ta.wrapped_lines(width).iter() {
            let row = &ta.text()[range.clone()];
            let row_width = unicode_width::UnicodeWidthStr::width(row);
            assert!(
                row_width <= width as usize,
                "row {row:?} is {row_width} cols at width {width}"
            );
        }
    }
}

#[test]
fn wrapped_lines_never_lose_bytes_across_widths() {
    // The invisible-text bug class: any row set must reconstruct the source.
    let ta = ta_with("the quick brown fox jumps over the lazy dog", 0);
    for width in 1..=44u16 {
        let rows = wrapped_text(&ta, width);
        assert_eq!(
            rows.concat(),
            ta.text(),
            "width {width} lost or duplicated text: {rows:?}"
        );
        assert_rows_tile_the_text(&ta, width);
    }
}

#[test]
fn wrapped_lines_wrap_cjk_without_needing_spaces() {
    // CJK has no spaces, so every grapheme is a break opportunity — the
    // word-boundary preference must not stop it from wrapping.
    let ta = ta_with("你好世界你好", 0);
    assert_eq!(wrapped_text(&ta, 4), vec!["你好", "世界", "你好"]);
    assert_rows_tile_the_text(&ta, 4);
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
