//! Display-width-aware text truncation.
//!
//! Absorbed from jcode's `jcode-tui-render` layout helpers: truncating by byte
//! or `char` count corrupts CJK / emoji (width-2) and breaks alignment. These
//! helpers measure and cut by terminal columns and are char-boundary safe.

use unicode_width::UnicodeWidthChar;
use unicode_width::UnicodeWidthStr;

/// The single-column ellipsis appended when text is truncated.
pub const ELLIPSIS: char = '…';

/// Display width of `s` in terminal columns (CJK / wide emoji count as 2).
pub fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Truncate `s` to at most `max_cols` display columns. When truncation occurs
/// the result ends with [`ELLIPSIS`] and still fits within `max_cols`. Never
/// splits a wide grapheme across the budget.
pub fn truncate_to_width(s: &str, max_cols: usize) -> String {
    if display_width(s) <= max_cols {
        return s.to_string();
    }
    if max_cols == 0 {
        return String::new();
    }
    // Reserve one column for the ellipsis.
    let budget = max_cols - 1;
    let mut width = 0usize;
    let mut out = String::new();
    for ch in s.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + cw > budget {
            break;
        }
        width += cw;
        out.push(ch);
    }
    out.push(ELLIPSIS);
    out
}

#[cfg(test)]
#[path = "truncate.test.rs"]
mod tests;
