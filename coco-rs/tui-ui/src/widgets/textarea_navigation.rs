//! Grapheme-, element-, and display-width-aware textarea navigation.

use super::*;

const WORD_SEPARATORS: &str = "`~!@#$%^&*()-=+[{]}\\|;:'\",.<>/?";

struct VerticalNavigationTarget {
    column: usize,
    width: u16,
    adjacent_line: Option<(usize, usize)>,
}

fn is_word_separator(ch: char) -> bool {
    WORD_SEPARATORS.contains(ch)
}

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

impl TextArea {
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
        let Some(target) = self.line_above_cursor() else {
            if let Some(prev_nl) = self.text[..self.cursor_pos].rfind('\n') {
                let target_col = self.acquire_preferred_col();
                let prev_line_start = self.text[..prev_nl].rfind('\n').map(|i| i + 1).unwrap_or(0);
                self.move_to_display_col_on_line(prev_line_start, prev_nl, target_col, u16::MAX);
            } else {
                self.cursor_pos = 0;
                self.preferred_col = None;
            }
            return;
        };
        match target.adjacent_line {
            Some((line_start, line_end)) => {
                if self.preferred_col.is_none() {
                    self.preferred_col = Some(target.column);
                }
                self.move_to_display_col_on_line(line_start, line_end, target.column, target.width);
            }
            None => {
                self.cursor_pos = 0;
                self.preferred_col = None;
            }
        }
    }

    pub fn move_cursor_up_at_width(&mut self, width: u16) {
        drop(self.wrapped_lines(width));
        self.move_cursor_up();
    }

    pub fn move_cursor_down(&mut self) {
        self.last_op_was_kill = false;
        let Some(target) = self.line_below_cursor() else {
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
                self.move_to_display_col_on_line(
                    next_line_start,
                    next_line_end,
                    target_col,
                    u16::MAX,
                );
            } else {
                self.cursor_pos = self.text.len();
                self.preferred_col = None;
            }
            return;
        };
        match target.adjacent_line {
            Some((line_start, line_end)) => {
                if self.preferred_col.is_none() {
                    self.preferred_col = Some(target.column);
                }
                self.move_to_display_col_on_line(line_start, line_end, target.column, target.width);
            }
            None => {
                self.cursor_pos = self.text.len();
                self.preferred_col = None;
            }
        }
    }

    pub fn move_cursor_down_at_width(&mut self, width: u16) {
        drop(self.wrapped_lines(width));
        self.move_cursor_down();
    }

    pub fn move_cursor_to_beginning_of_line(&mut self, behavior: BolBehavior) {
        let bol = self.beginning_of_current_line();
        if behavior == BolBehavior::WrapUp && self.cursor_pos == bol {
            self.set_cursor(self.beginning_of_line(self.cursor_pos.saturating_sub(1)));
        } else {
            self.set_cursor(bol);
        }
        self.preferred_col = None;
    }

    pub fn move_cursor_to_end_of_line(&mut self, behavior: EolBehavior) {
        let eol = self.end_of_current_line();
        if behavior == EolBehavior::WrapDown && self.cursor_pos == eol {
            let next_pos = (self.cursor_pos.saturating_add(1)).min(self.text.len());
            self.set_cursor(self.end_of_line(next_pos));
        } else {
            self.set_cursor(eol);
        }
    }

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
        let mut pieces = split_word_pieces(&prefix[run_start..run_end])
            .into_iter()
            .rev()
            .peekable();
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
        self.element_boundary_for_word_motion(start, true)
    }

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
        self.element_boundary_for_word_motion(end, false)
    }

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
        self.element_boundary_for_word_motion(end + next_non_ws, false)
    }

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

    fn current_display_col(&self) -> usize {
        let bol = self.beginning_of_current_line();
        UnicodeWidthStr::width(
            self.display_projection_with_width(bol..self.cursor_pos, u16::MAX)
                .text
                .as_str(),
        )
    }

    fn acquire_preferred_col(&mut self) -> usize {
        match self.preferred_col {
            Some(column) => column,
            None => {
                let column = self.current_display_col();
                self.preferred_col = Some(column);
                column
            }
        }
    }

    fn move_to_display_col_on_line(
        &mut self,
        line_start: usize,
        line_end: usize,
        target_col: usize,
        width: u16,
    ) {
        let line_start = self.clamp_pos_to_char_boundary(line_start.min(self.text.len()));
        let line_end = self.clamp_pos_to_char_boundary(line_end.min(self.text.len()));
        if line_start >= line_end {
            self.cursor_pos = line_start;
            return;
        }
        let mut width_so_far = 0usize;
        let mut pos = line_start;
        while pos < line_end {
            let next = self.next_atomic_boundary(pos).min(line_end);
            let unit_width = UnicodeWidthStr::width(
                self.display_projection_with_width(pos..next, width)
                    .text
                    .as_str(),
            );
            if width_so_far + unit_width > target_col {
                self.cursor_pos = pos;
                return;
            }
            width_so_far += unit_width;
            pos = next;
        }
        self.cursor_pos = line_end;
    }

    pub(super) fn wrapped_line_index_by_start(lines: &[Range<usize>], pos: usize) -> Option<usize> {
        let idx = lines.partition_point(|range| range.start <= pos);
        (idx > 0).then_some(idx - 1)
    }

    fn line_above_cursor(&self) -> Option<VerticalNavigationTarget> {
        let cache = self.wrap_cache.borrow();
        if cache.lines.is_empty() {
            return None;
        }
        let idx = Self::wrapped_line_index_by_start(&cache.lines, self.cursor_pos)?;
        let current = &cache.lines[idx];
        let target_col = self.preferred_col.unwrap_or_else(|| {
            UnicodeWidthStr::width(
                self.display_projection_with_width(current.start..self.cursor_pos, cache.width)
                    .text
                    .as_str(),
            )
        });
        let previous = idx.checked_sub(1).map(|idx| {
            let line = &cache.lines[idx];
            (line.start, self.visual_line_end_cursor(line))
        });
        Some(VerticalNavigationTarget {
            column: target_col,
            width: cache.width,
            adjacent_line: previous,
        })
    }

    fn line_below_cursor(&self) -> Option<VerticalNavigationTarget> {
        let cache = self.wrap_cache.borrow();
        if cache.lines.is_empty() {
            return None;
        }
        let idx = Self::wrapped_line_index_by_start(&cache.lines, self.cursor_pos)?;
        let current = &cache.lines[idx];
        let target_col = self.preferred_col.unwrap_or_else(|| {
            UnicodeWidthStr::width(
                self.display_projection_with_width(current.start..self.cursor_pos, cache.width)
                    .text
                    .as_str(),
            )
        });
        let next = cache
            .lines
            .get(idx + 1)
            .map(|line| (line.start, self.visual_line_end_cursor(line)));
        Some(VerticalNavigationTarget {
            column: target_col,
            width: cache.width,
            adjacent_line: next,
        })
    }

    fn visual_line_end_cursor(&self, line: &Range<usize>) -> usize {
        if line.start == line.end {
            line.start
        } else {
            self.prev_atomic_boundary(line.end).max(line.start)
        }
    }

    pub(super) fn prev_atomic_boundary(&self, pos: usize) -> usize {
        if pos == 0 {
            return 0;
        }
        if let Some(element) = self
            .elements
            .iter()
            .find(|element| pos > element.range.start && pos <= element.range.end)
        {
            return element.range.start;
        }
        let mut cursor = GraphemeCursor::new(pos, self.text.len(), false);
        match cursor.prev_boundary(&self.text, 0) {
            Ok(Some(boundary)) => boundary,
            Ok(None) => 0,
            Err(_) => pos.saturating_sub(1),
        }
    }

    pub(super) fn next_atomic_boundary(&self, pos: usize) -> usize {
        if pos >= self.text.len() {
            return self.text.len();
        }
        if let Some(element) = self
            .elements
            .iter()
            .find(|element| pos >= element.range.start && pos < element.range.end)
        {
            return element.range.end;
        }
        let mut cursor = GraphemeCursor::new(pos, self.text.len(), false);
        match cursor.next_boundary(&self.text, 0) {
            Ok(Some(boundary)) => boundary,
            Ok(None) => self.text.len(),
            Err(_) => pos.saturating_add(1),
        }
    }
}
