//! Editable text buffer for the chat composer.
//!
//! Owns byte-offset cursor + multi-line wrapped rendering + grapheme-aware
//! navigation + a single-entry kill buffer. Ported as a leaner subset of
//! `codex-rs/tui/src/bottom_pane/textarea.rs` (3418 LOC → ~650 LOC) with
//! these deliberate omissions vs upstream:
//!
//! - Modal Vim state (`coco-vim` owns vim semantics via `vim/wiring.rs`).
//! - `EditorKeymap` / `VimNormalKeymap` dispatch (`coco-tui`'s
//!   `keybinding_bridge` produces `TuiCommand`s; TextArea only exposes
//!   verbs).
//! - `pub fn input(&mut self, event: KeyEvent)` — never call; the bridge
//!   owns key→verb mapping.
//! - `TextElement` / placeholder ranges (paste pills live at `paste.rs`).
//! - `StatefulWidgetRef` / viewport scroll (the single-line composer
//!   doesn't need it yet; multi-line callers can read `wrapped_lines`
//!   and render themselves).
//!
//! The single-entry kill buffer mirrors codex-rs precisely:
//! whole-buffer replacement (`set_text`, `take_text`) intentionally keeps
//! the kill buffer alive so `Ctrl+Y` can recover the user's most recent
//! `Ctrl+K` even after submit / `/clear` clears the visible draft.

use std::cell::Ref;
use std::cell::RefCell;
use std::ops::Range;

use ratatui::layout::Rect;
use unicode_segmentation::GraphemeCursor;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Punctuation characters that split an alphanumeric word run into pieces
/// for emacs-style word movement (Alt+B / Alt+F). Mirrors codex-rs's
/// `WORD_SEPARATORS`.
const WORD_SEPARATORS: &str = "`~!@#$%^&*()-=+[{]}\\|;:'\",.<>/?";

fn is_word_separator(ch: char) -> bool {
    WORD_SEPARATORS.contains(ch)
}

/// Split a contiguous run of non-whitespace bytes into "pieces" that share
/// the same separator/non-separator category. Returns `(byte_offset_in_run,
/// piece_slice)` pairs in source order. Mirrors codex-rs.
fn split_word_pieces(run: &str) -> Vec<(usize, &str)> {
    let mut pieces = Vec::new();
    for (segment_start, segment) in run.split_word_bound_indices() {
        let mut piece_start = 0;
        let mut chars = segment.char_indices();
        let Some((_, first_char)) = chars.next() else {
            continue;
        };
        let mut in_separator = is_word_separator(first_char);

        for (idx, ch) in chars {
            let is_separator = is_word_separator(ch);
            if is_separator == in_separator {
                continue;
            }
            pieces.push((segment_start + piece_start, &segment[piece_start..idx]));
            piece_start = idx;
            in_separator = is_separator;
        }

        pieces.push((segment_start + piece_start, &segment[piece_start..]));
    }
    pieces
}

#[derive(Debug, Clone, Default)]
struct WrapCache {
    /// Terminal width the cached lines were computed for. `u16::MAX` is
    /// reserved as a sentinel meaning "uninitialized / dirty"; real
    /// terminal widths never reach `u16::MAX`.
    width: u16,
    lines: Vec<Range<usize>>,
}

impl WrapCache {
    /// Build a dirty (yet-to-be-computed) cache. Used both for the
    /// initial state and after any edit that invalidates wrap data.
    fn dirty() -> Self {
        Self {
            width: u16::MAX,
            lines: Vec::new(),
        }
    }
}

/// Behavior of `move_cursor_to_beginning_of_line` when the cursor is
/// already at BOL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BolBehavior {
    /// Stay at BOL (one shot — `Home` semantics).
    StayPut,
    /// Jump to BOL of the previous logical line (readline / emacs convention).
    WrapUp,
}

/// Behavior of `move_cursor_to_end_of_line` when the cursor is already at
/// EOL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EolBehavior {
    /// Stay at EOL.
    StayPut,
    /// Jump to EOL of the next logical line.
    WrapDown,
}

/// Snapshot of editable state for the undo stack. Opaque to callers —
/// captured via [`TextArea::snapshot`] and consumed by
/// [`TextArea::commit_undo`].
#[derive(Debug, Clone)]
pub struct UndoSnapshot {
    text: String,
    cursor: usize,
}

impl UndoSnapshot {
    /// The text content this snapshot will restore. Used by the vim
    /// wiring layer to detect whether the dispatched key actually mutated
    /// the buffer (cheap pointer-aware string comparison).
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
}

/// Maximum size of the undo stack. Keeps memory bounded for very long
/// editing sessions; vim's default is 1000 — composer use justifies less.
const UNDO_STACK_CAP: usize = 64;

/// Editable text with byte-offset cursor and a single-entry kill buffer.
#[derive(Debug)]
pub struct TextArea {
    text: String,
    /// Byte offset; always at a UTF-8 char boundary and `<= text.len()`.
    cursor_pos: usize,
    /// Lazily-recomputed wrapped-line ranges (cleared on every edit).
    wrap_cache: RefCell<WrapCache>,
    /// Remembered display column for vertical movement (`up` / `down`).
    /// `None` after any non-vertical mutation.
    preferred_col: Option<usize>,
    /// Last killed text. Preserved across `set_text` / `take_text` so
    /// post-submit yank still works.
    kill_buffer: String,
    /// `true` immediately after a kill operation. Consecutive kills append
    /// to `kill_buffer` (readline / emacs parity); any non-kill mutation
    /// resets the flag so a subsequent kill starts a fresh buffer.
    last_op_was_kill: bool,
    /// Bounded undo stack of (text, cursor) snapshots. Callers (e.g. the
    /// vim wiring layer) push before each text-changing command; `undo()`
    /// restores the last snapshot.
    undo_stack: Vec<UndoSnapshot>,
}

impl TextArea {
    pub fn new() -> Self {
        Self {
            text: String::new(),
            cursor_pos: 0,
            wrap_cache: RefCell::new(WrapCache::dirty()),
            preferred_col: None,
            kill_buffer: String::new(),
            last_op_was_kill: false,
            undo_stack: Vec::new(),
        }
    }

    // ──────────────────────────── Undo ───────────────────────────────

    /// Capture the current `(text, cursor)` for later restoration. Returns
    /// a snapshot that the caller can either commit to the stack (via
    /// [`commit_undo`]) after a successful mutation or discard otherwise.
    #[must_use]
    pub fn snapshot(&self) -> UndoSnapshot {
        UndoSnapshot {
            text: self.text.clone(),
            cursor: self.cursor_pos,
        }
    }

    /// Commit a previously-captured snapshot onto the undo stack. Drops
    /// duplicates (consecutive snapshots with the same text and cursor)
    /// and bounds the stack to `UNDO_STACK_CAP`.
    pub fn commit_undo(&mut self, snap: UndoSnapshot) {
        if let Some(top) = self.undo_stack.last()
            && top.text == snap.text
            && top.cursor == snap.cursor
        {
            return;
        }
        self.undo_stack.push(snap);
        if self.undo_stack.len() > UNDO_STACK_CAP {
            self.undo_stack.remove(0);
        }
    }

    /// Restore the most recent committed snapshot. Returns `true` if one
    /// was applied. No-op when the stack is empty.
    pub fn undo(&mut self) -> bool {
        let Some(snap) = self.undo_stack.pop() else {
            return false;
        };
        self.text = snap.text;
        self.cursor_pos = self.clamp_pos_to_char_boundary(snap.cursor.min(self.text.len()));
        self.wrap_cache.replace(WrapCache::dirty());
        self.preferred_col = None;
        self.last_op_was_kill = false;
        true
    }

    // ─────────────────────────── Raw access ──────────────────────────

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Byte offset of the cursor. Always at a UTF-8 char boundary.
    pub fn cursor(&self) -> usize {
        self.cursor_pos
    }

    pub fn set_cursor(&mut self, pos: usize) {
        self.cursor_pos = self.clamp_pos_to_char_boundary(pos.min(self.text.len()));
        self.preferred_col = None;
        self.last_op_was_kill = false;
    }

    /// Replace the entire visible buffer. Clamps the cursor into the new
    /// range. Intentionally preserves `kill_buffer` so a yank after
    /// submit/clear can still recover the user's most recent kill.
    pub fn set_text(&mut self, text: &str) {
        self.text.clear();
        self.text.push_str(text);
        self.cursor_pos = self.cursor_pos.min(self.text.len());
        self.cursor_pos = self.clamp_pos_to_char_boundary(self.cursor_pos);
        self.wrap_cache.replace(WrapCache::dirty());
        self.preferred_col = None;
        self.last_op_was_kill = false;
    }

    /// Replace the entire buffer and return the previous contents. Resets
    /// the cursor to the start of the (now-empty if `replacement` is empty)
    /// buffer. Like `set_text`, preserves the kill buffer.
    pub fn take_text(&mut self) -> String {
        let prev = std::mem::take(&mut self.text);
        self.cursor_pos = 0;
        self.wrap_cache.replace(WrapCache::dirty());
        self.preferred_col = None;
        self.last_op_was_kill = false;
        prev
    }

    // ─────────────────────────── Insertion ───────────────────────────

    pub fn insert_str(&mut self, s: &str) {
        self.insert_str_at(self.cursor_pos, s);
    }

    pub fn insert_str_at(&mut self, pos: usize, s: &str) {
        if s.is_empty() {
            return;
        }
        let pos = self.clamp_pos_to_char_boundary(pos.min(self.text.len()));
        self.text.insert_str(pos, s);
        self.wrap_cache.replace(WrapCache::dirty());
        if pos <= self.cursor_pos {
            self.cursor_pos += s.len();
        }
        self.preferred_col = None;
        self.last_op_was_kill = false;
    }

    pub fn replace_range(&mut self, range: Range<usize>, s: &str) {
        let start = self.clamp_pos_to_char_boundary(range.start.min(self.text.len()));
        let end = self.clamp_pos_to_char_boundary(range.end.min(self.text.len()));
        if start > end {
            return;
        }
        let removed_len = end - start;
        let inserted_len = s.len();
        if removed_len == 0 && inserted_len == 0 {
            return;
        }
        let diff = inserted_len as isize - removed_len as isize;

        self.text.replace_range(start..end, s);
        self.wrap_cache.replace(WrapCache::dirty());
        self.preferred_col = None;
        self.last_op_was_kill = false;

        // Move cursor relative to the edit.
        self.cursor_pos = if self.cursor_pos < start {
            self.cursor_pos
        } else if self.cursor_pos <= end {
            start + inserted_len
        } else {
            ((self.cursor_pos as isize) + diff) as usize
        }
        .min(self.text.len());
        self.cursor_pos = self.clamp_pos_to_char_boundary(self.cursor_pos);
    }

    // ─────────────────────────── Deletion ────────────────────────────

    /// Delete `n` grapheme clusters before the cursor.
    pub fn delete_backward(&mut self, n: usize) {
        if n == 0 || self.cursor_pos == 0 {
            return;
        }
        let mut target = self.cursor_pos;
        for _ in 0..n {
            target = self.prev_atomic_boundary(target);
            if target == 0 {
                break;
            }
        }
        self.replace_range(target..self.cursor_pos, "");
    }

    /// Delete `n` grapheme clusters after the cursor.
    pub fn delete_forward(&mut self, n: usize) {
        if n == 0 || self.cursor_pos >= self.text.len() {
            return;
        }
        let mut target = self.cursor_pos;
        for _ in 0..n {
            target = self.next_atomic_boundary(target);
            if target >= self.text.len() {
                break;
            }
        }
        self.replace_range(self.cursor_pos..target, "");
    }

    /// Kill (cut → kill buffer) from the cursor back to the start of the
    /// previous word.
    pub fn delete_backward_word(&mut self) {
        let start = self.beginning_of_previous_word();
        if start < self.cursor_pos {
            self.kill_range(start..self.cursor_pos);
        }
    }

    /// Kill from the cursor forward through the end of the next word.
    pub fn delete_forward_word(&mut self) {
        let end = self.end_of_next_word();
        if end > self.cursor_pos {
            self.kill_range(self.cursor_pos..end);
        }
    }

    /// Kill from the cursor to the end of the current logical line.
    /// If already at EOL and there's a trailing newline, the newline is
    /// killed (matches readline's `kill-line`).
    pub fn kill_to_end_of_line(&mut self) {
        let eol = self.end_of_current_line();
        let range = if self.cursor_pos == eol {
            if eol < self.text.len() {
                Some(self.cursor_pos..eol + 1)
            } else {
                None
            }
        } else {
            Some(self.cursor_pos..eol)
        };
        if let Some(range) = range {
            self.kill_range(range);
        }
    }

    /// Kill from the start of the current logical line up to the cursor.
    pub fn kill_to_beginning_of_line(&mut self) {
        let bol = self.beginning_of_current_line();
        let range = if self.cursor_pos == bol {
            if bol > 0 { Some(bol - 1..bol) } else { None }
        } else {
            Some(bol..self.cursor_pos)
        };
        if let Some(range) = range {
            self.kill_range(range);
        }
    }

    /// Insert the most recently killed text at the cursor.
    pub fn yank(&mut self) {
        if self.kill_buffer.is_empty() {
            return;
        }
        let text = self.kill_buffer.clone();
        self.insert_str(&text);
    }

    fn kill_range(&mut self, range: Range<usize>) {
        let start = self.clamp_pos_to_char_boundary(range.start.min(self.text.len()));
        let end = self.clamp_pos_to_char_boundary(range.end.min(self.text.len()));
        if start >= end {
            return;
        }
        let removed = self.text[start..end].to_string();
        if removed.is_empty() {
            return;
        }
        // Capture the accumulation flag before `replace_range` resets it,
        // then re-mark this op as a kill afterwards.
        let appending = self.last_op_was_kill;
        self.replace_range(start..end, "");
        if appending {
            self.kill_buffer.push_str(&removed);
        } else {
            self.kill_buffer = removed;
        }
        self.last_op_was_kill = true;
    }

    // ─────────────────────────── Movement ────────────────────────────

    pub fn move_cursor_left(&mut self) {
        self.cursor_pos = self.prev_atomic_boundary(self.cursor_pos);
        self.preferred_col = None;
        self.last_op_was_kill = false;
    }

    pub fn move_cursor_right(&mut self) {
        self.cursor_pos = self.next_atomic_boundary(self.cursor_pos);
        self.preferred_col = None;
        self.last_op_was_kill = false;
    }

    pub fn move_cursor_up(&mut self) {
        self.last_op_was_kill = false;
        // Prefer wrapped-line navigation if we have a cache.
        let Some((target_col, prev_line)) = self.line_above_cursor() else {
            // Fall back to logical-line navigation.
            if let Some(prev_nl) = self.text[..self.cursor_pos].rfind('\n') {
                let target_col = self.acquire_preferred_col();
                let prev_line_start = self.text[..prev_nl].rfind('\n').map(|i| i + 1).unwrap_or(0);
                self.move_to_display_col_on_line(prev_line_start, prev_nl, target_col);
            } else {
                self.cursor_pos = 0;
                self.preferred_col = None;
            }
            return;
        };
        match prev_line {
            Some((line_start, line_end)) => {
                if self.preferred_col.is_none() {
                    self.preferred_col = Some(target_col);
                }
                self.move_to_display_col_on_line(line_start, line_end, target_col);
            }
            None => {
                self.cursor_pos = 0;
                self.preferred_col = None;
            }
        }
    }

    pub fn move_cursor_down(&mut self) {
        self.last_op_was_kill = false;
        let Some((target_col, next_line)) = self.line_below_cursor() else {
            // Fall back to logical-line navigation.
            let target_col = self.acquire_preferred_col();
            if let Some(next_nl) = self.text[self.cursor_pos..]
                .find('\n')
                .map(|i| i + self.cursor_pos)
            {
                let next_line_start = next_nl + 1;
                let next_line_end = self.text[next_line_start..]
                    .find('\n')
                    .map(|i| i + next_line_start)
                    .unwrap_or(self.text.len());
                self.move_to_display_col_on_line(next_line_start, next_line_end, target_col);
            } else {
                self.cursor_pos = self.text.len();
                self.preferred_col = None;
            }
            return;
        };
        match next_line {
            Some((line_start, line_end)) => {
                if self.preferred_col.is_none() {
                    self.preferred_col = Some(target_col);
                }
                self.move_to_display_col_on_line(line_start, line_end, target_col);
            }
            None => {
                self.cursor_pos = self.text.len();
                self.preferred_col = None;
            }
        }
    }

    /// `Home` semantics: move to the beginning of the current logical line.
    /// `BolBehavior::WrapUp` makes a second press (already at BOL) move to
    /// the previous logical line's BOL.
    pub fn move_cursor_to_beginning_of_line(&mut self, behavior: BolBehavior) {
        let bol = self.beginning_of_current_line();
        if behavior == BolBehavior::WrapUp && self.cursor_pos == bol {
            self.set_cursor(self.beginning_of_line(self.cursor_pos.saturating_sub(1)));
        } else {
            self.set_cursor(bol);
        }
        self.preferred_col = None;
    }

    /// `End` semantics, symmetric with `move_cursor_to_beginning_of_line`.
    pub fn move_cursor_to_end_of_line(&mut self, behavior: EolBehavior) {
        let eol = self.end_of_current_line();
        if behavior == EolBehavior::WrapDown && self.cursor_pos == eol {
            let next_pos = (self.cursor_pos.saturating_add(1)).min(self.text.len());
            self.set_cursor(self.end_of_line(next_pos));
        } else {
            self.set_cursor(eol);
        }
    }

    // ─────────────────────── Word boundaries ─────────────────────────

    /// Beginning of the previous word (emacs `Alt+B`).
    #[must_use]
    pub fn beginning_of_previous_word(&self) -> usize {
        let prefix = &self.text[..self.cursor_pos];
        let Some((first_non_ws_idx, ch)) = prefix
            .char_indices()
            .rev()
            .find(|&(_, ch)| !ch.is_whitespace())
        else {
            return 0;
        };
        let run_start = prefix[..first_non_ws_idx]
            .char_indices()
            .rev()
            .find(|&(_, ch)| ch.is_whitespace())
            .map_or(0, |(idx, ch)| idx + ch.len_utf8());
        let run_end = first_non_ws_idx + ch.len_utf8();
        let pieces = split_word_pieces(&prefix[run_start..run_end]);
        let mut pieces = pieces.into_iter().rev().peekable();
        let Some((piece_start, piece)) = pieces.next() else {
            return run_start;
        };
        let mut start = run_start + piece_start;

        if piece.chars().all(is_word_separator) {
            while let Some((idx, piece)) = pieces.peek() {
                if !piece.chars().all(is_word_separator) {
                    break;
                }
                start = run_start + *idx;
                pieces.next();
            }
        }
        start
    }

    /// End of the next word (emacs `Alt+F`).
    #[must_use]
    pub fn end_of_next_word(&self) -> usize {
        let suffix = &self.text[self.cursor_pos..];
        let Some(first_non_ws) = suffix.find(|ch: char| !ch.is_whitespace()) else {
            return self.text.len();
        };
        let run = &suffix[first_non_ws..];
        let run = &run[..run.find(char::is_whitespace).unwrap_or(run.len())];
        let mut pieces = split_word_pieces(run).into_iter().peekable();
        let Some((start, piece)) = pieces.next() else {
            return self.cursor_pos + first_non_ws;
        };
        let word_start = self.cursor_pos + first_non_ws + start;
        let mut end = word_start + piece.len();
        if piece.chars().all(is_word_separator) {
            while let Some((idx, piece)) = pieces.peek() {
                if !piece.chars().all(is_word_separator) {
                    break;
                }
                end = self.cursor_pos + first_non_ws + *idx + piece.len();
                pieces.next();
            }
        }
        end
    }

    /// Beginning of the next word (used by some readline configurations).
    #[must_use]
    pub fn beginning_of_next_word(&self) -> usize {
        let Some(first_non_ws) = self.text[self.cursor_pos..].find(|c: char| !c.is_whitespace())
        else {
            return self.text.len();
        };
        let word_start = self.cursor_pos + first_non_ws;
        if word_start != self.cursor_pos {
            return word_start;
        }
        let end = self.end_of_next_word();
        if end >= self.text.len() {
            return self.text.len();
        }
        let Some(next_non_ws) = self.text[end..].find(|c: char| !c.is_whitespace()) else {
            return self.text.len();
        };
        end + next_non_ws
    }

    // ─────────────────────── Line boundaries ─────────────────────────

    #[must_use]
    pub fn beginning_of_line(&self, pos: usize) -> usize {
        self.text[..pos.min(self.text.len())]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0)
    }

    #[must_use]
    pub fn beginning_of_current_line(&self) -> usize {
        self.beginning_of_line(self.cursor_pos)
    }

    #[must_use]
    pub fn end_of_line(&self, pos: usize) -> usize {
        self.text[pos.min(self.text.len())..]
            .find('\n')
            .map(|i| i + pos)
            .unwrap_or(self.text.len())
    }

    #[must_use]
    pub fn end_of_current_line(&self) -> usize {
        self.end_of_line(self.cursor_pos)
    }

    // ────────────────────────── Rendering ────────────────────────────

    /// Number of wrapped rows the textarea will need to render at `width`.
    pub fn desired_height(&self, width: u16) -> u16 {
        self.wrapped_lines(width).len().max(1) as u16
    }

    /// On-screen cursor position within `area`, assuming the textarea is
    /// rendered starting at `area.x, area.y` with no scrolling. Returns
    /// `None` only if the buffer has no wrapped lines (impossible for the
    /// empty buffer — that still returns `Some((area.x, area.y))`).
    pub fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
        let lines = self.wrapped_lines(area.width);
        if lines.is_empty() {
            return Some((area.x, area.y));
        }
        let idx =
            Self::wrapped_line_index_by_start(&lines, self.cursor_pos).unwrap_or(lines.len() - 1);
        let ls = &lines[idx];
        let col = UnicodeWidthStr::width(&self.text[ls.start..self.cursor_pos.min(self.text.len())])
            as u16;
        let screen_row = (idx as u16).min(area.height.saturating_sub(1));
        Some((area.x + col, area.y + screen_row))
    }

    /// Byte ranges (one per displayed wrapped line) for the buffer at the
    /// given width. Lazily cached; cleared by any edit. Width=0 falls
    /// back to a single line.
    pub fn wrapped_lines(&self, width: u16) -> Ref<'_, Vec<Range<usize>>> {
        if self.wrap_cache.borrow().width != width {
            let lines = compute_wrapped_lines(&self.text, width);
            *self.wrap_cache.borrow_mut() = WrapCache { width, lines };
        }
        Ref::map(self.wrap_cache.borrow(), |c| &c.lines)
    }

    // ───────────────────────── Internal helpers ──────────────────────

    /// Display column of the cursor relative to the start of its logical
    /// line. Used for vertical-movement column preservation.
    fn current_display_col(&self) -> usize {
        let bol = self.beginning_of_current_line();
        UnicodeWidthStr::width(&self.text[bol..self.cursor_pos])
    }

    fn acquire_preferred_col(&mut self) -> usize {
        match self.preferred_col {
            Some(c) => c,
            None => {
                let c = self.current_display_col();
                self.preferred_col = Some(c);
                c
            }
        }
    }

    /// Set cursor to the position on `[line_start, line_end)` whose
    /// display column is closest to (but not exceeding) `target_col`.
    fn move_to_display_col_on_line(
        &mut self,
        line_start: usize,
        line_end: usize,
        target_col: usize,
    ) {
        let line_start = self.clamp_pos_to_char_boundary(line_start.min(self.text.len()));
        let line_end = self.clamp_pos_to_char_boundary(line_end.min(self.text.len()));
        if line_start >= line_end {
            self.cursor_pos = line_start;
            return;
        }
        let mut width_so_far = 0usize;
        for (i, g) in self.text[line_start..line_end].grapheme_indices(true) {
            width_so_far += UnicodeWidthStr::width(g);
            if width_so_far > target_col {
                self.cursor_pos = line_start + i;
                return;
            }
        }
        self.cursor_pos = line_end;
    }

    /// Index into `lines` of the wrapped line that contains `pos`.
    fn wrapped_line_index_by_start(lines: &[Range<usize>], pos: usize) -> Option<usize> {
        let idx = lines.partition_point(|r| r.start <= pos);
        if idx == 0 { None } else { Some(idx - 1) }
    }

    /// Compute the target column + previous-line range for vertical-up.
    /// Returns `None` if no wrap cache exists yet — caller falls back to
    /// logical-line nav.
    fn line_above_cursor(&self) -> Option<(usize, Option<(usize, usize)>)> {
        let cache = self.wrap_cache.borrow();
        if cache.lines.is_empty() {
            return None;
        }
        let lines = &cache.lines;
        let idx = Self::wrapped_line_index_by_start(lines, self.cursor_pos)?;
        let cur = &lines[idx];
        let target_col = self
            .preferred_col
            .unwrap_or_else(|| UnicodeWidthStr::width(&self.text[cur.start..self.cursor_pos]));
        if idx == 0 {
            Some((target_col, None))
        } else {
            let prev = &lines[idx - 1];
            let line_start = prev.start;
            let line_end = prev.end.saturating_sub(1).max(prev.start);
            Some((target_col, Some((line_start, line_end))))
        }
    }

    fn line_below_cursor(&self) -> Option<(usize, Option<(usize, usize)>)> {
        let cache = self.wrap_cache.borrow();
        if cache.lines.is_empty() {
            return None;
        }
        let lines = &cache.lines;
        let idx = Self::wrapped_line_index_by_start(lines, self.cursor_pos)?;
        let cur = &lines[idx];
        let target_col = self
            .preferred_col
            .unwrap_or_else(|| UnicodeWidthStr::width(&self.text[cur.start..self.cursor_pos]));
        if idx + 1 >= lines.len() {
            Some((target_col, None))
        } else {
            let next = &lines[idx + 1];
            let line_start = next.start;
            let line_end = next.end.saturating_sub(1).max(next.start);
            Some((target_col, Some((line_start, line_end))))
        }
    }

    /// Walk back one grapheme cluster from `pos`. Returns the new byte
    /// offset (always at a UTF-8 char boundary).
    fn prev_atomic_boundary(&self, pos: usize) -> usize {
        if pos == 0 {
            return 0;
        }
        let mut gc = GraphemeCursor::new(pos, self.text.len(), false);
        match gc.prev_boundary(&self.text, 0) {
            Ok(Some(b)) => b,
            Ok(None) => 0,
            Err(_) => pos.saturating_sub(1),
        }
    }

    /// Walk forward one grapheme cluster from `pos`.
    fn next_atomic_boundary(&self, pos: usize) -> usize {
        if pos >= self.text.len() {
            return self.text.len();
        }
        let mut gc = GraphemeCursor::new(pos, self.text.len(), false);
        match gc.next_boundary(&self.text, 0) {
            Ok(Some(b)) => b,
            Ok(None) => self.text.len(),
            Err(_) => pos.saturating_add(1),
        }
    }

    /// Snap `pos` to the nearest UTF-8 char boundary in the buffer.
    fn clamp_pos_to_char_boundary(&self, pos: usize) -> usize {
        let pos = pos.min(self.text.len());
        if self.text.is_char_boundary(pos) {
            return pos;
        }
        let mut prev = pos;
        while prev > 0 && !self.text.is_char_boundary(prev) {
            prev -= 1;
        }
        let mut next = pos;
        while next < self.text.len() && !self.text.is_char_boundary(next) {
            next += 1;
        }
        if pos.saturating_sub(prev) <= next.saturating_sub(pos) {
            prev
        } else {
            next
        }
    }
}

impl Default for TextArea {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute wrapped-line byte ranges for `text` at `width` display columns.
///
/// - Logical lines (delimited by `\n`) are processed independently. Each
///   wrapped range's `end` points just past the line's last byte (i.e.
///   the newline itself, if present, is NOT included — `partition_point`
///   logic in cursor positioning relies on this).
/// - Wrapping is grapheme-aware via `unicode-segmentation` and
///   display-column-aware via `unicode-width`. CJK fullwidth characters
///   correctly take 2 columns.
/// - `width == 0` degenerates to one range per logical line.
/// - An empty buffer returns a single `0..0` range so the cursor still
///   has a valid line to land on.
fn compute_wrapped_lines(text: &str, width: u16) -> Vec<Range<usize>> {
    if text.is_empty() {
        // `vec![0..0]` trips the `single_range_in_vec_init` lint (which
        // would prefer a value range or `vec![0; 0]` — both wrong here).
        // Use iter-once so we get exactly one `Range<usize>` element.
        return std::iter::once(0..0).collect();
    }
    let mut lines = Vec::new();
    let mut logical_start = 0usize;

    while logical_start <= text.len() {
        let logical_end = text[logical_start..]
            .find('\n')
            .map(|i| logical_start + i)
            .unwrap_or(text.len());
        wrap_logical_line(text, logical_start, logical_end, width, &mut lines);
        if logical_end == text.len() {
            break;
        }
        // Skip the '\n' itself; if the input ends in '\n' add a trailing
        // empty wrapped line so the cursor can land past the final newline.
        logical_start = logical_end + 1;
        if logical_start == text.len() {
            lines.push(text.len()..text.len());
            break;
        }
    }

    if lines.is_empty() {
        lines.push(0..0);
    }
    lines
}

fn wrap_logical_line(
    text: &str,
    start: usize,
    end: usize,
    width: u16,
    out: &mut Vec<Range<usize>>,
) {
    if start == end {
        out.push(start..end);
        return;
    }
    if width == 0 {
        out.push(start..end);
        return;
    }
    let slice = &text[start..end];
    if UnicodeWidthStr::width(slice) <= width as usize {
        out.push(start..end);
        return;
    }
    let limit = width as usize;
    let mut col = 0usize;
    let mut chunk_start = 0usize;
    for (idx, g) in slice.grapheme_indices(true) {
        let gw = UnicodeWidthStr::width(g);
        if col + gw > limit && idx > chunk_start {
            out.push(start + chunk_start..start + idx);
            chunk_start = idx;
            col = gw;
        } else {
            col += gw;
        }
    }
    if chunk_start < slice.len() {
        out.push(start + chunk_start..end);
    }
}

#[cfg(test)]
#[path = "textarea.test.rs"]
mod tests;
