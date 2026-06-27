//! `VT100Backend` — a [`SurfaceBackend`] that decodes the paint engine's
//! *real* emitted ANSI through a `vt100` terminal emulator, so tests can assert
//! on the resulting cell grid (text, styling, cursor) instead of the in-memory
//! ratatui buffer.
//!
//! The in-memory `TestBackend` only proves "ratatui thinks it drew X". It can
//! never catch a malformed SGR run, an off-by-one cursor move, or a scrollback
//! framing bug, because those live in the *bytes* the engine emits — not in the
//! buffer it diffed against. `VT100Backend` wraps `CrosstermBackend<vt100::Parser>`:
//! ratatui draws through crossterm, crossterm writes the exact production ANSI
//! into the parser, and the parser maintains a real terminal grid we can read
//! back. Every [`SurfaceBackend`] escape (BSU/ESU, scrollback clear, direct
//! history-row insertion) is delegated to the inner crossterm backend, so the
//! emulator sees byte-for-byte what a real terminal would.
//!
//! Gated behind `cfg(any(test, feature = "testing"))`; `app/tui` enables the
//! `testing` feature in its `[dev-dependencies]` so its tests can drive a
//! `SurfaceTerminal<VT100Backend>` end to end.

use ratatui::backend::Backend;
use ratatui::backend::ClearType;
use ratatui::backend::CrosstermBackend;
use ratatui::backend::WindowSize;
use ratatui::buffer::Cell;
use ratatui::layout::Position;
use ratatui::layout::Size;
use std::io;

use super::terminal::SurfaceBackend;

/// A terminal-emulator-backed [`SurfaceBackend`]. Construct it, hand it to
/// [`super::terminal::SurfaceTerminal::new`], draw, then inspect the decoded
/// grid via [`Self::screen`].
pub struct VT100Backend {
    inner: CrosstermBackend<vt100::Parser>,
}

impl std::fmt::Debug for VT100Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `vt100::Parser` is not `Debug`; surface the decoded screen text so a
        // failing `SurfaceTerminal<VT100Backend>` assertion still prints usefully.
        f.debug_struct("VT100Backend")
            .field("screen", &self.contents())
            .finish()
    }
}

impl VT100Backend {
    /// A `cols` × `rows` emulator with no off-screen scrollback.
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            inner: CrosstermBackend::new(vt100::Parser::new(rows, cols, 0)),
        }
    }

    /// A `cols` × `rows` emulator retaining `scrollback` rows above the visible
    /// viewport, so tests can assert on history committed off-screen.
    pub fn with_scrollback(cols: u16, rows: u16, scrollback: usize) -> Self {
        Self {
            inner: CrosstermBackend::new(vt100::Parser::new(rows, cols, scrollback)),
        }
    }

    /// The decoded terminal grid. Query cells with `screen.cell(row, col)`,
    /// the cursor with `screen.cursor_position()`, and visible text with
    /// `screen.contents()`.
    pub fn screen(&self) -> &vt100::Screen {
        self.inner.writer().screen()
    }

    /// Visible-screen text with styling stripped — convenient for substring
    /// assertions. Prefer [`Self::screen`] + per-cell checks for styling.
    pub fn contents(&self) -> String {
        self.screen().contents()
    }
}

impl Backend for VT100Backend {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.inner.draw(content)
    }

    fn append_lines(&mut self, n: u16) -> io::Result<()> {
        self.inner.append_lines(n)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        // Read from the emulator's decoded grid, NOT crossterm's real-tty cursor
        // query (which would block on a DSR response that never arrives in tests).
        let (row, col) = self.screen().cursor_position();
        Ok(Position::new(col, row))
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.inner.set_cursor_position(position)
    }

    fn clear(&mut self) -> io::Result<()> {
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        self.inner.clear_region(clear_type)
    }

    fn size(&self) -> io::Result<Size> {
        // vt100 reports (rows, cols); ratatui `Size` is (width, height).
        let (rows, cols) = self.screen().size();
        Ok(Size::new(cols, rows))
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        let (rows, cols) = self.screen().size();
        Ok(WindowSize {
            columns_rows: Size::new(cols, rows),
            // Pixel size is unused by the paint engine; report 0×0.
            pixels: Size::new(0, 0),
        })
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }

    // The workspace always enables ratatui's `scrolling-regions`, so the
    // `Backend` trait requires these. They are gated on ratatui's feature, not
    // a feature of this crate — implement them unconditionally and delegate.
    fn scroll_region_up(
        &mut self,
        region: std::ops::Range<u16>,
        line_count: u16,
    ) -> io::Result<()> {
        self.inner.scroll_region_up(region, line_count)
    }

    fn scroll_region_down(
        &mut self,
        region: std::ops::Range<u16>,
        line_count: u16,
    ) -> io::Result<()> {
        self.inner.scroll_region_down(region, line_count)
    }
}

impl SurfaceBackend for VT100Backend {
    fn clear_scrollback_and_screen(&mut self) -> io::Result<()> {
        self.inner.clear_scrollback_and_screen()
    }

    fn set_cursor_style(&mut self, style: crossterm::cursor::SetCursorStyle) -> io::Result<()> {
        self.inner.set_cursor_style(style)
    }

    fn begin_synchronized_update(&mut self) -> io::Result<()> {
        self.inner.begin_synchronized_update()
    }

    fn end_synchronized_update(&mut self) -> io::Result<()> {
        self.inner.end_synchronized_update()
    }

    fn enter_modal_alt_screen(&mut self) -> io::Result<()> {
        self.inner.enter_modal_alt_screen()
    }

    fn leave_modal_alt_screen(&mut self) -> io::Result<()> {
        self.inner.leave_modal_alt_screen()
    }

    fn leave_terminal_modes(&mut self) -> io::Result<()> {
        self.inner.leave_terminal_modes()
    }

    fn write_drop_trailing_newline(&mut self) -> io::Result<()> {
        self.inner.write_drop_trailing_newline()
    }

    fn insert_history_rows_direct(
        &mut self,
        rendered: &ratatui::buffer::Buffer,
        source_start_row: u16,
        row_count: u16,
        target_top: u16,
        scratch: &mut String,
    ) -> io::Result<Option<usize>> {
        self.inner.insert_history_rows_direct(
            rendered,
            source_start_row,
            row_count,
            target_top,
            scratch,
        )
    }
}
