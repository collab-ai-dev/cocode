//! Terminal setup and management.
//!
//! Provides terminal initialization/restoration and the [`Tui`] wrapper
//! that manages the native scrollback terminal surface.

use std::fmt;
use std::io::IsTerminal;
use std::io::Stdout;
use std::io::Write;
use std::io::{self};
use std::panic;
use std::sync::OnceLock;

use crossterm::Command;
use crossterm::cursor::MoveToNextLine;
use crossterm::cursor::Show;
use crossterm::event::DisableBracketedPaste;
use crossterm::event::DisableFocusChange;
use crossterm::event::EnableBracketedPaste;
use crossterm::event::EnableFocusChange;
use crossterm::execute;
use crossterm::terminal::LeaveAlternateScreen;
use crossterm::terminal::disable_raw_mode;
use crossterm::terminal::enable_raw_mode;
use ratatui::backend::ClearType;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
#[cfg(any(test, feature = "testing"))]
use ratatui::layout::Size;

use crate::FrameLayout;
use crate::job_control::SuspendContext;
use crate::state::AppState;
use crate::surface::controller::NativeSurfaceController;
use crate::surface::controller::NativeSurfaceFramePlan;
use crate::surface::modal::ModalSurfacePlacement;
use crate::surface::modal::ModalSurfaceState;
use crate::surface::modal::SurfaceFramePlan;
use crate::surface::stream::PreparedStreamAppend;
use crate::surface::viewport::interactive_viewport_desired_height;
use coco_tui_ui::engine::compatibility::TerminalCompatibility;
use coco_tui_ui::engine::seat::SeatDecision;
use coco_tui_ui::engine::seat::SeatInputs;
use coco_tui_ui::engine::seat::ViewportPin;
use coco_tui_ui::engine::terminal::SurfaceBackend;
use coco_tui_ui::engine::terminal::SurfaceTerminal;

/// Type alias for the terminal backend.
pub type TerminalBackend = CrosstermBackend<Stdout>;

/// Type alias for the native surface terminal.
pub(crate) type NativeTerminal = SurfaceTerminal<TerminalBackend>;

pub(crate) const NATIVE_VIEWPORT_MIN_HEIGHT: u16 = 4;
pub(crate) const NATIVE_VIEWPORT_MAX_HEIGHT: u16 = 12;
/// Max rendered rows the *streaming* live tail may occupy in the inline
/// viewport. Bounding it keeps the per-turn growth phase short (≤ this many
/// repaints, once) and the viewport height constant for the rest of the turn —
/// codex keeps its bottom pane fixed for the same reason. The dropped leading
/// rows are NOT lost: they commit to native scrollback at the next markdown
/// boundary and definitively at finalize. Display-only cap (see
/// `SurfaceStreamDriver::prepare`); the markdown commit boundary is untouched.
pub(crate) const STREAMING_LIVE_TAIL_CAP: u16 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(test)]
struct EnableAlternateScroll;

#[cfg(test)]
impl Command for EnableAlternateScroll {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        write!(f, "\x1b[?1007h")
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> io::Result<()> {
        Err(io::Error::other(
            "tried to execute EnableAlternateScroll using WinAPI; use ANSI instead",
        ))
    }

    #[cfg(windows)]
    fn is_ansi_code_supported(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DisableAlternateScroll;

impl Command for DisableAlternateScroll {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        write!(f, "\x1b[?1007l")
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> io::Result<()> {
        Err(io::Error::other(
            "tried to execute DisableAlternateScroll using WinAPI; use ANSI instead",
        ))
    }

    #[cfg(windows)]
    fn is_ansi_code_supported(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone)]
pub struct TuiDrawOutcome {
    pub layout: FrameLayout,
    pub retained_surface_visible: bool,
    pub attention_requested: bool,
}

/// Enable the TUI-private terminal modes (raw mode, bracketed paste,
/// focus-change reporting, and the kitty keyboard-enhancement push).
///
/// Shared by [`setup_terminal`] (initial install) and
/// [`crate::job_control::SuspendContext::suspend`] (re-arm after SIGCONT).
/// Idempotent at the terminal level: re-issuing the same escape sequences
/// while already in raw mode is a no-op. (The enhancement push/pop is a
/// stack, but every `enter` is paired with a `leave` pop on the suspend and
/// exit paths, so the stack depth stays at one.)
pub(crate) fn enter_tui_modes(stdout: &mut Stdout) -> io::Result<()> {
    enable_raw_mode()?;
    execute!(stdout, EnableBracketedPaste, EnableFocusChange)?;
    crate::keyboard_modes::enable_keyboard_enhancement();
    Ok(())
}

/// Disable TUI-private terminal modes and leave alt-screen if an state had
/// entered it. `LeaveAlternateScreen` is intentionally idempotent here so panic
/// cleanup and suspend/external-process paths share one terminal reset.
pub(crate) fn leave_tui_modes() -> io::Result<()> {
    crate::keyboard_modes::restore_keyboard_enhancement_stack();
    disable_raw_mode()?;
    execute!(
        io::stdout(),
        DisableAlternateScroll,
        LeaveAlternateScreen,
        DisableBracketedPaste,
        DisableFocusChange,
    )?;
    Ok(())
}

/// Set up the terminal for TUI mode.
///
/// Enables raw mode, bracketed paste, and focus-change reporting. The normal
/// surface stays in the main terminal buffer so finalized history can be
/// inserted into native scrollback. Alt-screen is entered only for state
/// surfaces that explicitly request it.
///
/// Panic hook install is idempotent across repeated [`setup_terminal`]
/// calls (e.g. tests that build and drop multiple Tui instances).
pub(crate) fn setup_terminal() -> io::Result<NativeTerminal> {
    if !io::stdin().is_terminal() {
        return Err(io::Error::other("stdin is not a terminal"));
    }
    if !io::stdout().is_terminal() {
        return Err(io::Error::other("stdout is not a terminal"));
    }

    let mut stdout = io::stdout();
    enter_tui_modes(&mut stdout)?;

    install_panic_hook_once();

    let backend = CrosstermBackend::new(stdout);
    SurfaceTerminal::new(backend)
}

/// Restore the terminal to its original state — leaves alt-screen and
/// disables the modes [`enter_tui_modes`] installed.
///
/// Process-exit/panic path, so it adds the hard keyboard-reporting reset
/// (`CSI < u`) on top of the stack pop: the parent shell must never inherit
/// enhanced key reporting even if a terminal missed the pop.
pub fn restore_terminal() -> io::Result<()> {
    leave_tui_modes()?;
    crate::keyboard_modes::reset_keyboard_reporting_after_exit();
    Ok(())
}

/// Install the panic hook exactly once across the lifetime of the
/// process. `setup_terminal` may be called multiple times (e.g. tests
/// that build a `Tui` then drop it), but `panic::take_hook` is global
/// and replacing it twice would chain wrong original handlers.
fn install_panic_hook_once() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        let original_hook = panic::take_hook();
        panic::set_hook(Box::new(move |panic_info| {
            // A panic inside a `PanicRestoreGuard` region (e.g. a contained
            // mermaid-layout panic that the caller `catch_unwind`s and recovers
            // from) must NOT tear down the terminal or print a backtrace — that
            // would corrupt the live render for a fully-recovered panic. Still
            // record it on the (off-screen) trace sink so a swallowed upstream
            // bug stays diagnosable.
            if coco_tui_ui::panic_guard::suppress_panic_restore() {
                tracing::warn!(
                    target: "tui::panic_guard",
                    panic = %panic_info,
                    "contained panic in catch_unwind region — recovering, terminal left intact"
                );
                return;
            }
            let _ = restore_terminal();
            original_hook(panic_info);
        }));
    });
}

/// TUI manager wrapping the native scrollback terminal surface.
pub struct Tui<B: SurfaceBackend = TerminalBackend> {
    terminal: SurfaceTerminal<B>,
    surface: NativeSurfaceController,
    modal_surface: ModalSurfaceState,
    suspend_context: SuspendContext,
    compatibility: TerminalCompatibility,
    alt_screen_active: bool,
    alt_saved_viewport: Option<Rect>,
    main_screen_viewport_pin: ViewportPin,
    restore_terminal_on_drop: bool,
    #[cfg(test)]
    last_geometry_commit: Option<SeatDecision>,
}

impl Tui<TerminalBackend> {
    /// Create a new Tui with a fresh terminal.
    pub fn new() -> io::Result<Self> {
        let terminal = setup_terminal()?;
        Ok(Self::from_terminal(
            terminal,
            TerminalCompatibility::detect(),
            /*restore_terminal_on_drop*/ true,
        ))
    }

    /// Initiate the Ctrl+Z suspend dance. Blocks until SIGCONT delivered
    /// (typically by `fg` in the parent shell), at which point we
    /// re-arm TUI modes and the resume-pending flag is set for the
    /// next [`draw`].
    ///
    /// No-op on non-Unix platforms.
    pub fn trigger_suspend(&mut self) -> io::Result<()> {
        self.leave_modal_alt_screen()?;
        self.suspend_context.suspend()?;
        Ok(())
    }

    /// Leave TUI-private terminal modes before running an interactive
    /// child process such as `$EDITOR`.
    pub fn prepare_external_process(&mut self) -> io::Result<()> {
        let mut stdout = io::stdout();
        self.leave_modal_alt_screen()?;
        leave_tui_modes()?;
        if let Err(err) = execute!(stdout, MoveToNextLine(1), Show) {
            let _ = enter_tui_modes(&mut stdout);
            return Err(err);
        }
        stdout.flush()
    }

    /// Re-enter TUI modes after an external process exits and force the
    /// next frame to repaint the native surface.
    pub fn restore_after_external_process(&mut self) -> io::Result<()> {
        let mut stdout = io::stdout();
        enter_tui_modes(&mut stdout)?;
        self.leave_modal_alt_screen()?;
        self.clear_surface_after_resume()
    }
}

impl<B> Tui<B>
where
    B: SurfaceBackend,
{
    fn from_terminal(
        terminal: SurfaceTerminal<B>,
        compatibility: TerminalCompatibility,
        restore_terminal_on_drop: bool,
    ) -> Self {
        Self {
            terminal,
            surface: NativeSurfaceController::default(),
            modal_surface: ModalSurfaceState::default(),
            suspend_context: SuspendContext::new(),
            compatibility,
            alt_screen_active: false,
            alt_saved_viewport: None,
            main_screen_viewport_pin: ViewportPin::Flowing,
            restore_terminal_on_drop,
            #[cfg(test)]
            last_geometry_commit: None,
        }
    }

    #[cfg(test)]
    fn new_for_test(terminal: SurfaceTerminal<B>, compatibility: TerminalCompatibility) -> Self {
        Self::from_terminal(
            terminal,
            compatibility,
            /*restore_terminal_on_drop*/ false,
        )
    }

    #[cfg(test)]
    fn terminal(&self) -> &SurfaceTerminal<B> {
        &self.terminal
    }

    #[cfg(test)]
    fn last_geometry_commit_for_test(&self) -> Option<SeatDecision> {
        self.last_geometry_commit
    }

    pub(crate) fn native_scrollback_status_message(&self) -> Option<&'static str> {
        self.compatibility.status_message()
    }

    pub(crate) fn retained_surface_visible(&self) -> bool {
        !self.alt_screen_active
    }

    /// Draw one native surface frame.
    pub fn draw(&mut self, state: &AppState) -> Result<TuiDrawOutcome, B::Error> {
        self.draw_with_frame_index(state, 0)
    }

    pub(crate) fn draw_with_frame_index(
        &mut self,
        state: &AppState,
        frame_index: u64,
    ) -> Result<TuiDrawOutcome, B::Error> {
        let perf_config = state.ui.display_settings.performance;
        self.terminal.set_perf_stats_enabled(perf_config.enabled);
        if self.suspend_context.take_resume_pending() {
            self.clear_surface_after_resume()?;
        }

        let plan_started = std::time::Instant::now();
        let size = self.terminal.size()?;
        self.terminal.sync_screen_size(size);
        let now = std::time::Instant::now();
        let plan = self.modal_surface.plan_for_native_viewport(
            state,
            self.compatibility,
            now,
            size.width,
            NATIVE_VIEWPORT_MAX_HEIGHT,
        );
        let plan_elapsed = plan_started.elapsed();
        // Build the interactive live tail exactly once per frame. The sizing
        // pass (`sync_surface_area` → `interactive_viewport_desired_height`)
        // and the paint pass (`render_live_viewport`) both consume it, so we
        // compute it here and thread it through instead of rebuilding twice.
        // This is pure CPU work (no terminal writes) and therefore stays
        // OUTSIDE the synchronized-update window opened below.
        let prepare_started = std::time::Instant::now();
        let native_frame = self
            .surface
            .prepare_native_frame(state, size.width, plan, now);
        let prepare_elapsed = prepare_started.elapsed();
        let stage_elapsed = plan_elapsed + prepare_elapsed;
        if crate::perf::should_log_stage(perf_config, frame_index, stage_elapsed) {
            tracing::debug!(
                target: crate::perf::TARGET,
                frame_index,
                stage = "prepare_native_frame",
                duration_us = crate::perf::duration_us(stage_elapsed),
                plan_us = crate::perf::duration_us(plan_elapsed),
                prepare_us = crate::perf::duration_us(native_frame.prepare_stats.prepare),
                stream_cache_hit = ?native_frame.prepare_stats.stream_cache_hit,
                lines = native_frame.live_lines.as_ref().map_or(0, Vec::len),
                history_append_rows = native_frame.finalized_history.expected_rows(),
                stream_append_rows = native_frame
                    .stream_append
                    .as_ref()
                    .map_or(0, PreparedStreamAppend::expected_rows),
                width = size.width,
                "tui frame stage completed",
            );
        }
        // === One synchronized-update window for the whole paint. ===
        // `?2026h` is emitted BEFORE `sync_surface_area` so the viewport
        // resize/clear/scroll is deferred by the terminal and never presents a
        // blank region between the clear and the repaint (the input-bar
        // flicker). The window brackets the clear, the native history insert,
        // and the viewport draw; the single ESU flush presents the composed
        // frame. ESU is emitted even when the inner draw errors so the terminal
        // never stays stuck in deferred-present.
        // Timed separately: this is a small stdout write, so a slow reading
        // here means terminal backpressure (the kernel pipe is full and the
        // emulator hasn't drained prior frames), not CPU work.
        let bsu_started = std::time::Instant::now();
        self.terminal.begin_synchronized_update()?;
        let bsu_elapsed = bsu_started.elapsed();
        if crate::perf::should_log_stage(perf_config, frame_index, bsu_elapsed) {
            tracing::debug!(
                target: crate::perf::TARGET,
                frame_index,
                stage = "begin_sync_update",
                duration_us = crate::perf::duration_us(bsu_elapsed),
                "tui frame stage completed",
            );
        }
        let drawn = self.draw_native_frame(state, plan, size, native_frame, frame_index);
        let present_start = perf_config.enabled.then(std::time::Instant::now);
        let ended = self.terminal.end_synchronized_update();
        if let Some(start) = present_start {
            let elapsed = start.elapsed();
            if crate::perf::should_log_stage(perf_config, frame_index, elapsed) {
                tracing::debug!(
                    target: crate::perf::TARGET,
                    frame_index,
                    stage = "present_flush",
                    duration_us = crate::perf::duration_us(elapsed),
                    "tui frame stage completed",
                );
            }
        }
        let outcome = match (drawn, ended) {
            (Ok(outcome), Ok(())) => outcome,
            (Err(err), _) | (Ok(_), Err(err)) => return Err(err),
        };
        Ok(TuiDrawOutcome {
            layout: outcome.layout,
            retained_surface_visible: self.retained_surface_visible(),
            attention_requested: plan.attention_requested,
        })
    }

    /// Paint the native surface for one frame: the viewport resize/clear/scroll
    /// (`sync_surface_area`) followed by the history insert + viewport draw.
    ///
    /// Runs entirely inside the caller's synchronized-update window so the clear
    /// and the repaint compose atomically. Returns the surface draw outcome; the
    /// caller always emits ESU regardless of this result. Each stage keeps its
    /// own perf span so the `tui::perf::frame` log breakdown is unchanged.
    fn draw_native_frame(
        &mut self,
        state: &AppState,
        plan: SurfaceFramePlan,
        size: ratatui::layout::Size,
        native_frame: NativeSurfaceFramePlan,
        frame_index: u64,
    ) -> Result<crate::surface::controller::NativeSurfaceDrawOutcome, B::Error> {
        let perf_config = state.ui.display_settings.performance;
        // The live tail is one display row per line, so its length is the
        // precomputed viewport content height for the sizing pass.
        let live_height = native_frame
            .live_lines
            .as_ref()
            .map(|lines| lines.len() as u16);
        // Pass the size read by the caller so the precomputed live tail (built
        // at `size.width`) and the viewport area are derived from one consistent
        // size, even if the terminal resizes mid-frame.
        let sync_start = perf_config.enabled.then(std::time::Instant::now);
        let geometry_commit = self.sync_surface_area(
            state,
            plan,
            size,
            live_height,
            native_frame.guaranteed_append_rows(),
        )?;
        #[cfg(test)]
        {
            self.last_geometry_commit = Some(geometry_commit);
        }
        let sync_elapsed = sync_start.map(|start| start.elapsed());
        if let Some(elapsed) = sync_elapsed
            && crate::perf::should_log_stage(perf_config, frame_index, elapsed)
        {
            tracing::debug!(
                target: crate::perf::TARGET,
                frame_index,
                stage = "sync_surface_area",
                duration_us = crate::perf::duration_us(elapsed),
                width = size.width,
                height = size.height,
                viewport = ?self.terminal.viewport_area(),
                "tui frame stage completed",
            );
        }
        let surface_start = perf_config.enabled.then(std::time::Instant::now);
        let outcome = self.surface.draw_with_plan_at_frame(
            &mut self.terminal,
            state,
            plan,
            native_frame,
            frame_index,
        )?;
        let history_bottom_y_after = self.terminal.history_bottom_y();
        let viewport_top_after = self.terminal.viewport_area().top();
        let unbacked_gap_rows = viewport_top_after.saturating_sub(history_bottom_y_after);
        let seats_flush = self.terminal.seats_flush();
        // Invariant (I-V1): the viewport seats flush on finalized history.
        // Anchored shrinks never open a gap, so there is no pinned-gap
        // exemption — any gap is a stale-anchor / second-writer regression
        // (the /clear-gap class).
        debug_assert!(
            seats_flush,
            "viewport must seat flush against history: pin={:?} viewport_top={} \
             history_bottom_y={} unbacked_gap_rows={} committed={:?}",
            self.main_screen_viewport_pin,
            viewport_top_after,
            history_bottom_y_after,
            unbacked_gap_rows,
            geometry_commit.viewport,
        );
        if !seats_flush {
            tracing::warn!(
                target: "tui::surface::geometry",
                pin = ?self.main_screen_viewport_pin,
                viewport_top = viewport_top_after,
                history_bottom_y = history_bottom_y_after,
                unbacked_gap_rows,
                committed_viewport = ?geometry_commit.viewport,
                "viewport is not seated flush against history"
            );
        }
        // Trace, not debug: this fires unconditionally on EVERY frame (no perf
        // sampling gate), so at debug it dominates any debug-filtered capture.
        tracing::trace!(
            target: "tui::surface::geometry",
            pin = ?self.main_screen_viewport_pin,
            previous_viewport = ?geometry_commit.previous_viewport,
            committed_viewport = ?geometry_commit.viewport,
            terminal_height = size.height,
            history_bottom_y_after,
            unbacked_gap_rows,
            deferred_shrink_rows = geometry_commit.deferred_shrink_rows,
            input_bottom = outcome.layout.input.bottom(),
            viewport_bottom = self.terminal.viewport_area().bottom(),
            "native surface geometry committed"
        );
        let surface_elapsed = surface_start.map(|start| start.elapsed());
        if let Some(elapsed) = surface_elapsed
            && crate::perf::should_log_stage(perf_config, frame_index, elapsed)
        {
            tracing::debug!(
                target: crate::perf::TARGET,
                frame_index,
                stage = "native_surface_draw",
                duration_us = crate::perf::duration_us(elapsed),
                viewport_updates = self.terminal.last_viewport_draw_stats().buffer_updates,
                history_rows = self.terminal.last_history_insert_stats().wrapped_rows,
                "tui frame stage completed",
            );
        }
        Ok(outcome)
    }

    /// Clear the terminal.
    pub fn clear(&mut self) -> Result<(), B::Error> {
        self.terminal.clear_owned_scrollback()?;
        self.surface.reset();
        self.main_screen_viewport_pin = ViewportPin::Flowing;
        Ok(())
    }

    /// Get terminal size.
    pub fn size(&self) -> Result<ratatui::layout::Size, B::Error> {
        self.terminal.size()
    }

    fn clear_surface_after_resume(&mut self) -> Result<(), B::Error> {
        self.terminal.clear_owned_scrollback()?;
        self.surface.reset();
        self.main_screen_viewport_pin = ViewportPin::Flowing;
        Ok(())
    }

    fn sync_surface_area(
        &mut self,
        state: &AppState,
        plan: SurfaceFramePlan,
        size: ratatui::layout::Size,
        precomputed_live_height: Option<u16>,
        guaranteed_append_rows: u16,
    ) -> Result<SeatDecision, B::Error> {
        let wants_alt = matches!(plan.modal_placement, Some(ModalSurfacePlacement::AltScreen));

        if wants_alt && !self.alt_screen_active {
            self.alt_saved_viewport = Some(self.terminal.viewport_area());
            self.terminal.backend_mut().enter_modal_alt_screen()?;
            self.alt_screen_active = true;
            self.terminal.backend_mut().clear_region(ClearType::All)?;
            self.terminal.invalidate_viewport();
        } else if !wants_alt && self.alt_screen_active {
            self.leave_modal_alt_screen()?;
        }

        if self.alt_screen_active {
            // Overlay policy stays in the shell (tui-v2 §6.3): an alt-screen
            // frame covers the whole screen and does not seat — the
            // main-screen pin bookkeeping is untouched.
            let previous_viewport = self.terminal.viewport_area();
            let history_bottom_y_before = self.terminal.history_bottom_y();
            let decision = SeatDecision {
                pin: self.main_screen_viewport_pin,
                previous_viewport,
                viewport: Rect::new(0, 0, size.width, size.height),
                deferred_shrink_rows: 0,
            };
            apply_native_viewport_commit(
                &mut self.terminal,
                decision,
                history_bottom_y_before,
                size.height,
                self.alt_screen_active,
            )?;
            return Ok(decision);
        }

        sync_main_surface_area(
            &mut self.terminal,
            &mut self.main_screen_viewport_pin,
            state,
            plan,
            size,
            precomputed_live_height,
            guaranteed_append_rows,
        )
    }

    fn leave_modal_alt_screen(&mut self) -> Result<(), B::Error> {
        if self.alt_screen_active {
            self.terminal.backend_mut().leave_modal_alt_screen()?;
            self.alt_screen_active = false;
        }
        if let Some(saved) = self.alt_saved_viewport.take() {
            self.terminal.set_viewport_area(saved);
            self.terminal.invalidate_viewport();
        }
        Ok(())
    }
}

fn sync_main_surface_area<B>(
    terminal: &mut SurfaceTerminal<B>,
    main_screen_viewport_pin: &mut ViewportPin,
    state: &AppState,
    plan: SurfaceFramePlan,
    size: ratatui::layout::Size,
    precomputed_live_height: Option<u16>,
    guaranteed_append_rows: u16,
) -> Result<SeatDecision, B::Error>
where
    B: SurfaceBackend,
{
    let history_bottom_y_before = terminal.history_bottom_y();
    let max_h = interactive_viewport_max_height(state, size.height);
    let desired_height = interactive_viewport_desired_height(
        state,
        size.width,
        max_h,
        plan,
        precomputed_live_height,
    );
    // The seat/pin decision is engine-owned (tui-v2 §6.3): the shell supplies
    // intent (desired height, policy bounds, append backing); the engine
    // anchors on its owned viewport top. Shrinks keep the top anchored (codex
    // semantics); a shrink while seated at the screen bottom commits only its
    // append-backed rows and defers the rest so the bottom-aligned composer
    // never lifts off the screen bottom.
    let decision = terminal.seat_viewport(SeatInputs {
        screen: size,
        desired_height,
        min_height: NATIVE_VIEWPORT_MIN_HEIGHT,
        max_height: max_h,
        guaranteed_append_rows,
    });
    *main_screen_viewport_pin = decision.pin;
    apply_native_viewport_commit(
        terminal,
        decision,
        history_bottom_y_before,
        size.height,
        /*alt_screen_active*/ false,
    )?;
    Ok(decision)
}

fn apply_native_viewport_commit<B>(
    terminal: &mut SurfaceTerminal<B>,
    decision: SeatDecision,
    history_bottom_y_before: u16,
    terminal_height: u16,
    alt_screen_active: bool,
) -> Result<(), B::Error>
where
    B: SurfaceBackend,
{
    if terminal.viewport_area() != decision.viewport {
        tracing::debug!(
            target: "tui::surface",
            previous = ?terminal.viewport_area(),
            next = ?decision.viewport,
            viewport_height = decision.viewport.height,
            viewport_bottom = decision.viewport.bottom(),
            terminal_height,
            history_bottom_y_before,
            history_bottom_y = terminal.history_bottom_y(),
            alt_screen_active,
            bottom_pinned = decision.viewport.bottom() == terminal_height,
            pin = ?decision.pin,
            "sync surface area"
        );
        terminal.apply_viewport_area(decision.viewport, !alt_screen_active)?;
    }
    Ok(())
}

#[cfg(test)]
fn draw_native_frame_for_test<B>(
    terminal: &mut SurfaceTerminal<B>,
    surface: &mut NativeSurfaceController,
    main_screen_viewport_pin: &mut ViewportPin,
    state: &AppState,
    plan: SurfaceFramePlan,
    size: Size,
    native_frame: NativeSurfaceFramePlan,
) -> Result<SeatDecision, B::Error>
where
    B: SurfaceBackend,
{
    let live_height = native_frame
        .live_lines
        .as_ref()
        .map(|lines| lines.len() as u16);
    let decision = sync_main_surface_area(
        terminal,
        main_screen_viewport_pin,
        state,
        plan,
        size,
        live_height,
        native_frame.guaranteed_append_rows(),
    )?;
    surface.draw_with_plan_at_frame(terminal, state, plan, native_frame, 0)?;
    assert!(terminal.seats_flush());
    Ok(decision)
}

impl<B> Drop for Tui<B>
where
    B: SurfaceBackend,
{
    fn drop(&mut self) {
        // Teardown leaves a modal alt-screen ONLY if one is active, then
        // disables the TUI input modes, then parks the shell prompt. The
        // main session never enters the alt screen (native scrollback lives
        // in the primary buffer), so `leave_terminal_modes` deliberately does
        // NOT emit `LeaveAlternateScreen` (`CSI ?1049l`): an unpaired one
        // performs a DECRC onto whatever the shared save register holds
        // (coco never writes `\x1b7`; prior programs may have), yanking the
        // cursor to a stale position so the resume hint printed next
        // overprints the transcript. codex's
        // `restore_common` omits the alt-screen leave for the same reason;
        // the modal case is handled by `leave_modal_alt_screen` below, which
        // only fires when a modal alt-screen is actually active.
        //
        // The mode-restore *escapes* go through the surface backend
        // (`leave_terminal_modes`) instead of a free `execute!(io::stdout())`, so
        // they interleave deterministically with the prompt placement and the
        // trailing newline — no shared-global-stdout ordering assumption, and the
        // sequence is unit-testable on a recording backend. Only the non-sink
        // global state stays a free call (below), gated by
        // `restore_terminal_on_drop`.
        let _ = self.leave_modal_alt_screen();
        let _ = self.terminal.backend_mut().leave_terminal_modes();
        if self.restore_terminal_on_drop {
            // Process-global, non-sink terminal state: raw-mode termios and the
            // kitty keyboard-enhancement stack / reporting reset. Cursor-neutral,
            // so its position in the sequence is irrelevant; skipped in tests,
            // which own no real terminal. The panic path restores the same state
            // via `restore_terminal`.
            crate::keyboard_modes::restore_keyboard_enhancement_stack();
            let _ = disable_raw_mode();
            crate::keyboard_modes::reset_keyboard_reporting_after_exit();
        }
        let _ = self.terminal.prepare_shell_prompt_after_exit();
        // zsh shows PROMPT_EOL_MARK (`%`) when the command's final output
        // does not end in a newline; emit it last so nothing follows it.
        let _ = self.terminal.backend_mut().write_drop_trailing_newline();
    }
}

/// Max inline-viewport height for this frame. Streaming/idle is capped at
/// [`NATIVE_VIEWPORT_MAX_HEIGHT`] so finalized history stays visible, but while
/// an interactive prompt (AskUserQuestion / permission) is active the viewport
/// may grow to nearly the full screen so all options fit — the viewport only
/// grows to the prompt's actual desired height, so small prompts stay small and
/// large ones push history up into scrollback. Mirrors codex's bottom pane,
/// which sizes to content rather than a fixed cap.
fn interactive_viewport_max_height(state: &AppState, screen_height: u16) -> u16 {
    if state.ui.interaction.active_prompt.is_some() {
        screen_height
            .saturating_sub(NATIVE_VIEWPORT_MIN_HEIGHT)
            .max(NATIVE_VIEWPORT_MAX_HEIGHT)
            .min(screen_height)
    } else {
        NATIVE_VIEWPORT_MAX_HEIGHT
    }
}

/// Seat rect for a bare anchor with no overflow backing — thin shell wrapper
/// over the engine helper with this shell's height-policy constants applied.
#[cfg(any(test, feature = "testing"))]
pub(crate) fn native_viewport_area_with_max(
    anchor_y: u16,
    size: Size,
    desired_height: u16,
    max_height: u16,
) -> Rect {
    coco_tui_ui::engine::seat::seat_viewport_area(
        anchor_y,
        size,
        desired_height,
        NATIVE_VIEWPORT_MIN_HEIGHT,
        max_height,
    )
}

#[cfg(test)]
#[path = "terminal.test.rs"]
mod tests;
