//! Vim text objects — select regions of text.
//!
//! Each text object returns a half-open `Range<usize>` of byte offsets,
//! the same shape `TextArea::replace_range` consumes. Returning `None`
//! means the cursor isn't positioned on a valid object (e.g. `i"` away
//! from any quoted string).

use std::ops::Range;

use super::TextObjScope;
use super::motions::next_char_boundary;

/// Result of resolving a text object: byte range to operate on, or `None`
/// if no object exists at the cursor.
pub type TextObjResult = Option<Range<usize>>;

/// Word text object (`iw` / `aw`).
///
/// Inside a word: span the whole word. Inside whitespace: span the whole
/// whitespace run. `Around` extends to include the trailing whitespace
/// after a word.
pub(super) fn word(text: &str, pos: usize, scope: TextObjScope) -> TextObjResult {
    if text.is_empty() || pos >= text.len() {
        return None;
    }
    let cur_ch = text[pos..].chars().next()?;
    let cur_is_ws = cur_ch.is_whitespace();

    // Walk backward to the start of the current run (same is_whitespace
    // category as the cursor's char).
    let start = text[..pos]
        .char_indices()
        .rev()
        .take_while(|&(_, c)| c.is_whitespace() == cur_is_ws)
        .last()
        .map(|(i, _)| i)
        .unwrap_or(pos);

    // Walk forward to the byte AFTER the last char of the run.
    let end_of_run = text[pos..]
        .char_indices()
        .find(|&(_, c)| c.is_whitespace() != cur_is_ws)
        .map(|(i, _)| pos + i)
        .unwrap_or(text.len());

    // For `aw` on a word, swallow the trailing whitespace run too.
    let end = if scope == TextObjScope::Around && !cur_is_ws {
        text[end_of_run..]
            .char_indices()
            .find(|&(_, c)| !c.is_whitespace())
            .map(|(i, _)| end_of_run + i)
            .unwrap_or(text.len())
    } else {
        end_of_run
    };

    Some(start..end)
}

/// Quoted text object (`i"` / `a"` / `i'` / `a'`).
pub(super) fn quoted(text: &str, pos: usize, quote: char, scope: TextObjScope) -> TextObjResult {
    // Find opening quote at or before the cursor.
    let scan_end = next_char_boundary(text, pos);
    let open = text[..scan_end]
        .char_indices()
        .rev()
        .find(|&(_, c)| c == quote)
        .map(|(i, _)| i)?;

    // Find closing quote after the opening one.
    let after_open = open + quote.len_utf8();
    let close_rel = text[after_open..]
        .char_indices()
        .find(|&(_, c)| c == quote)
        .map(|(i, _)| i)?;
    let close = after_open + close_rel;

    match scope {
        TextObjScope::Inner => Some(after_open..close),
        TextObjScope::Around => Some(open..close + quote.len_utf8()),
    }
}

/// Bracket text object (`i(` / `a(` / `i{` / `a{` / `i[` / `a[`).
pub(super) fn bracket(
    text: &str,
    pos: usize,
    open_ch: char,
    close_ch: char,
    scope: TextObjScope,
) -> TextObjResult {
    // Walk backward to the matching opening bracket (respect nesting).
    let scan_end = next_char_boundary(text, pos);
    let mut depth: i32 = 0;
    let mut open = None;
    for (i, c) in text[..scan_end].char_indices().rev() {
        if c == close_ch {
            depth += 1;
        }
        if c == open_ch {
            if depth == 0 {
                open = Some(i);
                break;
            }
            depth -= 1;
        }
    }
    let open = open?;

    // Walk forward to the matching closing bracket.
    let after_open = open + open_ch.len_utf8();
    depth = 0;
    let mut close = None;
    for (i, c) in text[after_open..].char_indices() {
        if c == open_ch {
            depth += 1;
        }
        if c == close_ch {
            if depth == 0 {
                close = Some(after_open + i);
                break;
            }
            depth -= 1;
        }
    }
    let close = close?;

    match scope {
        TextObjScope::Inner => Some(after_open..close),
        TextObjScope::Around => Some(open..close + close_ch.len_utf8()),
    }
}
