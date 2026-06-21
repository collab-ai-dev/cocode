//! Surface terminal substrate for the native-scrollback TUI.

use crossterm::cursor::SetCursorStyle;
use crossterm::event::DisableBracketedPaste;
use crossterm::event::DisableFocusChange;
use crossterm::queue;
use crossterm::terminal::BeginSynchronizedUpdate;
use crossterm::terminal::EndSynchronizedUpdate;
use crossterm::terminal::EnterAlternateScreen;
use crossterm::terminal::LeaveAlternateScreen;
use ratatui::backend::Backend;
use ratatui::backend::ClearType;
use ratatui::backend::CrosstermBackend;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::buffer::Cell;
use ratatui::layout::Position;
use ratatui::layout::Rect;
use ratatui::layout::Size;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::widgets::Widget;
use std::io::Write;
use std::time::Duration;
use std::time::Instant;
use unicode_width::UnicodeWidthStr;

use super::CursorClaim;
use super::history_insert::HistoryRows;

pub trait SurfaceBackend: Backend {
    fn clear_scrollback_and_screen(&mut self) -> Result<(), Self::Error> {
        self.clear_region(ClearType::All)
    }

    fn set_cursor_style(&mut self, _style: SetCursorStyle) -> Result<(), Self::Error> {
        Ok(())
    }

    fn begin_synchronized_update(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn end_synchronized_update(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn enter_modal_alt_screen(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn leave_modal_alt_screen(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    /// Emit the terminal-mode-restore escapes (disable alternate scroll,
    /// bracketed paste, and focus reporting) through this backend's sink.
    ///
    /// Does NOT leave the alternate screen. coco keeps the main session in
    /// the primary buffer (native scrollback) and never enters the alt
    /// screen for it, so a `CSI ?1049l` here would be unpaired: on a real
    /// terminal it performs a DECRC onto whatever the shared save register
    /// happens to hold (coco never writes `\x1b7`; prompt frameworks and
    /// prior programs do), yanking the cursor to a stale position so
    /// whatever prints next (the resume hint) overprints the transcript.
    /// codex's `restore_common` omits the alt-screen leave for exactly
    /// this reason. Leaving a modal alt-screen is the caller's job
    /// (`leave_modal_alt_screen`), emitted only when actually in it.
    ///
    /// Routing teardown escapes through the owned backend — rather than a
    /// free `execute!(io::stdout(), …)` — keeps the exit byte order
    /// observable and deterministically interleaved with the prompt-cursor
    /// placement that follows. The shell's panic / external-process path
    /// emits its own reset to global stdout, where no backend is reachable
    /// (`coco_tui`'s `leave_tui_modes`). Default no-op for backends that do
    /// not model terminal modes.
    fn leave_terminal_modes(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn write_drop_trailing_newline(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn insert_history_rows_direct(
        &mut self,
        _rendered: &Buffer,
        _source_start_row: u16,
        _row_count: u16,
        _target_top: u16,
        _scratch: &mut String,
    ) -> Result<Option<usize>, Self::Error> {
        Ok(None)
    }
}

impl<W> SurfaceBackend for CrosstermBackend<W>
where
    W: Write,
{
    fn clear_scrollback_and_screen(&mut self) -> Result<(), Self::Error> {
        write!(self, "\x1b[r\x1b[0m\x1b[H\x1b[2J\x1b[3J\x1b[H")?;
        Write::flush(self)
    }

    fn set_cursor_style(&mut self, style: SetCursorStyle) -> Result<(), Self::Error> {
        queue!(self, style)?;
        Ok(())
    }

    fn begin_synchronized_update(&mut self) -> Result<(), Self::Error> {
        queue!(self, BeginSynchronizedUpdate)?;
        Ok(())
    }

    fn end_synchronized_update(&mut self) -> Result<(), Self::Error> {
        queue!(self, EndSynchronizedUpdate)?;
        Write::flush(self)
    }

    fn enter_modal_alt_screen(&mut self) -> Result<(), Self::Error> {
        queue!(self, EnterAlternateScreen)?;
        write!(self, "\x1b[?1007h")?;
        Write::flush(self)
    }

    fn leave_modal_alt_screen(&mut self) -> Result<(), Self::Error> {
        write!(self, "\x1b[?1007l")?;
        queue!(self, LeaveAlternateScreen)?;
        Write::flush(self)
    }

    fn leave_terminal_modes(&mut self) -> Result<(), Self::Error> {
        // Disable alternate scroll + bracketed paste + focus reporting. The
        // alternate screen is intentionally NOT left here (see the trait
        // doc): the main session never enters it, so an unpaired `?1049l`
        // would DECRC the cursor to a stale saved position and the resume
        // hint would overprint the transcript.
        write!(self, "\x1b[?1007l")?;
        queue!(self, DisableBracketedPaste, DisableFocusChange)?;
        Write::flush(self)
    }

    fn write_drop_trailing_newline(&mut self) -> Result<(), Self::Error> {
        self.write_all(b"\r\n")?;
        Write::flush(self)
    }

    fn insert_history_rows_direct(
        &mut self,
        rendered: &Buffer,
        source_start_row: u16,
        row_count: u16,
        target_top: u16,
        scratch: &mut String,
    ) -> Result<Option<usize>, Self::Error> {
        // No DECSC/DECRC bracket: every row is absolutely addressed and the
        // engine re-parks the cursor from app-tracked state after the insert
        // (`SurfaceTerminal::insert_history_rows`), so the terminal's shared
        // save register stays untouched — a stale save there is what an
        // unpaired DECRC (e.g. a `?1049l`) dereferences.
        scratch.clear();
        scratch.reserve((row_count as usize) * (rendered.area.width as usize + 16));
        for source_y in source_start_row..source_start_row.saturating_add(row_count) {
            let target_y = target_top + (source_y - source_start_row);
            scratch.push_str("\x1b[");
            push_u16(scratch, target_y + 1);
            scratch.push_str(";1H");
            let mut current_style: Option<CellStyleKey> = None;
            // Cells occupied by the continuation half of a preceding wide
            // (CJK / emoji) grapheme. ratatui 0.30 fills these with a reset
            // space (`skip == false`), so width tracking — not `cell.skip` —
            // is what keeps a `运` from being emitted as `运 `.
            let mut to_skip = 0usize;
            for x in 0..rendered.area.width {
                let index = rendered.index_of(x, source_y);
                let cell = &rendered.content[index];
                if !cell.skip && to_skip == 0 {
                    let next_style = CellStyleKey::from(cell);
                    if current_style != Some(next_style) {
                        push_ansi_style_prefix(scratch, next_style);
                        current_style = Some(next_style);
                    }
                    scratch.push_str(cell.symbol());
                }
                to_skip = display_width(cell.symbol()).saturating_sub(1);
            }
        }
        scratch.push_str("\x1b[0m");
        let bytes = scratch.len();
        self.write_all(scratch.as_bytes())?;
        Ok(Some(bytes))
    }
}

impl SurfaceBackend for TestBackend {
    fn clear_scrollback_and_screen(&mut self) -> Result<(), Self::Error> {
        let size = self.size()?;
        *self = TestBackend::new(size.width, size.height);
        Ok(())
    }
}

/// A retained inline viewport with explicit cursor and history accounting.
///
/// This is intentionally smaller than the final native terminal contract. It
/// proves the central ownership model on crates.io `ratatui 0.30`: coco owns
/// viewport geometry, draw buffers, cursor policy application, and visible
/// history row accounting instead of relying on stock `Viewport::Inline`.
#[derive(Debug)]
pub struct SurfaceTerminal<B: SurfaceBackend> {
    backend: B,
    buffers: [Buffer; 2],
    current: usize,
    viewport_area: Rect,
    last_known_screen_size: Size,
    visible_history_rows: u16,
    history_bottom_y: u16,
    /// Where the engine last deliberately placed the cursor (claim, hidden
    /// park, clear, or shell prompt). History inserts re-park from this
    /// app-owned value with an absolute move — never via the terminal's
    /// DECSC/DECRC save register, which is shared, hidden state with
    /// per-terminal semantics.
    last_parked_cursor: Position,
    invalidated: bool,
    perf_stats_enabled: bool,
    history_row_scratch: String,
    last_viewport_draw_stats: ViewportDrawStats,
    last_history_insert_stats: HistoryInsertStats,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewportResizePolicy {
    PreserveHistory,
    ScrollHistoryOnGrowth,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ViewportDrawStats {
    pub buffer_updates: usize,
    pub invalidated: bool,
    /// Time spent in the caller's render closure (widget paint into the back
    /// buffer) — separates "building the frame" from the diff/draw/flush
    /// pipeline below it.
    pub render_elapsed: Duration,
    pub diff_elapsed: Duration,
    pub draw_elapsed: Duration,
    pub flush_elapsed: Duration,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct HistoryInsertStats {
    pub wrapped_rows: u16,
    pub buffer_updates: usize,
    pub bytes_written: usize,
    pub invalidated: bool,
    pub build_elapsed: Duration,
    pub draw_elapsed: Duration,
    pub flush_elapsed: Duration,
}

/// Frame passed to surface renderers.
pub struct SurfaceFrame<'a> {
    viewport_area: Rect,
    buffer: &'a mut Buffer,
    cursor_claim: Option<CursorClaim>,
}

impl<'a> SurfaceFrame<'a> {
    /// Area of the retained viewport.
    pub fn area(&self) -> Rect {
        self.viewport_area
    }

    /// Render a ratatui widget into the frame buffer.
    pub fn render_widget<W: Widget>(&mut self, widget: W, area: Rect) {
        widget.render(area, self.buffer);
    }

    /// Claim the terminal cursor for this frame.
    pub fn set_cursor_claim(&mut self, claim: CursorClaim) {
        self.cursor_claim = Some(claim);
    }
}

impl<B> SurfaceTerminal<B>
where
    B: SurfaceBackend,
{
    /// Create a surface terminal using the backend's current size as the
    /// initial viewport.
    pub fn new(backend: B) -> Result<Self, B::Error> {
        let screen_size = backend.size()?;
        let viewport_area = Rect::new(0, 0, screen_size.width, 0);
        Ok(Self {
            backend,
            buffers: [Buffer::empty(viewport_area), Buffer::empty(viewport_area)],
            current: 0,
            viewport_area,
            last_known_screen_size: screen_size,
            visible_history_rows: 0,
            history_bottom_y: viewport_area.top(),
            last_parked_cursor: Position { x: 0, y: 0 },
            invalidated: true,
            perf_stats_enabled: false,
            history_row_scratch: String::new(),
            last_viewport_draw_stats: ViewportDrawStats::default(),
            last_history_insert_stats: HistoryInsertStats::default(),
        })
    }

    /// Immutable backend access for tests and surface adapters.
    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// Mutable backend access for integration glue.
    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    /// Current retained viewport area.
    pub fn viewport_area(&self) -> Rect {
        self.viewport_area
    }

    /// Row immediately after the finalized history owned by this surface.
    pub fn history_bottom_y(&self) -> u16 {
        self.history_bottom_y
    }

    /// Last backend screen size observed by the surface terminal.
    #[cfg(test)]
    pub fn last_known_screen_size(&self) -> Size {
        self.last_known_screen_size
    }

    /// Current backend-reported terminal size.
    pub fn size(&self) -> Result<Size, B::Error> {
        self.backend.size()
    }

    /// Synchronize the terminal size observed by the outer draw loop.
    ///
    /// Native-surface callers already need the screen size to plan the
    /// viewport. Passing that observation here keeps history insertion off the
    /// terminal-size syscall hot path.
    pub fn sync_screen_size(&mut self, size: Size) {
        self.note_observed_screen_size(size);
    }

    pub fn last_viewport_draw_stats(&self) -> ViewportDrawStats {
        self.last_viewport_draw_stats
    }

    pub fn last_history_insert_stats(&self) -> HistoryInsertStats {
        self.last_history_insert_stats
    }

    pub fn set_perf_stats_enabled(&mut self, enabled: bool) {
        self.perf_stats_enabled = enabled;
    }

    /// Rows of finalized history known to be visible above the viewport.
    #[cfg(any(test, feature = "testing"))]
    pub fn visible_history_rows(&self) -> u16 {
        self.visible_history_rows
    }

    /// Set the retained viewport area and resize both diff buffers.
    pub fn set_viewport_area(&mut self, area: Rect) {
        let had_history = self.visible_history_rows > 0;
        self.viewport_area = area;
        self.buffers[0].resize(area);
        self.buffers[1].resize(area);
        self.visible_history_rows = self.visible_history_rows.min(area.top());
        self.history_bottom_y = if had_history {
            self.history_bottom_y
                .min(area.top())
                .max(self.visible_history_rows)
        } else {
            area.top()
        };
        self.invalidate_viewport();
    }

    /// Move the retained viewport and clear stale cells from the old
    /// interactive region. When the inline viewport grows upward, callers can
    /// ask the terminal to scroll finalized history up before clearing the old
    /// viewport, matching the native-scrollback draw path.
    pub fn apply_viewport_area(
        &mut self,
        area: Rect,
        scroll_history_on_growth: bool,
    ) -> Result<(), B::Error> {
        let policy = if scroll_history_on_growth {
            ViewportResizePolicy::ScrollHistoryOnGrowth
        } else {
            ViewportResizePolicy::PreserveHistory
        };
        self.apply_viewport_area_with_policy(area, policy)
    }

    pub fn apply_viewport_area_with_policy(
        &mut self,
        area: Rect,
        policy: ViewportResizePolicy,
    ) -> Result<(), B::Error> {
        let previous = self.viewport_area;
        if previous == area {
            return Ok(());
        }

        let size = self.size()?;
        let initial_fullscreen = previous.x == 0
            && previous.y == 0
            && previous.width == size.width
            && previous.height == size.height;

        if policy == ViewportResizePolicy::ScrollHistoryOnGrowth
            && !initial_fullscreen
            && area.y < previous.y
            && area.bottom() >= size.height
        {
            let scroll_by = previous.y - area.y;
            if scroll_by > 0 && previous.y > 0 {
                self.backend.scroll_region_up(0..previous.y, scroll_by)?;
                self.history_bottom_y = self.history_bottom_y.saturating_sub(scroll_by);
            }
            self.clear_after_position(Position {
                x: 0,
                y: previous.y,
            })?;
        } else {
            let clear_y = if initial_fullscreen {
                area.y
            } else {
                previous.y.min(area.y)
            };
            self.clear_after_position(Position { x: 0, y: clear_y })?;
        }

        tracing::debug!(
            target: "tui::surface",
            previous = ?previous,
            next = ?area,
            history_bottom_y = self.history_bottom_y,
            ?policy,
            "apply viewport area"
        );
        self.set_viewport_area(area);
        Ok(())
    }

    /// Mark the next draw as a full repaint.
    pub fn invalidate_viewport(&mut self) {
        self.invalidated = true;
        self.previous_buffer_mut().reset();
    }

    /// Record history rows inserted above the retained viewport.
    pub fn note_history_rows_inserted(&mut self, rows: u16) {
        self.history_bottom_y = self
            .history_bottom_y
            .saturating_add(rows)
            .min(self.viewport_area.top());
        self.visible_history_rows = self
            .visible_history_rows
            .saturating_add(rows)
            .min(self.history_bottom_y);
    }

    /// Clear visible terminal content owned by the surface and reset history
    /// accounting.
    pub fn clear_owned_scrollback(&mut self) -> Result<(), B::Error> {
        let previous = self.viewport_area;
        self.backend.clear_scrollback_and_screen()?;
        // The clear escape ends with the cursor homed.
        self.last_parked_cursor = Position { x: 0, y: 0 };
        self.visible_history_rows = 0;
        self.history_bottom_y = 0;
        self.viewport_area.y = 0;
        self.buffers[0].resize(self.viewport_area);
        self.buffers[1].resize(self.viewport_area);
        tracing::debug!(
            target: "tui::surface",
            previous = ?previous,
            next = ?self.viewport_area,
            "clear owned scrollback"
        );
        self.invalidate_viewport();
        Ok(())
    }

    /// Clear the retained interactive viewport while preserving rows above it.
    #[cfg(test)]
    pub fn clear_viewport_to_end(&mut self) -> Result<(), B::Error> {
        if self.viewport_area.width == 0 || self.viewport_area.height == 0 {
            return Ok(());
        }

        self.clear_after_position(Position {
            x: self.viewport_area.x,
            y: self.viewport_area.y,
        })
    }

    /// Remove transient viewport chrome and leave the shell prompt at the
    /// first row after finalized history.
    pub fn prepare_shell_prompt_after_exit(&mut self) -> Result<(), B::Error> {
        if self.viewport_area.width == 0 {
            return Ok(());
        }

        let size = self.size()?;
        if size.height == 0 {
            return Ok(());
        }

        let prompt = Position {
            x: 0,
            y: self.viewport_area.top().min(size.height.saturating_sub(1)),
        };
        self.clear_after_position(prompt)?;
        self.backend.show_cursor()?;
        self.backend.set_cursor_position(prompt)?;
        self.last_parked_cursor = prompt;
        self.backend.flush()
    }

    /// Clear from `position` through the visible screen bottom.
    ///
    /// Issues no explicit trailing flush of its own. What prevents the
    /// input-bar flicker is the caller's synchronized-update window, NOT flush
    /// avoidance: the per-frame draw path brackets this clear between
    /// `?2026h`/`?2026l`, so a 2026-capable terminal defers presentation until
    /// the repaint and a viewport resize never shows a blank region. (The
    /// underlying ratatui cursor-move/clear ops still flush; on terminals
    /// without synchronized-update support the deferral does not apply.) The
    /// removed trailing flush was redundant — the sub-ops already flushed.
    /// Teardown callers that run OUTSIDE a draw frame (e.g.
    /// [`Self::prepare_shell_prompt_after_exit`]) already issue their own flush.
    pub fn clear_after_position(&mut self, position: Position) -> Result<(), B::Error> {
        self.backend.set_cursor_position(position)?;
        self.backend.clear_region(ClearType::CurrentLine)?;
        self.backend.clear_region(ClearType::AfterCursor)?;
        self.last_parked_cursor = position;
        self.invalidate_viewport();
        Ok(())
    }

    /// Insert finalized history rows immediately above the retained viewport.
    ///
    /// `TestBackend` uses ratatui's scrolling-region API so tests can verify
    /// visible behavior. Production crossterm backends use direct VT writes for
    /// inserted cells while preserving this same surface API.
    pub fn insert_history_rows(&mut self, rendered: &HistoryRows) -> Result<u16, B::Error> {
        if rendered.is_empty()
            || self.viewport_area.width == 0
            || rendered.width() != self.viewport_area.width
        {
            self.last_history_insert_stats = HistoryInsertStats::default();
            return Ok(0);
        }

        let rows = rendered.height();
        let previous = self.viewport_area;
        let was_invalidated = self.invalidated;
        let mut buffer_updates = 0usize;
        let mut bytes_written = 0usize;
        let mut draw_elapsed = Duration::default();
        self.move_viewport_down_for_history(rows)?;

        let viewport_top = self.viewport_area.top();
        if rows == 0 || viewport_top == 0 {
            self.last_history_insert_stats = HistoryInsertStats {
                wrapped_rows: rows,
                buffer_updates,
                bytes_written: 0,
                invalidated: was_invalidated,
                build_elapsed: Duration::default(),
                draw_elapsed,
                flush_elapsed: Duration::default(),
            };
            return Ok(0);
        }

        let mut start_row = 0;
        let gap_below_history = viewport_top.saturating_sub(self.history_bottom_y);
        if gap_below_history > 0 {
            let rows_to_draw = rows.min(gap_below_history);
            let target_top = self.history_bottom_y;
            let draw = self.draw_history_rows(rendered.buffer(), 0, rows_to_draw, target_top)?;
            buffer_updates += draw.buffer_updates;
            bytes_written += draw.bytes_written;
            draw_elapsed += draw.elapsed;
            self.history_bottom_y = self.history_bottom_y.saturating_add(rows_to_draw);
            start_row = rows_to_draw;
        }

        while start_row < rows {
            let chunk_rows = (rows - start_row).min(viewport_top);
            self.backend.scroll_region_up(0..viewport_top, chunk_rows)?;
            let target_top = viewport_top - chunk_rows;
            let draw =
                self.draw_history_rows(rendered.buffer(), start_row, chunk_rows, target_top)?;
            buffer_updates += draw.buffer_updates;
            bytes_written += draw.bytes_written;
            draw_elapsed += draw.elapsed;
            start_row += chunk_rows;
        }
        // Re-park the cursor from app-tracked state with an absolute move.
        // The row writes (and the DECSTBM-based scrolls, which home the
        // cursor) left it inside finalized history; restoring from memory
        // instead of a DECSC/DECRC bracket keeps the terminal's shared save
        // register untouched and the restore deterministic across terminals.
        self.backend.set_cursor_position(self.last_parked_cursor)?;
        let flush_start = self.perf_stats_enabled.then(Instant::now);
        self.backend.flush()?;
        let flush_elapsed = flush_start.map(|start| start.elapsed()).unwrap_or_default();
        self.history_bottom_y = viewport_top;
        self.note_history_rows_inserted(rows);
        tracing::debug!(
            target: "tui::surface",
            rows,
            previous_viewport = ?previous,
            next_viewport = ?self.viewport_area,
            history_bottom_y = self.history_bottom_y,
            visible_history_rows = self.visible_history_rows,
            "insert history rows"
        );
        self.invalidate_viewport();
        self.last_history_insert_stats = HistoryInsertStats {
            wrapped_rows: rows,
            buffer_updates,
            bytes_written,
            invalidated: was_invalidated,
            build_elapsed: Duration::default(),
            draw_elapsed,
            flush_elapsed,
        };
        Ok(rows)
    }

    fn draw_history_rows(
        &mut self,
        rendered: &Buffer,
        source_start_row: u16,
        row_count: u16,
        target_top: u16,
    ) -> Result<HistoryRowsDraw, B::Error> {
        let draw_start = self.perf_stats_enabled.then(Instant::now);
        if let Some(bytes_written) = self.backend.insert_history_rows_direct(
            rendered,
            source_start_row,
            row_count,
            target_top,
            &mut self.history_row_scratch,
        )? {
            let elapsed = draw_start.map(|start| start.elapsed()).unwrap_or_default();
            return Ok(HistoryRowsDraw {
                buffer_updates: 0,
                bytes_written,
                elapsed,
            });
        }

        let updates = drawable_cell_indices(rendered)
            .into_iter()
            .filter_map(|index| {
                let cell = &rendered.content[index];
                let (x, y) = rendered.pos_of(index);
                if y >= source_start_row && y < source_start_row + row_count {
                    Some((x, target_top + (y - source_start_row), cell))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        let buffer_updates = updates.len();
        self.backend.draw(updates.into_iter())?;
        let elapsed = draw_start.map(|start| start.elapsed()).unwrap_or_default();
        Ok(HistoryRowsDraw {
            buffer_updates,
            bytes_written: 0,
            elapsed,
        })
    }

    fn move_viewport_down_for_history(&mut self, rows: u16) -> Result<(), B::Error> {
        if rows == 0 {
            return Ok(());
        }
        let screen_size = self.last_known_screen_size;
        let available_below = screen_size
            .height
            .saturating_sub(self.viewport_area.bottom());
        let scroll_amount = rows.min(available_below);
        if scroll_amount == 0 {
            return Ok(());
        }

        let region_top = self.viewport_area.top();
        self.backend
            .scroll_region_down(region_top..screen_size.height, scroll_amount)?;
        self.viewport_area.y = self.viewport_area.y.saturating_add(scroll_amount);
        self.buffers[0].resize(self.viewport_area);
        self.buffers[1].resize(self.viewport_area);
        self.invalidate_viewport();
        Ok(())
    }

    pub fn begin_synchronized_update(&mut self) -> Result<(), B::Error> {
        self.backend.begin_synchronized_update()
    }

    pub fn end_synchronized_update(&mut self) -> Result<(), B::Error> {
        self.backend.end_synchronized_update()
    }

    /// Draw one retained viewport frame and apply the frame's cursor claim.
    pub fn draw_viewport<F>(&mut self, render: F) -> Result<(), B::Error>
    where
        F: FnOnce(&mut SurfaceFrame<'_>),
    {
        self.autoresize()?;
        let viewport_area = self.viewport_area;
        let render_start = self.perf_stats_enabled.then(Instant::now);
        let current = self.current_buffer_mut();
        current.reset();
        let mut frame = SurfaceFrame {
            viewport_area,
            buffer: current,
            cursor_claim: None,
        };
        render(&mut frame);
        let render_elapsed = render_start
            .map(|start| start.elapsed())
            .unwrap_or_default();
        let cursor_claim = frame.cursor_claim;

        let was_invalidated = self.invalidated;
        let diff_start = self.perf_stats_enabled.then(Instant::now);
        let updates = self.buffer_updates();
        let diff_elapsed = diff_start.map(|start| start.elapsed()).unwrap_or_default();
        let draw_start = self.perf_stats_enabled.then(Instant::now);
        self.backend
            .draw(updates.iter().map(|(x, y, cell)| (*x, *y, cell)))?;
        let draw_elapsed = draw_start.map(|start| start.elapsed()).unwrap_or_default();
        self.apply_cursor_claim(cursor_claim)?;
        let flush_start = self.perf_stats_enabled.then(Instant::now);
        self.backend.flush()?;
        let flush_elapsed = flush_start.map(|start| start.elapsed()).unwrap_or_default();
        self.last_viewport_draw_stats = ViewportDrawStats {
            buffer_updates: updates.len(),
            invalidated: was_invalidated,
            render_elapsed,
            diff_elapsed,
            draw_elapsed,
            flush_elapsed,
        };
        self.swap_buffers();
        self.invalidated = false;
        Ok(())
    }

    fn autoresize(&mut self) -> Result<(), B::Error> {
        let screen_size = self.backend.size()?;
        self.note_observed_screen_size(screen_size);
        Ok(())
    }

    fn note_observed_screen_size(&mut self, size: Size) {
        if self.last_known_screen_size != size {
            self.last_known_screen_size = size;
            self.invalidate_viewport();
        }
    }

    fn apply_cursor_claim(&mut self, claim: Option<CursorClaim>) -> Result<(), B::Error> {
        if let Some(claim) = claim {
            self.backend.set_cursor_style(claim.style)?;
            self.backend.show_cursor()?;
            self.backend.set_cursor_position(claim.position)?;
            self.last_parked_cursor = claim.position;
        } else {
            self.backend.hide_cursor()?;
            let park = Position { x: 0, y: 0 };
            self.backend.set_cursor_position(park)?;
            self.last_parked_cursor = park;
        }
        Ok(())
    }

    fn buffer_updates(&self) -> Vec<(u16, u16, Cell)> {
        let current = self.current_buffer();
        let previous = self.previous_buffer();
        let mut updates = Vec::new();
        for y in current.area.y..current.area.bottom() {
            let mut invalidated = 0usize;
            let mut to_skip = 0usize;
            for x in current.area.x..current.area.right() {
                let index = current.index_of(x, y);
                let next = &current.content[index];
                let prev = &previous.content[index];
                if !next.skip
                    && to_skip == 0
                    && (self.invalidated || next != prev || invalidated > 0)
                {
                    let (x, y) = current.pos_of(index);
                    updates.push((x, y, next.clone()));
                }

                to_skip = display_width(next.symbol()).saturating_sub(1);
                let affected_width = display_width(next.symbol()).max(display_width(prev.symbol()));
                invalidated = affected_width.max(invalidated).saturating_sub(1);
            }
        }
        updates
    }

    fn current_buffer(&self) -> &Buffer {
        &self.buffers[self.current]
    }

    fn current_buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffers[self.current]
    }

    fn previous_buffer(&self) -> &Buffer {
        &self.buffers[1 - self.current]
    }

    fn previous_buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffers[1 - self.current]
    }

    fn swap_buffers(&mut self) {
        self.current = 1 - self.current;
        self.current_buffer_mut().reset();
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct HistoryRowsDraw {
    buffer_updates: usize,
    bytes_written: usize,
    elapsed: Duration,
}

fn drawable_cell_indices(buffer: &Buffer) -> Vec<usize> {
    let mut indices = Vec::new();
    for y in buffer.area.y..buffer.area.bottom() {
        let mut to_skip = 0usize;
        for x in buffer.area.x..buffer.area.right() {
            let index = buffer.index_of(x, y);
            let cell = &buffer.content[index];
            if !cell.skip && to_skip == 0 {
                indices.push(index);
            }
            to_skip = display_width(cell.symbol()).saturating_sub(1);
        }
    }
    indices
}

fn display_width(symbol: &str) -> usize {
    UnicodeWidthStr::width(symbol).max(1)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CellStyleKey {
    fg: Color,
    bg: Color,
    modifier: Modifier,
}

impl From<&Cell> for CellStyleKey {
    fn from(cell: &Cell) -> Self {
        Self {
            fg: cell.fg,
            bg: cell.bg,
            modifier: cell.modifier,
        }
    }
}

fn push_ansi_style_prefix(out: &mut String, style: CellStyleKey) {
    out.push_str("\x1b[0");
    push_color_code(out, style.fg, ColorLayer::Foreground);
    push_color_code(out, style.bg, ColorLayer::Background);
    let modifiers = style.modifier;
    if modifiers.contains(Modifier::BOLD) {
        out.push_str(";1");
    }
    if modifiers.contains(Modifier::DIM) {
        out.push_str(";2");
    }
    if modifiers.contains(Modifier::ITALIC) {
        out.push_str(";3");
    }
    if modifiers.contains(Modifier::UNDERLINED) {
        out.push_str(";4");
    }
    if modifiers.contains(Modifier::SLOW_BLINK) {
        out.push_str(";5");
    }
    if modifiers.contains(Modifier::RAPID_BLINK) {
        out.push_str(";6");
    }
    if modifiers.contains(Modifier::REVERSED) {
        out.push_str(";7");
    }
    if modifiers.contains(Modifier::HIDDEN) {
        out.push_str(";8");
    }
    if modifiers.contains(Modifier::CROSSED_OUT) {
        out.push_str(";9");
    }
    out.push('m');
}

#[derive(Debug, Clone, Copy)]
enum ColorLayer {
    Foreground,
    Background,
}

fn push_color_code(out: &mut String, color: Color, layer: ColorLayer) {
    let base = match layer {
        ColorLayer::Foreground => 30,
        ColorLayer::Background => 40,
    };
    let bright_base = match layer {
        ColorLayer::Foreground => 90,
        ColorLayer::Background => 100,
    };
    match color {
        Color::Reset => {}
        Color::Black => push_sgr_number(out, base),
        Color::Red => push_sgr_number(out, base + 1),
        Color::Green => push_sgr_number(out, base + 2),
        Color::Yellow => push_sgr_number(out, base + 3),
        Color::Blue => push_sgr_number(out, base + 4),
        Color::Magenta => push_sgr_number(out, base + 5),
        Color::Cyan => push_sgr_number(out, base + 6),
        Color::Gray | Color::White => push_sgr_number(out, base + 7),
        Color::DarkGray => push_sgr_number(out, bright_base),
        Color::LightRed => push_sgr_number(out, bright_base + 1),
        Color::LightGreen => push_sgr_number(out, bright_base + 2),
        Color::LightYellow => push_sgr_number(out, bright_base + 3),
        Color::LightBlue => push_sgr_number(out, bright_base + 4),
        Color::LightMagenta => push_sgr_number(out, bright_base + 5),
        Color::LightCyan => push_sgr_number(out, bright_base + 6),
        Color::Rgb(r, g, b) => match layer {
            ColorLayer::Foreground => {
                push_extended_color(out, 38, r, g, b);
            }
            ColorLayer::Background => {
                push_extended_color(out, 48, r, g, b);
            }
        },
        Color::Indexed(i) => match layer {
            ColorLayer::Foreground => {
                push_indexed_color(out, 38, i);
            }
            ColorLayer::Background => {
                push_indexed_color(out, 48, i);
            }
        },
    }
}

fn push_sgr_number(out: &mut String, value: u16) {
    out.push(';');
    push_u16(out, value);
}

fn push_extended_color(out: &mut String, prefix: u16, r: u8, g: u8, b: u8) {
    push_sgr_number(out, prefix);
    out.push_str(";2;");
    push_u16(out, u16::from(r));
    out.push(';');
    push_u16(out, u16::from(g));
    out.push(';');
    push_u16(out, u16::from(b));
}

fn push_indexed_color(out: &mut String, prefix: u16, index: u8) {
    push_sgr_number(out, prefix);
    out.push_str(";5;");
    push_u16(out, u16::from(index));
}

fn push_u16(out: &mut String, mut value: u16) {
    let mut buf = [0u8; 5];
    let mut index = buf.len();
    loop {
        index -= 1;
        buf[index] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    for digit in &buf[index..] {
        out.push(char::from(*digit));
    }
}

#[cfg(test)]
#[path = "terminal.test.rs"]
mod tests;
