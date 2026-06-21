//! Vim operators — `d` / `c` / `y` applied to byte ranges.
//!
//! Operates directly on `TextArea` using its byte-offset cursor and
//! `replace_range` API. The register lives in `PersistentState` so it
//! survives across keystrokes within the same vim session.

use std::ops::Range;

use super::Operator;
use super::PersistentState;
use super::motions::next_char_boundary;
use super::motions::prev_char_boundary;
use coco_tui_ui::widgets::TextArea;

/// Apply an operator to a half-open byte range.
///
/// `range.start..range.end` is the slice the motion produced; `apply_operator`
/// canonicalizes it (clamp + swap if reversed). Returns the operated text
/// so transitions.rs can record it for dot-repeat if needed.
pub(super) fn apply_operator(
    textarea: &mut TextArea,
    op: Operator,
    range: Range<usize>,
    persistent: &mut PersistentState,
) -> String {
    let text_len = textarea.text().len();
    let lo = range.start.min(range.end).min(text_len);
    let hi = range.start.max(range.end).min(text_len);
    let operated = textarea.text()[lo..hi].to_string();

    match op {
        Operator::Delete | Operator::Change => {
            textarea.replace_range(lo..hi, "");
            textarea.set_cursor(lo);
            persistent.register = operated.clone();
            persistent.register_is_linewise = false;
        }
        Operator::Yank => {
            persistent.register = operated.clone();
            persistent.register_is_linewise = false;
            // Yank leaves cursor at the start of the operated range (vim
            // convention).
            textarea.set_cursor(lo);
        }
    }

    operated
}

/// Delete the entire current line (vim `dd`).
///
/// Single-line buffer: clear contents. Multi-line: delete `[bol, eol+1)`
/// to include the trailing newline; if on the LAST line (no trailing
/// newline) include the preceding newline instead so the line above
/// becomes the new "current" line.
pub(super) fn delete_line(textarea: &mut TextArea, persistent: &mut PersistentState) {
    let (range, operated) = compute_line_range(textarea);
    persistent.register = operated;
    persistent.register_is_linewise = true;
    let target_cursor = range.start;
    textarea.replace_range(range, "");
    textarea.set_cursor(target_cursor.min(textarea.text().len()));
}

/// Change the entire current line (vim `cc`). Identical to `delete_line`
/// in effect — transitions.rs handles the mode switch to Insert.
pub(super) fn change_line(textarea: &mut TextArea, persistent: &mut PersistentState) {
    delete_line(textarea, persistent);
}

/// Yank the entire current line (vim `yy`).
pub(super) fn yank_line(textarea: &mut TextArea, persistent: &mut PersistentState) {
    let line = current_line_content(textarea);
    persistent.register = line;
    persistent.register_is_linewise = true;
}

fn current_line_content(textarea: &TextArea) -> String {
    let bol = textarea.beginning_of_current_line();
    let eol = textarea.end_of_current_line();
    textarea.text()[bol..eol].to_string()
}

/// Compute the range and content for `dd`/`cc`.
fn compute_line_range(textarea: &TextArea) -> (Range<usize>, String) {
    let text = textarea.text();
    let len = text.len();
    let bol = textarea.beginning_of_current_line();
    let eol = textarea.end_of_current_line();
    let content = text[bol..eol].to_string();

    if eol < len {
        // Has a trailing newline; eat it so the line below moves up.
        (bol..eol + 1, content)
    } else if bol > 0 {
        // Last line, has a preceding newline; eat that instead.
        (bol - 1..eol, content)
    } else {
        // Only line in the buffer; clear in place.
        (0..eol, content)
    }
}

/// Put (paste) from the register after the cursor (vim `p`).
pub(super) fn put_after(textarea: &mut TextArea, persistent: &PersistentState) {
    if persistent.register.is_empty() {
        return;
    }
    if persistent.register_is_linewise {
        let eol = textarea.end_of_current_line();
        let line = persistent.register.trim_end_matches('\n');
        // Insert `\n<line>` after the current line. Cursor lands at the
        // start of the newly inserted line (`eol + 1`).
        let inserted = format!("\n{line}");
        textarea.insert_str_at(eol, &inserted);
        textarea.set_cursor(eol + 1);
    } else {
        // Insert after the current grapheme; cursor lands on the LAST char
        // of the pasted content.
        let insert_at = next_char_boundary(textarea.text(), textarea.cursor());
        let len = persistent.register.len();
        textarea.insert_str_at(insert_at, &persistent.register);
        let end_of_paste = insert_at + len;
        textarea.set_cursor(prev_char_boundary(textarea.text(), end_of_paste));
    }
}

/// Put (paste) from the register before the cursor (vim `P`).
pub(super) fn put_before(textarea: &mut TextArea, persistent: &PersistentState) {
    if persistent.register.is_empty() {
        return;
    }
    if persistent.register_is_linewise {
        let bol = textarea.beginning_of_current_line();
        let line = persistent.register.trim_end_matches('\n');
        let inserted = format!("{line}\n");
        textarea.insert_str_at(bol, &inserted);
        textarea.set_cursor(bol);
    } else {
        let insert_at = textarea.cursor();
        let len = persistent.register.len();
        textarea.insert_str_at(insert_at, &persistent.register);
        let end_of_paste = insert_at + len;
        textarea.set_cursor(prev_char_boundary(textarea.text(), end_of_paste));
    }
}

/// Replace the character under the cursor with `ch` (vim `r<char>`).
pub(super) fn replace_char(textarea: &mut TextArea, ch: char) {
    let cursor = textarea.cursor();
    let text_len = textarea.text().len();
    if cursor >= text_len {
        return;
    }
    let end = next_char_boundary(textarea.text(), cursor);
    let mut buf = [0u8; 4];
    textarea.replace_range(cursor..end, ch.encode_utf8(&mut buf));
    textarea.set_cursor(cursor);
}
