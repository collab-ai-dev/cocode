//! Frame-level seating tests (I-V4): drive the full `sync → commit → tail
//! fill → history emission → viewport draw` path through the backend-generic
//! harness. The pure seat/pin/shrink math lives in
//! `coco_tui_ui::engine::seat` and is tested there; these tests pin the
//! shell↔engine integration where the C1 and A4 regressions lived.

use std::cell::RefCell;
use std::convert::Infallible;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use coco_messages::AssistantContent;
use coco_messages::TextContent;
use coco_messages::create_assistant_message;
use coco_tui_ui::engine::compatibility::TerminalCompatibility;
use coco_types::TokenUsage;
use crossterm::Command as _;
use pretty_assertions::assert_eq;
use ratatui::backend::Backend;
use ratatui::backend::ClearType;
use ratatui::backend::TestBackend;
use ratatui::backend::WindowSize;
use ratatui::buffer::Buffer;
use ratatui::buffer::Cell;
use ratatui::layout::Position;
use ratatui::layout::Rect;
use ratatui::layout::Size;

use crate::state::ModalState;
use crate::surface::modal::HistorySurfaceMode;
use coco_tui_ui::engine::terminal::SurfaceBackend;
use coco_tui_ui::engine::terminal::SurfaceTerminal;

use super::*;

#[test]
fn interactive_viewport_max_height_grows_for_active_prompt() {
    use crate::state::PanePromptState;
    use crate::state::PlanEntryPromptState;
    let mut state = crate::state::AppState::new();
    // No prompt: the streaming/idle cap.
    assert_eq!(
        interactive_viewport_max_height(&state, 60),
        NATIVE_VIEWPORT_MAX_HEIGHT
    );
    // Active prompt: grows to nearly the full screen so all options fit.
    state
        .ui
        .push_prompt(PanePromptState::PlanEntry(PlanEntryPromptState {
            description: "x".into(),
        }));
    assert_eq!(
        interactive_viewport_max_height(&state, 60),
        60 - NATIVE_VIEWPORT_MIN_HEIGHT
    );
    // Never below the normal cap; clamped to the screen on tiny terminals.
    assert_eq!(interactive_viewport_max_height(&state, 10), 10);
}

#[test]
fn native_frame_overflow_shrink_defers_without_duplication() {
    // The permission-prompt regression at frame level: history overflows the
    // screen, a tall prompt closes and the desired height shrinks with
    // nothing to append. The old seat jumped to the screen bottom and
    // back-filled the freed rows from the history tail cache — duplicating
    // rows still visible above the gap. The seat now DEFERS the unbacked
    // shrink: the viewport keeps its seat (bottom stays on the screen
    // bottom, so the composer never bounces), the surplus height renders as
    // blank filler, and nothing ever repaints history.
    let width = 40;
    let height = 24;
    let size = Size::new(width, height);
    let plan = native_history_plan();
    let mut state = AppState::new();
    push_history_messages(&mut state, 40);

    let backend = TestBackend::new(width, height);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.sync_screen_size(size);
    terminal.set_viewport_area(Rect::new(0, 14, width, 10));
    let mut surface = NativeSurfaceController::default();

    let initial_frame = surface.prepare_native_frame(&state, width, plan, Instant::now());
    surface
        .draw_with_plan_at_frame(&mut terminal, &state, plan, initial_frame, 0)
        .expect("initial draw");
    assert_eq!(terminal.history_bottom_y(), 14);

    let native_frame = surface.prepare_native_frame(&state, width, plan, Instant::now());
    let mut pin = ViewportPin::BottomPinned;
    let decision = draw_native_frame_for_test(
        &mut terminal,
        &mut surface,
        &mut pin,
        &state,
        plan,
        size,
        native_frame,
    )
    .expect("shrink frame");

    assert_eq!(pin, ViewportPin::BottomPinned);
    assert_eq!(terminal.viewport_area(), Rect::new(0, 14, width, 10));
    assert_eq!(terminal.history_bottom_y(), 14);
    assert_eq!(decision.viewport, Rect::new(0, 14, width, 10));
    assert_eq!(decision.deferred_shrink_rows, 5);
    // No history row appears twice on screen (the duplication signature).
    let history_rows = (0..14)
        .map(|y| buffer_row(terminal.backend().buffer(), y))
        .map(|row| row.trim_end().to_string())
        .filter(|row| !row.is_empty())
        .collect::<Vec<_>>();
    let mut deduped = history_rows.clone();
    deduped.sort();
    deduped.dedup();
    assert_eq!(
        deduped.len(),
        history_rows.len(),
        "history rows duplicated on screen: {history_rows:?}"
    );
}

#[test]
fn sync_main_surface_uses_restored_inline_viewport_baseline() {
    let width = 80;
    let height = 24;
    let size = Size::new(width, height);
    let plan = native_history_plan();
    let state = AppState::new();
    let backend = TestBackend::new(width, height);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.sync_screen_size(size);
    // 19 history rows + a 5-row inline viewport (composer 3 + 2-row status bar:
    // mode label + cycle hint) exactly fill the 24-row screen — the same
    // history-adjacent exact fit the test was built around, shifted by the
    // taller baseline status bar so the restored baseline stays stable.
    terminal.set_viewport_area(Rect::new(0, 19, width, 5));
    terminal.note_history_rows_inserted(19);
    let mut surface = NativeSurfaceController::default();
    let native_frame = surface.prepare_native_frame(&state, width, plan, Instant::now());
    let mut pin = ViewportPin::BottomPinned;

    let decision = sync_main_surface_area(
        &mut terminal,
        &mut pin,
        &state,
        plan,
        size,
        native_frame
            .live_lines
            .as_ref()
            .map(|lines| lines.len() as u16),
        native_frame.guaranteed_append_rows(),
    )
    .expect("sync");

    assert_eq!(decision.previous_viewport, Rect::new(0, 19, width, 5));
    assert_eq!(decision.viewport, Rect::new(0, 19, width, 5));
}

#[test]
fn tui_alt_screen_leave_uses_restored_inline_viewport_baseline() {
    let width = 80;
    let height = 24;
    let size = Size::new(width, height);
    let backend = TestBackend::new(width, height);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.sync_screen_size(size);
    // 19 history rows + a 5-row inline viewport (composer 3 + 2-row status bar)
    // exactly fill the 24-row screen, so the baseline survives the round-trip.
    terminal.set_viewport_area(Rect::new(0, 19, width, 5));
    terminal.note_history_rows_inserted(19);
    let mut tui = Tui::new_for_test(terminal, TerminalCompatibility::NativeScrollback);

    let mut modal_state = AppState::new();
    modal_state.ui.show_modal(ModalState::Help);
    tui.draw(&modal_state).expect("alt-screen draw");
    assert_eq!(
        tui.terminal().viewport_area(),
        Rect::new(0, 0, width, height)
    );

    let main_state = AppState::new();
    tui.draw(&main_state).expect("restore draw");
    let decision = tui
        .last_geometry_commit_for_test()
        .expect("restore frame geometry commit");

    // The pre-modal inline baseline (0,19,5) is preserved across the
    // alt-screen round-trip.
    assert_eq!(decision.previous_viewport, Rect::new(0, 19, width, 5));
    assert_eq!(decision.viewport, Rect::new(0, 19, width, 5));
    assert_eq!(tui.terminal().viewport_area(), Rect::new(0, 19, width, 5));
}

#[test]
fn overflow_shrink_defers_when_unbacked() {
    // Sync-pass half of the prompt-close behavior: an unbacked shrink while
    // seated on the screen bottom defers wholesale — the viewport keeps its
    // seat (no reveal, no bottom lift), and only append-backed rows commit.
    let width = 80;
    let height = 24;
    let size = Size::new(width, height);
    let plan = native_history_plan();
    let state = AppState::new();
    let backend = TestBackend::new(width, height);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.sync_screen_size(size);
    terminal.set_viewport_area(Rect::new(0, 14, width, 10));
    terminal.note_history_rows_inserted(30);
    let mut surface = NativeSurfaceController::default();
    let native_frame = surface.prepare_native_frame(&state, width, plan, Instant::now());
    let mut pin = ViewportPin::BottomPinned;

    let decision = sync_main_surface_area(
        &mut terminal,
        &mut pin,
        &state,
        plan,
        size,
        native_frame
            .live_lines
            .as_ref()
            .map(|lines| lines.len() as u16),
        /*guaranteed_append_rows*/ 0,
    )
    .expect("sync");

    assert_eq!(pin, ViewportPin::BottomPinned);
    assert_eq!(decision.viewport, Rect::new(0, 14, width, 10));
    assert_eq!(decision.deferred_shrink_rows, 5);
    assert_eq!(terminal.viewport_area(), decision.viewport);
}

#[test]
fn short_history_growth_stays_flowing() {
    let width = 80;
    let height = 24;
    let size = Size::new(width, height);
    let plan = native_history_plan();
    let state = AppState::new();
    let backend = TestBackend::new(width, height);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.sync_screen_size(size);
    terminal.set_viewport_area(Rect::new(0, 2, width, 4));
    terminal.note_history_rows_inserted(2);
    let mut pin = ViewportPin::Flowing;

    let decision = sync_main_surface_area(
        &mut terminal,
        &mut pin,
        &state,
        plan,
        size,
        Some(10),
        /*guaranteed_append_rows*/ 0,
    )
    .expect("sync");

    assert_eq!(pin, ViewportPin::Flowing);
    assert_eq!(decision.viewport.top(), 2);
    assert!(decision.viewport.height > 4);
    assert_eq!(terminal.viewport_area(), decision.viewport);
}

#[test]
fn alternate_scroll_commands_emit_xterm_private_mode_bytes() {
    let mut enabled = String::new();
    EnableAlternateScroll
        .write_ansi(&mut enabled)
        .expect("write enable bytes");
    assert_eq!(enabled, "\x1b[?1007h");

    let mut disabled = String::new();
    DisableAlternateScroll
        .write_ansi(&mut disabled)
        .expect("write disable bytes");
    assert_eq!(disabled, "\x1b[?1007l");
}

#[test]
fn drop_teardown_routes_four_steps_through_backend_in_order() {
    // Regression: the exit sequence must leave the alt-screen / restore terminal
    // modes (the `CSI ?1049l` DECRC) BEFORE parking the shell-prompt cursor,
    // else the DECRC yanks the cursor up into finalized history and the resume
    // hint printed next overprints the transcript. All four teardown steps now
    // route through the surface backend, so the order is observable here rather
    // than relying on a shared-global-stdout assumption.
    let width = 80;
    let height = 24;
    let log = TeardownLog::default();
    let backend = RecordingBackend {
        inner: TestBackend::new(width, height),
        log: log.clone(),
    };
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.sync_screen_size(Size::new(width, height));
    // Non-empty viewport so `prepare_shell_prompt_after_exit` runs its body and
    // shows the cursor — our marker for the prompt-placement step.
    terminal.set_viewport_area(Rect::new(0, 6, width, 4));
    let mut tui = Tui::new_for_test(terminal, TerminalCompatibility::NativeScrollback);
    // Force the modal alt-screen leave to emit so step 1 is exercised too.
    tui.alt_screen_active = true;

    drop(tui);

    assert_eq!(
        log.steps(),
        vec![
            "leave_modal_alt_screen",
            "leave_terminal_modes",
            "prepare_shell_prompt",
            "trailing_newline",
        ]
    );
}

fn native_history_plan() -> SurfaceFramePlan {
    SurfaceFramePlan {
        modal_placement: None,
        history_surface: HistorySurfaceMode::NativeScrollback,
        attention_requested: false,
    }
}

fn push_history_messages(state: &mut AppState, count: usize) {
    for index in 0..count {
        let message = create_assistant_message(
            vec![AssistantContent::Text(TextContent::new(format!(
                "history row {index:02}"
            )))],
            "test-model",
            TokenUsage::default(),
        );
        state
            .session
            .transcript
            .on_message_appended(Arc::new(message));
    }
}

fn buffer_row(buffer: &Buffer, y: u16) -> String {
    let mut row = String::new();
    for x in 0..buffer.area.width {
        row.push_str(buffer[(x, y)].symbol());
    }
    row
}

/// Ordered log of the teardown operations `Tui::drop` issues through the
/// backend, so the regression test can pin their sequence.
#[derive(Clone, Debug, Default)]
struct TeardownLog(Rc<RefCell<Vec<&'static str>>>);

impl TeardownLog {
    fn record(&self, step: &'static str) {
        self.0.borrow_mut().push(step);
    }

    fn steps(&self) -> Vec<&'static str> {
        self.0.borrow().clone()
    }
}

/// `TestBackend` wrapper that records the teardown-relevant operations in call
/// order. Render/cursor methods delegate to the inner `TestBackend`;
/// `show_cursor` is the unique marker for `prepare_shell_prompt_after_exit`.
#[derive(Debug)]
struct RecordingBackend {
    inner: TestBackend,
    log: TeardownLog,
}

impl Backend for RecordingBackend {
    type Error = Infallible;

    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.inner.draw(content)
    }

    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        self.log.record("prepare_shell_prompt");
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        self.inner.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> Result<(), Self::Error> {
        self.inner.set_cursor_position(position)
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> Result<(), Self::Error> {
        self.inner.clear_region(clear_type)
    }

    fn size(&self) -> Result<Size, Self::Error> {
        self.inner.size()
    }

    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        self.inner.window_size()
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        self.inner.flush()
    }

    fn scroll_region_up(&mut self, region: Range<u16>, line_count: u16) -> Result<(), Self::Error> {
        self.inner.scroll_region_up(region, line_count)
    }

    fn scroll_region_down(
        &mut self,
        region: Range<u16>,
        line_count: u16,
    ) -> Result<(), Self::Error> {
        self.inner.scroll_region_down(region, line_count)
    }
}

impl SurfaceBackend for RecordingBackend {
    fn leave_modal_alt_screen(&mut self) -> Result<(), Self::Error> {
        self.log.record("leave_modal_alt_screen");
        Ok(())
    }

    fn leave_terminal_modes(&mut self) -> Result<(), Self::Error> {
        self.log.record("leave_terminal_modes");
        Ok(())
    }

    fn write_drop_trailing_newline(&mut self) -> Result<(), Self::Error> {
        self.log.record("trailing_newline");
        Ok(())
    }
}
