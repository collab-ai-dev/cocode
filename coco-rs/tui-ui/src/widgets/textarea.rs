//! Editable text buffer for the chat composer.
//!
//! Owns byte-offset cursor + multi-line wrapped rendering + grapheme-aware
//! navigation + a single-entry kill buffer. Ported as a leaner subset of
//! `codex-rs/tui/src/bottom_pane/textarea.rs` (3418 LOC → ~650 LOC) with
//! these deliberate omissions vs upstream:
//!
//! - Modal Vim state (`coco-vim` owns vim semantics via `vim/wiring.rs`).
//! - `EditorKeymap` / `VimNormalKeymap` dispatch (`coco-tui`'s
//! `keybinding_bridge` produces `TuiCommand`s; TextArea only exposes
//! verbs).
//! - `pub fn input(&mut self, event: KeyEvent)` — never call; the bridge
//! owns key→verb mapping.
//! - `StatefulWidgetRef` / viewport scroll (the single-line composer
//! doesn't need it yet; multi-line callers can read `wrapped_lines`
//! and render themselves).
//!
//! The single-entry kill buffer:
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

use super::textarea_elements::ElementDisplay;
use super::textarea_elements::ElementId;
use super::textarea_elements::ElementKind;
use super::textarea_elements::ProjectedTextElement;
use super::textarea_elements::TextAreaSnapshot;
use super::textarea_elements::TextElement;
use super::textarea_elements::TextProjection;
use super::textarea_layout::WrapAtom;
use super::textarea_layout::compute_wrapped_lines;

/// Validation failures for atomic textarea elements and snapshots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElementError {
    EmptySource,
    MultilineSource,
    MultilineDisplay,
    InvalidRange,
    OverlappingRange,
    DuplicateId,
    InvalidCursor,
    IdExhausted,
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

/// Snapshot of editable state for the undo/redo stacks. Internal: mutating
/// verbs checkpoint themselves, so callers never build or hold one.
#[derive(Debug, Clone)]
struct UndoSnapshot {
    text: String,
    cursor: usize,
    elements: Vec<TextElement>,
}

/// Maximum size of the undo stack. Keeps memory bounded for very long
/// editing sessions; vim's default is 1000 — composer use justifies less.
const UNDO_STACK_CAP: usize = 64;

/// What kind of edit a mutation is, for undo-entry batching.
///
/// Typing should undo a word at a time, not a keystroke at a time, so a run of
/// same-kind single-grapheme edits collapses into one entry. Anything coarser —
/// a paste, a word kill, a programmatic splice — is its own entry: the user
/// performed one action and expects one undo to reverse it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MutationKind {
    /// One grapheme typed at the cursor.
    InsertChar,
    /// A multi-grapheme or off-cursor insertion — paste, yank, splice.
    InsertBlock,
    /// Backspace.
    DeleteBackward,
    /// Forward delete.
    DeleteForward,
    /// A word- or line-granularity kill.
    Kill,
    /// An arbitrary range replacement.
    Replace,
}

impl MutationKind {
    /// Whether a contiguous run of this kind collapses into one undo entry.
    /// The coarse kinds do not: each already corresponds to one user action.
    fn coalesces(self) -> bool {
        match self {
            Self::InsertChar | Self::DeleteBackward | Self::DeleteForward => true,
            Self::InsertBlock | Self::Kill | Self::Replace => false,
        }
    }
}

/// The previous mutation, for deciding whether the next one continues its run.
#[derive(Debug, Clone, Copy)]
struct LastMutation {
    kind: MutationKind,
    /// Where the mutation left the cursor. A run is contiguous only when the
    /// next edit starts exactly here, so moving the cursor away and editing
    /// elsewhere starts a fresh entry. (A round-trip that lands back on the
    /// same offset does *not* break the run — deliberately: it is
    /// indistinguishable from never having moved, and undo still fully
    /// reverses either way.)
    cursor: usize,
}

/// Editable text with byte-offset cursor and a single-entry kill buffer.
#[derive(Debug)]
pub struct TextArea {
    text: String,
    /// Byte offset; always at a UTF-8 char boundary and `<= text.len()`.
    cursor_pos: usize,
    /// Atomic regions sorted by source-buffer position.
    elements: Vec<TextElement>,
    /// Monotonic allocator; IDs are never reused, including across undo.
    next_element_id: i64,
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
    /// Bounded undo stack of (text, cursor) snapshots. Every mutating verb
    /// self-checkpoints through [`TextArea::pre_mutate`]; `undo()` restores
    /// the last snapshot.
    undo_stack: Vec<UndoSnapshot>,
    /// Snapshots popped by `undo`, restorable by `redo`. Any fresh mutation
    /// clears it — once the buffer moves somewhere new, the redo branch is no
    /// longer reachable and offering it would resurrect unrelated text.
    redo_stack: Vec<UndoSnapshot>,
    /// The previous mutation, for undo-entry batching. `None` breaks the run,
    /// so the next edit starts a fresh entry.
    last_mutation: Option<LastMutation>,
    /// Nesting depth of [`TextArea::undo_group`]. Non-zero suppresses per-verb
    /// checkpoints so a compound edit commits exactly one entry.
    group_depth: usize,
}

impl TextArea {
    pub fn new() -> Self {
        Self {
            text: String::new(),
            cursor_pos: 0,
            elements: Vec::new(),
            next_element_id: 1,
            wrap_cache: RefCell::new(WrapCache::dirty()),
            preferred_col: None,
            kill_buffer: String::new(),
            last_op_was_kill: false,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_mutation: None,
            group_depth: 0,
        }
    }

    // ──────────────────────────── Undo ───────────────────────────────

    /// Capture the current `(text, cursor)`.
    fn undo_snapshot(&self) -> UndoSnapshot {
        UndoSnapshot {
            text: self.text.clone(),
            cursor: self.cursor_pos,
            elements: self.elements.clone(),
        }
    }

    /// Push onto the undo stack, dropping consecutive duplicates and bounding
    /// the stack to `UNDO_STACK_CAP`.
    fn push_undo(&mut self, snap: UndoSnapshot) {
        if let Some(top) = self.undo_stack.last()
            && top.text == snap.text
            && top.cursor == snap.cursor
            && top.elements == snap.elements
        {
            return;
        }
        self.undo_stack.push(snap);
        if self.undo_stack.len() > UNDO_STACK_CAP {
            self.undo_stack.remove(0);
        }
    }

    /// Checkpoint before a mutation, unless it continues an existing run or an
    /// [`undo_group`](Self::undo_group) owns the checkpointing.
    ///
    /// `at` is the cursor position BEFORE the edit: a run is contiguous only
    /// when this edit starts exactly where the previous one left the cursor.
    fn pre_mutate(&mut self, kind: MutationKind, at: usize) {
        // A fresh edit invalidates the redo branch: the state redo would return
        // to is no longer reachable from here.
        self.redo_stack.clear();
        if self.group_depth > 0 {
            return;
        }
        let continues_run = self
            .last_mutation
            .is_some_and(|last| last.kind == kind && kind.coalesces() && last.cursor == at);
        if continues_run {
            return;
        }
        let snap = self.undo_snapshot();
        self.push_undo(snap);
    }

    /// Record what a mutation was, so the next one can tell whether it
    /// continues the run. `ends_run` breaks the batch even on a matching kind —
    /// used at a typed word boundary so undo steps land on words.
    fn note_mutation(&mut self, kind: MutationKind, ends_run: bool) {
        self.last_mutation = (!ends_run).then_some(LastMutation {
            kind,
            cursor: self.cursor_pos,
        });
    }

    /// Run `edit` as ONE atomic undo step: every mutation inside collapses into
    /// a single entry, and a group that changes nothing leaves none behind.
    ///
    /// This is the seam for compound edits whose intermediate states the user
    /// never saw and should never land on — a vim operator, a chip expansion.
    /// Nesting is safe; only the outermost group commits.
    pub fn undo_group<R>(&mut self, edit: impl FnOnce(&mut Self) -> R) -> R {
        let before = self.undo_snapshot();
        self.group_depth = self.group_depth.saturating_add(1);
        // Restore the depth even if `edit` panics, so a recovered panic cannot
        // leave `group_depth` stuck above zero and silently suppress every
        // future checkpoint. `AssertUnwindSafe` is sound here: on the unwind
        // path the only state touched is `group_depth` (decremented) before
        // re-raising — no half-updated buffer is ever observed.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| edit(self)));
        self.group_depth = self.group_depth.saturating_sub(1);
        let out = match result {
            Ok(out) => out,
            Err(payload) => std::panic::resume_unwind(payload),
        };
        if self.group_depth == 0 && (self.text != before.text || self.elements != before.elements) {
            self.push_undo(before);
            // A group is one deliberate action; never let the next keystroke
            // batch into it.
            self.last_mutation = None;
        }
        out
    }

    fn restore(&mut self, snap: UndoSnapshot) {
        self.text = snap.text;
        self.elements = snap.elements;
        self.cursor_pos = self.clamp_pos_to_char_boundary(snap.cursor.min(self.text.len()));
        self.wrap_cache.replace(WrapCache::dirty());
        self.preferred_col = None;
        self.last_op_was_kill = false;
        // Undo/redo is not itself an edit run.
        self.last_mutation = None;
    }

    /// Restore the most recent undo entry, making the current state redoable.
    /// Returns `true` if one was applied.
    pub fn undo(&mut self) -> bool {
        let Some(snap) = self.undo_stack.pop() else {
            return false;
        };
        let current = self.undo_snapshot();
        self.restore(snap);
        self.redo_stack.push(current);
        true
    }

    /// Re-apply the most recently undone entry. Returns `true` if one was
    /// applied. The redo stack is cleared by any fresh mutation, so this only
    /// ever replays states the user actually undid.
    pub fn redo(&mut self) -> bool {
        let Some(snap) = self.redo_stack.pop() else {
            return false;
        };
        let current = self.undo_snapshot();
        self.restore(snap);
        self.push_undo(current);
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
        let pos = self.clamp_pos_to_char_boundary(pos.min(self.text.len()));
        self.cursor_pos = self.clamp_pos_to_element_boundary(pos);
        self.preferred_col = None;
        self.last_op_was_kill = false;
    }

    /// Replace the entire visible buffer. Clamps the cursor into the new
    /// range. Intentionally preserves `kill_buffer` so a yank after
    /// submit/clear can still recover the user's most recent kill.
    pub fn set_text(&mut self, text: &str) {
        self.text.clear();
        self.text.push_str(text);
        self.elements.clear();
        self.cursor_pos = self.cursor_pos.min(self.text.len());
        self.cursor_pos = self.clamp_pos_to_char_boundary(self.cursor_pos);
        self.wrap_cache.replace(WrapCache::dirty());
        self.preferred_col = None;
        self.last_op_was_kill = false;
        self.reset_edit_history();
    }

    /// Drop the undo/redo history at a wholesale buffer swap.
    ///
    /// `set_text` / `take_text` load a *different* buffer (history recall,
    /// reverse-search preview, stash restore, submit) rather than edit the
    /// current one, so they are an edit-history boundary. Leaving the stacks
    /// intact lets `redo` resurrect text from the prior buffer and lets `undo`
    /// cross the swap — both violate the redo-stack invariant. They bypass
    /// [`pre_mutate`] (the only other place the stacks are cleared), so the
    /// reset is explicit here.
    fn reset_edit_history(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.last_mutation = None;
    }

    /// Replace the entire buffer and return the previous contents. Resets
    /// the cursor to the start of the (now-empty if `replacement` is empty)
    /// buffer. Like `set_text`, preserves the kill buffer.
    pub fn take_text(&mut self) -> String {
        let prev = std::mem::take(&mut self.text);
        self.elements.clear();
        self.cursor_pos = 0;
        self.wrap_cache.replace(WrapCache::dirty());
        self.preferred_col = None;
        self.last_op_was_kill = false;
        self.reset_edit_history();
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
        let pos = self.clamp_insertion_pos(pos);
        // Typing is one grapheme landing at the cursor; anything else (a paste,
        // a yank, an off-cursor splice) is one deliberate action and gets its
        // own undo entry.
        let single_grapheme = s.graphemes(true).count() == 1;
        let kind = if pos == self.cursor_pos && single_grapheme {
            MutationKind::InsertChar
        } else {
            MutationKind::InsertBlock
        };
        self.pre_mutate(kind, self.cursor_pos);
        self.text.insert_str(pos, s);
        self.shift_elements_after_edit(pos, pos, s.len());
        self.wrap_cache.replace(WrapCache::dirty());
        if pos <= self.cursor_pos {
            self.cursor_pos += s.len();
        }
        self.preferred_col = None;
        self.last_op_was_kill = false;
        // Whitespace closes the run, so undo steps land on word boundaries:
        // typing "hello world" undoes to "hello ", then to "".
        let ends_run = s.chars().all(char::is_whitespace);
        self.note_mutation(kind, ends_run);
    }

    pub fn replace_range(&mut self, range: Range<usize>, s: &str) {
        self.replace_range_as(range, s, MutationKind::Replace);
    }

    /// Canonical range a replacement will affect after expanding across
    /// indivisible elements and snapping to UTF-8 boundaries.
    #[must_use]
    pub fn expanded_edit_range(&self, range: Range<usize>) -> Range<usize> {
        let raw_start = range.start.min(self.text.len());
        let raw_end = range.end.min(self.text.len());
        let range = if raw_start == raw_end {
            let pos = self.clamp_insertion_pos(raw_start);
            pos..pos
        } else {
            self.expand_range_to_elements(raw_start.min(raw_end)..raw_start.max(raw_end))
        };
        let start = self.clamp_pos_to_char_boundary(range.start);
        let end = self.clamp_pos_to_char_boundary(range.end);
        start..end
    }

    /// Whether a non-empty edit range intersects an indivisible element.
    ///
    /// Callers with plain-text-only registers (notably Vim) use this to reject
    /// operations that could delete or yank an attachment without also being
    /// able to retain its app-owned payload.
    #[must_use]
    pub fn range_overlaps_element(&self, range: Range<usize>) -> bool {
        let range = self.expanded_edit_range(range);
        range.start < range.end
            && self
                .elements
                .iter()
                .any(|element| ranges_overlap(&element.range, &range))
    }

    /// `replace_range` with an explicit mutation kind, so the deletion verbs
    /// built on it batch as themselves rather than as opaque replacements.
    fn replace_range_as(&mut self, range: Range<usize>, s: &str, kind: MutationKind) {
        let range = self.expanded_edit_range(range);
        let start = range.start;
        let end = range.end;
        if start > end {
            return;
        }
        let removed_len = end - start;
        let inserted_len = s.len();
        if removed_len == 0 && inserted_len == 0 {
            return;
        }
        let diff = inserted_len as isize - removed_len as isize;

        self.pre_mutate(kind, self.cursor_pos);
        self.text.replace_range(start..end, s);
        self.shift_elements_after_edit(start, end, inserted_len);
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
        self.note_mutation(kind, /*ends_run*/ false);
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
        self.replace_range_as(target..self.cursor_pos, "", MutationKind::DeleteBackward);
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
        self.replace_range_as(self.cursor_pos..target, "", MutationKind::DeleteForward);
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
        let range = self.expanded_edit_range(range);
        let start = range.start;
        let end = range.end;
        if start >= end {
            return;
        }
        // The kill ring is deliberately plain text. Deleting an app-owned
        // element into it would leave only its display token, so a later yank
        // could resurrect an inert lookalike without the attachment payload.
        if self.range_overlaps_element(start..end) {
            return;
        }
        let removed = self.text[start..end].to_string();
        if removed.is_empty() {
            return;
        }
        // Capture the accumulation flag before `replace_range` resets it,
        // then re-mark this op as a kill afterwards.
        let appending = self.last_op_was_kill;
        self.replace_range_as(start..end, "", MutationKind::Kill);
        if appending {
            self.kill_buffer.push_str(&removed);
        } else {
            self.kill_buffer = removed;
        }
        self.last_op_was_kill = true;
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
        let col = UnicodeWidthStr::width(
            self.display_projection_with_width(
                ls.start..self.cursor_pos.min(self.text.len()),
                area.width,
            )
            .text
            .as_str(),
        ) as u16;
        let screen_row = (idx as u16).min(area.height.saturating_sub(1));
        Some((area.x + col, area.y + screen_row))
    }

    /// Byte ranges (one per displayed wrapped line) for the buffer at the
    /// given width. Lazily cached; cleared by any edit. Width=0 falls
    /// back to a single line.
    pub fn wrapped_lines(&self, width: u16) -> Ref<'_, Vec<Range<usize>>> {
        if self.wrap_cache.borrow().width != width {
            let atoms = self
                .elements
                .iter()
                .map(|element| WrapAtom {
                    range: element.range.clone(),
                    display_width: element.display_width().min(usize::from(width.max(1))),
                })
                .collect::<Vec<_>>();
            let lines = compute_wrapped_lines(&self.text, width, &atoms);
            *self.wrap_cache.borrow_mut() = WrapCache { width, lines };
        }
        Ref::map(self.wrap_cache.borrow(), |c| &c.lines)
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

    fn clamp_pos_to_element_boundary(&self, pos: usize) -> usize {
        let Some(element) = self
            .elements
            .iter()
            .find(|element| pos > element.range.start && pos < element.range.end)
        else {
            return pos;
        };
        let to_start = pos - element.range.start;
        let to_end = element.range.end - pos;
        if to_start <= to_end {
            element.range.start
        } else {
            element.range.end
        }
    }

    fn clamp_insertion_pos(&self, pos: usize) -> usize {
        let pos = self.clamp_pos_to_char_boundary(pos.min(self.text.len()));
        self.clamp_pos_to_element_boundary(pos)
    }

    fn element_boundary_for_word_motion(&self, pos: usize, toward_start: bool) -> usize {
        let Some(element) = self
            .elements
            .iter()
            .find(|element| pos > element.range.start && pos < element.range.end)
        else {
            return pos;
        };
        if toward_start {
            element.range.start
        } else {
            element.range.end
        }
    }

    fn expand_range_to_elements(&self, range: Range<usize>) -> Range<usize> {
        let mut range = range.start.min(self.text.len())..range.end.min(self.text.len());
        loop {
            let mut changed = false;
            for element in &self.elements {
                if ranges_overlap(&element.range, &range) {
                    let start = range.start.min(element.range.start);
                    let end = range.end.max(element.range.end);
                    changed |= start != range.start || end != range.end;
                    range = start..end;
                }
            }
            if !changed {
                return range;
            }
        }
    }

    fn shift_elements_after_edit(&mut self, start: usize, end: usize, inserted_len: usize) {
        let removed_len = end.saturating_sub(start);
        let delta = inserted_len as isize - removed_len as isize;
        self.elements
            .retain(|element| !ranges_overlap(&element.range, &(start..end)));
        for element in &mut self.elements {
            if element.range.start >= end {
                element.range.start = element.range.start.saturating_add_signed(delta);
                element.range.end = element.range.end.saturating_add_signed(delta);
            }
        }
    }
}

impl Default for TextArea {
    fn default() -> Self {
        Self::new()
    }
}

fn ranges_overlap(left: &Range<usize>, right: &Range<usize>) -> bool {
    left.start < right.end && left.end > right.start
}

#[cfg(test)]
#[path = "textarea.test.rs"]
mod tests;

#[path = "textarea_atomic.rs"]
mod atomic;

#[path = "textarea_navigation.rs"]
mod navigation;
