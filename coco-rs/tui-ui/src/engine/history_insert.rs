//! History-row preparation for native scrollback insertion.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use ratatui::widgets::Wrap;

/// Rendered history rows ready to be inserted into native scrollback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryRows {
    buffer: Buffer,
}

/// Borrowed suffix rows from a [`HistoryRows`] buffer.
#[derive(Debug, Clone, Copy)]
pub struct HistoryRowsSlice<'a> {
    rows: &'a HistoryRows,
    source_start_row: u16,
    row_count: u16,
}

impl HistoryRows {
    pub fn new(buffer: Buffer) -> Self {
        Self { buffer }
    }

    pub fn width(&self) -> u16 {
        self.buffer.area.width
    }

    pub fn height(&self) -> u16 {
        self.buffer.area.height
    }

    pub fn is_empty(&self) -> bool {
        self.height() == 0
    }

    pub fn buffer(&self) -> &Buffer {
        &self.buffer
    }

    pub fn estimated_bytes(&self) -> usize {
        self.buffer
            .content
            .iter()
            .map(|cell| cell.symbol().len() + 8)
            .sum()
    }

    pub fn tail_slice(&self, rows: u16) -> HistoryRowsSlice<'_> {
        let row_count = rows.min(self.height());
        HistoryRowsSlice {
            rows: self,
            source_start_row: self.height().saturating_sub(row_count),
            row_count,
        }
    }

    pub fn tail_rows_copy(&self, rows: u16) -> Self {
        Self::copy_tail_from_slices(self.width(), &[self.tail_slice(rows)], rows)
    }

    pub fn copy_tail_from_slices(
        width: u16,
        slices: &[HistoryRowsSlice<'_>],
        max_rows: u16,
    ) -> Self {
        Self::copy_tail_from_matching_slices(width, slices, max_rows)
    }

    pub fn try_copy_tail_from_slices(
        width: u16,
        slices: &[HistoryRowsSlice<'_>],
        max_rows: u16,
    ) -> Option<Self> {
        if slices
            .iter()
            .any(|slice| !slice.is_empty() && slice.width() != width)
        {
            return None;
        }
        Some(Self::copy_tail_from_matching_slices(
            width, slices, max_rows,
        ))
    }

    fn copy_tail_from_matching_slices(
        width: u16,
        slices: &[HistoryRowsSlice<'_>],
        max_rows: u16,
    ) -> Self {
        if width == 0 || max_rows == 0 {
            return Self::new(Buffer::empty(Rect::new(0, 0, width, 0)));
        }

        let total_rows = slices
            .iter()
            .filter(|slice| slice.width() == width)
            .map(HistoryRowsSlice::height)
            .fold(0u16, u16::saturating_add);
        let rows_to_copy = total_rows.min(max_rows);
        if rows_to_copy == 0 {
            return Self::new(Buffer::empty(Rect::new(0, 0, width, 0)));
        }

        let mut skip_rows = total_rows.saturating_sub(rows_to_copy);
        let mut target_y = 0u16;
        let mut buffer = Buffer::empty(Rect::new(0, 0, width, rows_to_copy));
        for slice in slices.iter().filter(|slice| slice.width() == width) {
            if skip_rows >= slice.height() {
                skip_rows -= slice.height();
                continue;
            }
            let source_offset = skip_rows;
            skip_rows = 0;
            let source_start = slice.source_start_row() + source_offset;
            let source_end = slice.source_start_row() + slice.height();
            for source_y in source_start..source_end {
                if target_y >= rows_to_copy {
                    break;
                }
                copy_row_from(slice.buffer(), source_y, &mut buffer, target_y, width);
                target_y += 1;
            }
        }
        Self::new(buffer)
    }
}

fn copy_row_from(source: &Buffer, source_y: u16, target: &mut Buffer, target_y: u16, width: u16) {
    let source_start = source.index_of(0, source_y);
    let target_start = target.index_of(0, target_y);
    let width = width as usize;
    target.content[target_start..target_start + width]
        .clone_from_slice(&source.content[source_start..source_start + width]);
}

impl<'a> HistoryRowsSlice<'a> {
    pub fn width(&self) -> u16 {
        self.rows.width()
    }

    pub fn height(&self) -> u16 {
        self.row_count
    }

    pub fn is_empty(&self) -> bool {
        self.row_count == 0
    }

    pub fn buffer(&self) -> &'a Buffer {
        self.rows.buffer()
    }

    pub fn source_start_row(&self) -> u16 {
        self.source_start_row
    }
}

/// Render finalized history lines into rows at the target width.
///
/// Lines are **wrapped** to `width` (word-wrap, `trim: false`) so a committed
/// scrollback line occupies the exact same rows it did in the live tail — which
/// paints with `Paragraph::wrap(Wrap { trim: false })`. Without wrapping here, a
/// logical line longer than `width` was clipped to a single row on commit,
/// silently dropping its wrapped continuation and desyncing native scrollback
/// from the live render. The buffer height is the wrapped row count
/// (`Paragraph::line_count`, the same `WordWrapper` the renderer uses), not
/// `lines.len()`, so the caller inserts the correct number of scrollback rows.
pub fn render_history_rows(lines: Vec<Line<'static>>, width: u16) -> HistoryRows {
    if width == 0 || lines.is_empty() {
        return HistoryRows::new(Buffer::empty(Rect::new(0, 0, width, 0)));
    }
    // DEBUG (ambiguous-width investigation, tui-v2): the wrapped row count below
    // (`line_count`) and every column position assume the default unicode width
    // (East-Asian Ambiguous = 1). A terminal that renders ambiguous chars wide
    // (CJK setups) wraps such a line to MORE rows than computed here, so the
    // caller scrolls the terminal by too few rows and native scrollback desyncs
    // from reality. Flag any committed line whose default width disagrees with
    // the CJK width so a `tui=debug` repro shows exactly which rows are at risk.
    if tracing::enabled!(target: "tui::engine::width", tracing::Level::DEBUG) {
        for (idx, line) in lines.iter().enumerate() {
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            let w = unicode_width::UnicodeWidthStr::width(text.as_str());
            let w_cjk = unicode_width::UnicodeWidthStr::width_cjk(text.as_str());
            if w != w_cjk {
                tracing::debug!(
                    target: "tui::engine::width",
                    line = idx,
                    width_default = w,
                    width_cjk = w_cjk,
                    render_width = width,
                    // The dangerous case: fits under `width` by the default
                    // measure but overflows by the CJK measure → the terminal
                    // wraps to an extra row the row-count math never accounts for.
                    cjk_overflows = w <= width as usize && w_cjk > width as usize,
                    text = %text.chars().take(100).collect::<String>(),
                    "history row has ambiguous-width chars (default vs cjk width differ)",
                );
            }
        }
    }
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    let height = paragraph.line_count(width).min(u16::MAX as usize) as u16;
    let area = Rect::new(0, 0, width, height);
    let mut buffer = Buffer::empty(area);
    if height > 0 {
        paragraph.render(area, &mut buffer);
    }
    HistoryRows::new(buffer)
}

#[cfg(test)]
#[path = "history_insert.test.rs"]
mod tests;
