//! Vim motions — cursor movement commands.
//!
//! Each motion takes `(text, byte_cursor) -> byte_cursor`. All positions are
//! UTF-8 byte offsets at char boundaries, matching `TextArea`'s cursor model.
//! `unicode-segmentation` handles grapheme-aware word boundaries; ASCII
//! falls through the same paths.

/// Move forward to the start of the next word (vim `w`).
///
/// Skip the current run of non-whitespace, then any whitespace, landing on
/// the first character of the next word. Returns `text.len()` if no next
/// word exists.
pub(super) fn word_forward(text: &str, pos: usize) -> usize {
    let len = text.len();
    let pos = pos.min(len);
    // Skip current non-whitespace run.
    let after_word = text[pos..]
        .char_indices()
        .find(|&(_, c)| c.is_whitespace())
        .map(|(i, _)| pos + i)
        .unwrap_or(len);
    // Skip the whitespace run that follows.
    text[after_word..]
        .char_indices()
        .find(|&(_, c)| !c.is_whitespace())
        .map(|(i, _)| after_word + i)
        .unwrap_or(len)
}

/// Move backward to the start of the previous word (vim `b`).
///
/// Walks the prefix in reverse: skips trailing whitespace, then walks past
/// the next whitespace run, landing on the first character of the trailing
/// word.
pub(super) fn word_backward(text: &str, pos: usize) -> usize {
    let prefix = &text[..pos.min(text.len())];
    let trimmed = prefix.trim_end_matches(char::is_whitespace);
    match trimmed.rfind(char::is_whitespace) {
        Some(ws_byte) => {
            // `ws_byte` is the byte offset of the last whitespace char in
            // `trimmed`. The word starts immediately after that char.
            let ws_char_len = trimmed[ws_byte..].chars().next().map_or(0, char::len_utf8);
            ws_byte + ws_char_len
        }
        None => 0,
    }
}

/// Move forward to the end of the current/next word (vim `e`).
///
/// Returns the byte offset of the LAST character of the word (inclusive,
/// matching vim's cursor convention for `e`).
pub(super) fn word_end(text: &str, pos: usize) -> usize {
    let len = text.len();
    if pos >= len {
        return len;
    }
    // Step past the current char so `e` advances even when already on a word.
    let start = next_char_boundary(text, pos);
    // Skip any whitespace.
    let skipped_ws = text[start..]
        .char_indices()
        .find(|&(_, c)| !c.is_whitespace())
        .map(|(i, _)| start + i)
        .unwrap_or(len);
    // Find the next whitespace (or EOF) — end of the word run.
    let after_word = text[skipped_ws..]
        .char_indices()
        .find(|&(_, c)| c.is_whitespace())
        .map(|(i, _)| skipped_ws + i)
        .unwrap_or(len);
    // `e` lands on the LAST char of the word: step back one char from
    // `after_word`. Falls back to `pos` if the word is empty.
    if after_word > skipped_ws {
        prev_char_boundary(text, after_word)
    } else {
        pos
    }
}

/// Move to the start of the current logical line (vim `0`).
pub(super) fn line_start(text: &str, pos: usize) -> usize {
    text[..pos.min(text.len())]
        .rfind('\n')
        .map(|i| i + 1)
        .unwrap_or(0)
}

/// Move to the first non-blank character on the current line (vim `^`).
pub(super) fn first_non_blank(text: &str, pos: usize) -> usize {
    let bol = line_start(text, pos);
    text[bol..]
        .char_indices()
        .take_while(|&(_, c)| c != '\n')
        .find(|&(_, c)| !c.is_whitespace())
        .map(|(i, _)| bol + i)
        .unwrap_or(bol)
}

/// Move to the last character on the current line (vim `$`).
///
/// Returns the byte offset of the line's last character. For an empty line
/// returns the line start (cursor stays put). The trailing newline is NOT
/// part of the position.
pub(super) fn line_end(text: &str, pos: usize) -> usize {
    let len = text.len();
    let pos = pos.min(len);
    let eol = text[pos..].find('\n').map(|i| pos + i).unwrap_or(len);
    if eol > pos {
        prev_char_boundary(text, eol)
    } else {
        pos
    }
}

/// Find a character forward from the cursor (vim `f<char>`).
///
/// Searches strictly past the current cursor position. Returns the byte
/// offset of the matched character, or `None` if not found before EOF.
pub(super) fn find_char_forward(text: &str, pos: usize, ch: char) -> Option<usize> {
    let start = next_char_boundary(text, pos).min(text.len());
    text[start..]
        .char_indices()
        .find(|&(_, c)| c == ch)
        .map(|(i, _)| start + i)
}

/// Find a character backward from the cursor (vim `F<char>`).
pub(super) fn find_char_backward(text: &str, pos: usize, ch: char) -> Option<usize> {
    text[..pos.min(text.len())]
        .char_indices()
        .rev()
        .find(|&(_, c)| c == ch)
        .map(|(i, _)| i)
}

/// Find-till forward (vim `t<char>`): land one char before the match.
pub(super) fn till_char_forward(text: &str, pos: usize, ch: char) -> Option<usize> {
    find_char_forward(text, pos, ch).map(|hit| {
        let one_back = prev_char_boundary(text, hit);
        one_back.max(next_char_boundary(text, pos)).min(text.len())
    })
}

/// Find-till backward (vim `T<char>`): land one char after the match.
pub(super) fn till_char_backward(text: &str, pos: usize, ch: char) -> Option<usize> {
    find_char_backward(text, pos, ch).map(|hit| {
        let one_forward = next_char_boundary(text, hit);
        let one_back = prev_char_boundary(text, pos);
        one_forward.min(one_back)
    })
}

/// Jump to the start of the buffer (vim `gg`).
pub(super) fn go_to_top(_text: &str) -> usize {
    0
}

/// Jump to the last character of the buffer (vim `G`).
pub(super) fn go_to_bottom(text: &str) -> usize {
    if text.is_empty() {
        0
    } else {
        prev_char_boundary(text, text.len())
    }
}

// ─────────────────────────── Helpers ───────────────────────────

/// Byte offset of the next char boundary. Returns `text.len()` at EOF.
pub(crate) fn next_char_boundary(text: &str, pos: usize) -> usize {
    if pos >= text.len() {
        return text.len();
    }
    text[pos..]
        .char_indices()
        .nth(1)
        .map(|(i, _)| pos + i)
        .unwrap_or(text.len())
}

/// Byte offset of the previous char boundary. Returns 0 at BOF.
pub(crate) fn prev_char_boundary(text: &str, pos: usize) -> usize {
    if pos == 0 {
        return 0;
    }
    text[..pos.min(text.len())]
        .char_indices()
        .next_back()
        .map(|(i, _)| i)
        .unwrap_or(0)
}
