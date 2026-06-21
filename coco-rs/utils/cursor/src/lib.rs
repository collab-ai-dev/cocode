//! Text cursor management with kill ring and word boundaries.
//!
//! Provides cursor operations for multi-line text editing:
//! - Character/word/line movement
//! - Kill ring (cut text buffer for Ctrl+K/Y)
//! - Word boundary detection
//! - UTF-8 safe character indexing

/// Maximum entries in the kill ring.
const KILL_RING_MAX: usize = 10;

/// Text cursor with kill ring support.
#[derive(Debug, Default)]
pub struct Cursor {
    /// Current cursor position (character index, NOT byte).
    pub pos: i32,
    /// Kill ring — stores killed (cut) text for yank.
    kill_ring: Vec<String>,
    /// Current kill ring index for yank-pop.
    kill_ring_index: usize,
    /// Whether the last action was a kill (for accumulation).
    last_was_kill: bool,
}

impl Cursor {
    /// Create a new cursor at position 0.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a cursor at a specific position.
    pub fn at(pos: i32) -> Self {
        Self {
            pos,
            ..Default::default()
        }
    }

    /// Move cursor left by one character, clamped at 0.
    pub fn left(&mut self) {
        self.pos = (self.pos - 1).max(0);
        self.last_was_kill = false;
    }

    /// Move cursor right by one character, clamped at text length.
    pub fn right(&mut self, text_len: i32) {
        self.pos = (self.pos + 1).min(text_len);
        self.last_was_kill = false;
    }

    /// Move cursor to start of line.
    pub fn home(&mut self) {
        self.pos = 0;
        self.last_was_kill = false;
    }

    /// Move cursor to end of text.
    pub fn end(&mut self, text_len: i32) {
        self.pos = text_len;
        self.last_was_kill = false;
    }

    /// Move cursor one word left.
    ///
    /// Skips whitespace, then moves to the start of the previous word.
    pub fn word_left(&mut self, text: &str) {
        let chars: Vec<char> = text.chars().collect();
        let mut pos = self.pos as usize;

        // Skip whitespace
        while pos > 0 && chars.get(pos - 1).is_some_and(|c| c.is_whitespace()) {
            pos -= 1;
        }
        // Skip word characters
        while pos > 0 && chars.get(pos - 1).is_some_and(|c| !c.is_whitespace()) {
            pos -= 1;
        }

        self.pos = pos as i32;
        self.last_was_kill = false;
    }

    /// Move cursor one word right.
    ///
    /// Skips current word, then skips whitespace.
    pub fn word_right(&mut self, text: &str) {
        let chars: Vec<char> = text.chars().collect();
        let len = chars.len();
        let mut pos = self.pos as usize;

        // Skip word characters
        while pos < len && !chars[pos].is_whitespace() {
            pos += 1;
        }
        // Skip whitespace
        while pos < len && chars[pos].is_whitespace() {
            pos += 1;
        }

        self.pos = pos as i32;
        self.last_was_kill = false;
    }

    /// Kill (cut) text from cursor to end of line.
    ///
    /// If cursor is at end, kills the newline character.
    /// Consecutive kills accumulate in the kill ring.
    pub fn kill_to_end(&mut self, text: &str) -> Option<KillResult> {
        let chars: Vec<char> = text.chars().collect();
        let pos = self.pos as usize;

        if pos >= chars.len() {
            return None;
        }

        // Find end of current line
        let end = chars[pos..]
            .iter()
            .position(|c| *c == '\n')
            .map(|i| pos + i)
            .unwrap_or(chars.len());

        // If at end of line, kill the newline
        let end = if end == pos && pos < chars.len() {
            pos + 1
        } else {
            end
        };

        let killed: String = chars[pos..end].iter().collect();

        if killed.is_empty() {
            return None;
        }

        // Accumulate if consecutive kill
        if let Some(last) = self.kill_ring.last_mut()
            && self.last_was_kill
        {
            last.push_str(&killed);
        } else {
            if self.kill_ring.len() >= KILL_RING_MAX {
                self.kill_ring.remove(0);
            }
            self.kill_ring.push(killed.clone());
            self.kill_ring_index = self.kill_ring.len() - 1;
        }

        self.last_was_kill = true;

        Some(KillResult {
            killed,
            start: pos,
            end,
        })
    }

    /// Yank (paste) the most recent kill ring entry.
    ///
    /// Returns the text to insert at the current cursor position.
    pub fn yank(&mut self) -> Option<&str> {
        self.last_was_kill = false;
        self.kill_ring.last().map(String::as_str)
    }

    /// Yank-pop — cycle through kill ring entries (after yank).
    pub fn yank_pop(&mut self) -> Option<&str> {
        if self.kill_ring.is_empty() {
            return None;
        }
        if self.kill_ring_index > 0 {
            self.kill_ring_index -= 1;
        } else {
            self.kill_ring_index = self.kill_ring.len() - 1;
        }
        self.kill_ring.get(self.kill_ring_index).map(String::as_str)
    }

    /// Convert character position to byte offset in the string.
    pub fn to_byte_offset(&self, text: &str) -> usize {
        text.char_indices()
            .nth(self.pos as usize)
            .map(|(i, _)| i)
            .unwrap_or(text.len())
    }
}

/// Result of a kill operation.
#[derive(Debug)]
pub struct KillResult {
    /// The killed text.
    pub killed: String,
    /// Start position (character index).
    pub start: usize,
    /// End position (character index).
    pub end: usize,
}

/// Find the word at a given position (for double-click selection).
pub fn word_at(text: &str, pos: i32) -> Option<(i32, i32)> {
    let chars: Vec<char> = text.chars().collect();
    let p = pos as usize;

    if p >= chars.len() {
        return None;
    }

    if chars[p].is_whitespace() {
        return None;
    }

    let mut start = p;
    while start > 0 && !chars[start - 1].is_whitespace() {
        start -= 1;
    }

    let mut end = p;
    while end < chars.len() && !chars[end].is_whitespace() {
        end += 1;
    }

    Some((start as i32, end as i32))
}

#[cfg(test)]
#[path = "lib.test.rs"]
mod tests;
