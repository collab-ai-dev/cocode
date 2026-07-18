//! Crossterm adapter for the generic background [`FrameWriter`].

use std::io;
use std::io::Write;
use std::ops::Range;
use std::time::Duration;

use crossterm::cursor::SetCursorStyle;
use ratatui::backend::Backend;
use ratatui::backend::ClearType;
use ratatui::backend::CrosstermBackend;
use ratatui::backend::WindowSize;
use ratatui::buffer::Cell;
use ratatui::layout::Position;
use ratatui::layout::Size;

use super::frame_writer::DrainBarrier;
use super::frame_writer::FrameDelivery;
use super::frame_writer::FrameWriter;
use super::frame_writer::FrameWriterOptions;
use super::terminal::DirectHistoryInsert;
use super::terminal::SurfaceBackend;
use super::terminal::TerminalWriteStats;

/// Crossterm backend whose completed synchronized-update frames are delivered
/// by [`FrameWriter`]. Ratatui output is incremental and therefore lossless.
pub struct FrameCrosstermBackend<W>
where
    W: Write + Send + 'static,
{
    inner: CrosstermBackend<FrameWriter<W>>,
}

impl<W> std::fmt::Debug for FrameCrosstermBackend<W>
where
    W: Write + Send + 'static,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrameCrosstermBackend")
            .field("writer", self.inner.writer())
            .finish()
    }
}

impl<W> FrameCrosstermBackend<W>
where
    W: Write + Send + 'static,
{
    pub fn new(writer: W, options: FrameWriterOptions) -> io::Result<Self> {
        Ok(Self {
            inner: CrosstermBackend::new(FrameWriter::new(writer, options)?),
        })
    }

    pub fn drain_barrier(&self) -> DrainBarrier {
        self.inner.writer().barrier()
    }

    fn present_incremental(&mut self) -> io::Result<()> {
        self.inner.writer_mut().present(FrameDelivery::Incremental)
    }

    fn present_teardown(&mut self) -> io::Result<()> {
        self.inner.writer_mut().present(FrameDelivery::Teardown)
    }
}

impl<W> Backend for FrameCrosstermBackend<W>
where
    W: Write + Send + 'static,
{
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
        self.inner.get_cursor_position()
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
        self.inner.size()
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        self.inner.window_size()
    }

    fn flush(&mut self) -> io::Result<()> {
        Backend::flush(&mut self.inner)
    }

    fn scroll_region_up(&mut self, region: Range<u16>, line_count: u16) -> io::Result<()> {
        self.inner.scroll_region_up(region, line_count)
    }

    fn scroll_region_down(&mut self, region: Range<u16>, line_count: u16) -> io::Result<()> {
        self.inner.scroll_region_down(region, line_count)
    }
}

impl<W> SurfaceBackend for FrameCrosstermBackend<W>
where
    W: Write + Send + 'static,
{
    fn clear_scrollback_and_screen(&mut self) -> io::Result<()> {
        self.inner.clear_scrollback_and_screen()
    }

    fn set_cursor_style(&mut self, style: SetCursorStyle) -> io::Result<()> {
        self.inner.set_cursor_style(style)
    }

    fn begin_synchronized_update(&mut self) -> io::Result<()> {
        self.inner.begin_synchronized_update()
    }

    fn end_synchronized_update(&mut self) -> io::Result<()> {
        self.inner.end_synchronized_update()?;
        self.present_incremental()
    }

    fn enter_modal_alt_screen(&mut self) -> io::Result<()> {
        self.inner.enter_modal_alt_screen()
    }

    fn leave_modal_alt_screen(&mut self) -> io::Result<()> {
        self.inner.leave_modal_alt_screen()
    }

    fn begin_terminal_restore(&mut self) -> io::Result<()> {
        self.inner.begin_terminal_restore()
    }

    fn finish_terminal_restore(&mut self) -> io::Result<()> {
        self.inner.finish_terminal_restore()
    }

    fn write_drop_trailing_newline(&mut self) -> io::Result<()> {
        self.inner.write_drop_trailing_newline()?;
        self.present_teardown()
    }

    fn drain_output(&mut self, timeout: Duration) -> io::Result<bool> {
        self.present_incremental()?;
        self.drain_barrier().wait_drained(timeout)
    }

    fn terminal_write_stats(&self) -> Option<TerminalWriteStats> {
        self.drain_barrier().latest_write_stats()
    }

    fn insert_history_rows_direct(
        &mut self,
        request: DirectHistoryInsert<'_>,
    ) -> io::Result<Option<usize>> {
        self.inner.insert_history_rows_direct(request)
    }
}

#[cfg(test)]
#[path = "frame_backend.test.rs"]
mod tests;
