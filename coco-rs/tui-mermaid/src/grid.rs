//! A small styled character grid that the layout emitter paints into, then
//! flattens to `Vec<Line<'static>>`. Box-drawing strokes merge at junctions so
//! crossing edges and node borders connect cleanly; wide glyphs reserve a
//! trailing shadow cell so column alignment holds.

use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use unicode_width::UnicodeWidthChar;
use unicode_width::UnicodeWidthStr;

/// Sentinel marking the second column of a width-2 glyph; skipped on flatten.
const SHADOW: char = '\u{0}';

#[derive(Clone, Copy)]
struct GCell {
    ch: char,
    style: Style,
}

pub(crate) struct CellGrid {
    w: usize,
    h: usize,
    cells: Vec<GCell>,
}

impl CellGrid {
    pub(crate) fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            cells: vec![
                GCell {
                    ch: ' ',
                    style: Style::default(),
                };
                w.saturating_mul(h)
            ],
        }
    }

    pub(crate) fn width(&self) -> usize {
        self.w
    }

    fn set(&mut self, x: usize, y: usize, ch: char, style: Style) {
        if x < self.w && y < self.h {
            self.cells[y * self.w + x] = GCell { ch, style };
        }
    }

    fn ch_at(&self, x: usize, y: usize) -> char {
        if x < self.w && y < self.h {
            self.cells[y * self.w + x].ch
        } else {
            ' '
        }
    }

    /// Place a box-drawing stroke, merging with any existing stroke so the two
    /// connect at a junction. Non-stroke cells are overwritten.
    pub(crate) fn stroke(&mut self, x: usize, y: usize, ch: char, style: Style) {
        let merged = match (box_mask(self.ch_at(x, y)), box_mask(ch)) {
            (Some(a), Some(b)) => mask_to_box(a | b),
            _ => ch,
        };
        self.set(x, y, merged, style);
    }

    pub(crate) fn hline(&mut self, x0: usize, x1: usize, y: usize, style: Style) {
        for x in x0.min(x1)..=x0.max(x1) {
            self.stroke(x, y, '─', style);
        }
    }

    pub(crate) fn vline(&mut self, x: usize, y0: usize, y1: usize, style: Style) {
        for y in y0.min(y1)..=y0.max(y1) {
            self.stroke(x, y, '│', style);
        }
    }

    /// Rounded-corner rectangle border; interior untouched.
    pub(crate) fn rect(&mut self, x: usize, y: usize, w: usize, h: usize, style: Style) {
        if w < 2 || h < 2 {
            return;
        }
        let (x1, y1) = (x + w - 1, y + h - 1);
        for cx in (x + 1)..x1 {
            self.stroke(cx, y, '─', style);
            self.stroke(cx, y1, '─', style);
        }
        for cy in (y + 1)..y1 {
            self.stroke(x, cy, '│', style);
            self.stroke(x1, cy, '│', style);
        }
        // Corners last so they win over the straight runs.
        self.set(x, y, '╭', style);
        self.set(x1, y, '╮', style);
        self.set(x, y1, '╰', style);
        self.set(x1, y1, '╯', style);
    }

    /// Write `s` starting at `x`, advancing by display width and reserving a
    /// shadow cell after each wide glyph. Stops at the right edge.
    pub(crate) fn text(&mut self, x: usize, y: usize, s: &str, style: Style) {
        let mut col = x;
        for ch in s.chars() {
            let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
            if cw == 0 || ch == SHADOW {
                continue;
            }
            if col >= self.w {
                break;
            }
            // A width-2 glyph needs both `col` and its shadow at `col+1`; if the
            // shadow would fall off the right edge, stop rather than emit a glyph
            // whose true display width exceeds the grid width by one.
            if cw == 2 && col + 1 >= self.w {
                break;
            }
            self.set(col, y, ch, style);
            if cw == 2 {
                self.set(col + 1, y, SHADOW, style);
            }
            col += cw;
        }
    }

    /// Center `s` within `[x, x + max_w)`, truncating with `…` on overflow.
    pub(crate) fn text_centered(
        &mut self,
        x: usize,
        y: usize,
        max_w: usize,
        s: &str,
        style: Style,
    ) {
        let t = truncate_to_width(s, max_w);
        let off = max_w.saturating_sub(t.width()) / 2;
        self.text(x + off, y, &t, style);
    }

    /// Overwrite a single cell unconditionally (used for arrowheads).
    pub(crate) fn put(&mut self, x: usize, y: usize, ch: char, style: Style) {
        self.set(x, y, ch, style);
    }

    /// True if every cell in `[x, x + w)` on row `y` is blank. Out-of-bounds
    /// counts as not-clear, so a caller can't place a run that runs off the
    /// grid. A wide glyph's SHADOW cell counts as occupied.
    pub(crate) fn run_is_clear(&self, x: usize, y: usize, w: usize) -> bool {
        if y >= self.h || x.saturating_add(w) > self.w {
            return false;
        }
        (x..x + w).all(|c| self.cells[y * self.w + c].ch == ' ')
    }

    /// Flatten to lines, coalescing same-style runs and trimming trailing blank
    /// cells per row AND the leading/trailing fully-blank rows the layout's
    /// boundary padding introduces, so the diagram emits no surrounding
    /// whitespace into the markdown stream.
    pub(crate) fn into_lines(self) -> Vec<Line<'static>> {
        let mut out = Vec::with_capacity(self.h);
        for row in 0..self.h {
            let base = row * self.w;
            let last = (0..self.w)
                .rev()
                .find(|&c| {
                    let ch = self.cells[base + c].ch;
                    ch != ' ' && ch != SHADOW
                })
                .map(|c| c + 1)
                .unwrap_or(0);
            let mut spans: Vec<Span<'static>> = Vec::new();
            let mut buf = String::new();
            let mut buf_style: Option<Style> = None;
            for col in 0..last {
                let cell = self.cells[base + col];
                if cell.ch == SHADOW {
                    continue;
                }
                if buf_style != Some(cell.style) {
                    if !buf.is_empty() {
                        spans.push(Span::styled(
                            std::mem::take(&mut buf),
                            buf_style.unwrap_or_default(),
                        ));
                    }
                    buf_style = Some(cell.style);
                }
                buf.push(cell.ch);
            }
            if !buf.is_empty() {
                spans.push(Span::styled(buf, buf_style.unwrap_or_default()));
            }
            out.push(Line::from(spans));
        }
        // Trim leading/trailing fully-blank rows (layout boundary padding).
        let is_blank =
            |l: &Line<'static>| l.spans.iter().all(|s| s.content.chars().all(|c| c == ' '));
        let (Some(first), Some(lastnb)) = (
            out.iter().position(|l| !is_blank(l)),
            out.iter().rposition(|l| !is_blank(l)),
        ) else {
            return Vec::new();
        };
        out.truncate(lastnb + 1);
        out.drain(..first);
        out
    }
}

// Display-width-aware truncation is the canonical helper in coco-tui-ui; reuse
// it rather than re-implementing (one source of truth across the TUI crates).
pub(crate) use coco_tui_ui::truncate::truncate_to_width;

/// Box-drawing connection mask: bit0=up, bit1=down, bit2=left, bit3=right.
fn box_mask(ch: char) -> Option<u8> {
    let m = match ch {
        '─' => 0b1100,
        '│' => 0b0011,
        '╭' | '┌' => 0b1010,
        '╮' | '┐' => 0b0110,
        '╰' | '└' => 0b1001,
        '╯' | '┘' => 0b0101,
        '├' => 0b1011,
        '┤' => 0b0111,
        '┬' => 0b1110,
        '┴' => 0b1101,
        '┼' => 0b1111,
        _ => return None,
    };
    Some(m)
}

fn mask_to_box(m: u8) -> char {
    match m {
        // only up/down bits set (bit0=up, bit1=down) → vertical stroke.
        0b0001..=0b0011 => '│',
        0b1100 | 0b0100 | 0b1000 => '─',
        0b1010 => '╭',
        0b0110 => '╮',
        0b1001 => '╰',
        0b0101 => '╯',
        0b1011 => '├',
        0b0111 => '┤',
        0b1110 => '┬',
        0b1101 => '┴',
        _ => '┼',
    }
}

#[cfg(test)]
#[path = "grid.test.rs"]
mod tests;
