//! Native-scrollback draw orchestration.

use std::time::Duration;
use std::time::Instant;

use crate::FrameLayout;
use crate::state::AppState;
use crate::surface::history_driver::HistoryReplayMode;
use crate::surface::history_driver::PreparedFinalizedHistory;
use crate::surface::history_driver::SessionHeader;
use crate::surface::history_driver::SurfaceHistoryDriver;
#[cfg(any(test, feature = "testing"))]
use crate::surface::modal::ModalSurfaceState;
use crate::surface::modal::SurfaceFramePlan;
use crate::surface::stream::PreparedStreamAppend;
use crate::surface::stream::SurfaceStreamDriver;
use crate::surface::viewport::render_interactive_viewport;
use crate::transcript::cells::RenderedCell;
use crate::transcript::emission::HistoryEmissionOutcome;
use crate::transcript::render::HistoryLineRenderOptions;
use crate::widgets::TranscriptLayoutIndex;
#[cfg(any(test, feature = "testing"))]
use coco_tui_ui::engine::compatibility::TerminalCompatibility;
use coco_tui_ui::engine::terminal::HistoryInsertStats;
use coco_tui_ui::engine::terminal::SurfaceBackend;
use coco_tui_ui::engine::terminal::SurfaceTerminal;
use coco_tui_ui::style::UiStyles;
use ratatui::text::Line;

// Deliberately NOT Clone: `stream` holds the single `ScrollbackStreamCommit`
// owner (tui-v2 §6.7-10) — an accidental clone would create the second copy
// that invariant forbids.
#[derive(Debug, Default)]
pub(crate) struct NativeSurfaceController {
    history: SurfaceHistoryDriver,
    stream: SurfaceStreamDriver,
    transcript_layout: TranscriptLayoutIndex,
    history_display: Option<HistoryDisplayState>,
    /// Session header (+ fingerprint) cached by its input key — rebuilt only
    /// when a header input (model/effort/cwd/branch/theme/width/…) changes,
    /// not per frame.
    session_header: Option<(u64, SessionHeader)>,
}

/// Display-mode inputs whose change requires a full history re-render (the
/// committed cells are re-derived). Deliberately excludes reasoning metadata:
/// that is a side-cache read at cell-build time (`history_options`), so the
/// finalize draw bakes it into the assistant cell's single append-only emit —
/// a per-turn metadata attach must NOT force `replay_all_capped`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HistoryDisplayState {
    show_system_reminders: bool,
    show_thinking: bool,
    syntax_highlighting: coco_tui_ui::display::SyntaxHighlighting,
    theme_hash: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct NativeSurfaceDrawOutcome {
    #[cfg(test)]
    pub(crate) history: HistoryEmissionOutcome,
    pub(crate) layout: FrameLayout,
}

#[derive(Debug)]
pub(crate) struct NativeSurfaceFramePlan {
    pub(crate) live_lines: Option<Vec<Line<'static>>>,
    pub(crate) finalized_history: PreparedFinalizedHistory,
    pub(crate) stream_append: Option<PreparedStreamAppend>,
    pub(crate) stream_commit_invalidated: bool,
    pub(crate) prepare_stats: NativePrepareStats,
}

/// Prepare-stage timing collected by [`NativeSurfaceController::prepare_native_frame`]
/// for the `prepare_native_frame` perf stage. The live-stream prepare and the
/// finalized-history prepare run as one pass, so their cost is reported as a
/// single span.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct NativePrepareStats {
    pub(crate) prepare: Duration,
    /// `Some(hit)` when the stream projection ran (`None` for view-mode /
    /// no-stream frames) — verifies the generation cache in production traces.
    pub(crate) stream_cache_hit: Option<bool>,
}

impl NativeSurfaceFramePlan {
    /// Rows this frame's history emission is guaranteed to append — the
    /// backing for a bottom-seated shrink commit (`SeatInputs`).
    pub(crate) fn guaranteed_append_rows(&self) -> u16 {
        let stream_rows = self
            .stream_append
            .as_ref()
            .map_or(0, PreparedStreamAppend::expected_rows);
        self.finalized_history
            .expected_rows()
            .saturating_add(stream_rows)
    }
}

impl NativeSurfaceController {
    pub(crate) fn prepare_native_frame(
        &mut self,
        state: &AppState,
        width: u16,
        plan: SurfaceFramePlan,
        now: Instant,
    ) -> NativeSurfaceFramePlan {
        let prepare_started = Instant::now();
        let prepared_live = (width > 0).then(|| self.stream.prepare(state, width, plan));
        let (live_lines, stream_append, stream_commit_invalidated, stream_cache_hit) =
            match prepared_live {
                Some(prepared) => (
                    Some(prepared.lines),
                    prepared.stream_append,
                    prepared.commit_invalidated,
                    prepared.stream_cache_hit,
                ),
                None => (None, None, false, None),
            };
        let options = history_options(state, width);
        let header_key = crate::presentation::header::header_input_key(
            state,
            UiStyles::new(&state.ui.theme).theme_hash(),
            width,
        );
        if self
            .session_header
            .as_ref()
            .is_none_or(|(key, _)| *key != header_key)
        {
            self.session_header = Some((
                header_key,
                SessionHeader::new(session_header_lines(state, width)),
            ));
        }
        let cells = state.session.transcript.cells();
        let transcript_revision = state.session.transcript.revision();
        let history_display = HistoryDisplayState::from(state);
        let history_display_changed = self
            .history_display
            .as_ref()
            .is_some_and(|previous| *previous != history_display);
        // The anchored finalize reads the SINGLE scrollback commit owned by the
        // stream driver (tui-v2 §6.7-10) — there is no second copy on the
        // history driver to keep in sync.
        let finalized_history = if stream_commit_invalidated {
            PreparedFinalizedHistory::ReplayRequired
        } else if plan.native_history_enabled()
            && !history_display_changed
            && !self.history.replay_due(now)
            && let Some((_, session_header)) = self.session_header.as_ref()
        {
            self.history.prepare_append(
                session_header,
                cells,
                transcript_revision,
                options,
                self.stream.commit(),
            )
        } else {
            PreparedFinalizedHistory::ReplayRequired
        };
        NativeSurfaceFramePlan {
            live_lines,
            finalized_history,
            stream_append,
            stream_commit_invalidated,
            prepare_stats: NativePrepareStats {
                prepare: prepare_started.elapsed(),
                stream_cache_hit,
            },
        }
    }

    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn draw<B>(
        &mut self,
        terminal: &mut SurfaceTerminal<B>,
        state: &AppState,
    ) -> Result<NativeSurfaceDrawOutcome, B::Error>
    where
        B: SurfaceBackend,
    {
        self.draw_at(terminal, state, Instant::now())
    }

    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn draw_with_plan<B>(
        &mut self,
        terminal: &mut SurfaceTerminal<B>,
        state: &AppState,
        plan: SurfaceFramePlan,
        precomputed_live: Option<Vec<Line<'static>>>,
    ) -> Result<NativeSurfaceDrawOutcome, B::Error>
    where
        B: SurfaceBackend,
    {
        // Unit tests have no `Tui` wrapper, so the test helper owns the
        // synchronized-update window itself.
        self.draw_at_with_plan(terminal, state, Instant::now(), plan, precomputed_live, 0)
    }

    /// Production frame entry. The caller (`Tui::draw_with_frame_index`) owns
    /// the single synchronized-update window that also brackets the viewport
    /// resize (`sync_surface_area`), so this path must NOT open its own — a
    /// nested `?2026h`/`?2026l` would emit a premature ESU and present a torn
    /// mid-frame. It therefore calls `draw_at_inner` directly.
    pub(crate) fn draw_with_plan_at_frame<B>(
        &mut self,
        terminal: &mut SurfaceTerminal<B>,
        state: &AppState,
        plan: SurfaceFramePlan,
        native_frame: NativeSurfaceFramePlan,
        frame_index: u64,
    ) -> Result<NativeSurfaceDrawOutcome, B::Error>
    where
        B: SurfaceBackend,
    {
        self.draw_at_inner(
            terminal,
            state,
            Instant::now(),
            plan,
            native_frame,
            frame_index,
        )
    }

    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn draw_at<B>(
        &mut self,
        terminal: &mut SurfaceTerminal<B>,
        state: &AppState,
        now: Instant,
    ) -> Result<NativeSurfaceDrawOutcome, B::Error>
    where
        B: SurfaceBackend,
    {
        let mut modal_state = ModalSurfaceState::default();
        let plan = modal_state.plan(state, TerminalCompatibility::NativeScrollback, now);
        self.draw_at_with_plan(terminal, state, now, plan, None, 0)
    }

    /// Test-only frame entry that owns its synchronized-update window. The
    /// production path goes through `draw_with_plan_at_frame`, where `Tui`
    /// owns the single window (see that method's note); this variant exists so
    /// unit tests can drive a full frame without a `Tui`.
    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn draw_at_with_plan<B>(
        &mut self,
        terminal: &mut SurfaceTerminal<B>,
        state: &AppState,
        now: Instant,
        plan: SurfaceFramePlan,
        precomputed_live: Option<Vec<Line<'static>>>,
        frame_index: u64,
    ) -> Result<NativeSurfaceDrawOutcome, B::Error>
    where
        B: SurfaceBackend,
    {
        let mut native_frame =
            self.prepare_native_frame(state, terminal.viewport_area().width, plan, now);
        if let Some(precomputed_live) = precomputed_live {
            native_frame.live_lines = Some(precomputed_live);
        }
        terminal.begin_synchronized_update()?;
        let outcome = self.draw_at_inner(terminal, state, now, plan, native_frame, frame_index);
        let end = terminal.end_synchronized_update();
        match (outcome, end) {
            (Ok(outcome), Ok(())) => Ok(outcome),
            (Err(err), _) | (Ok(_), Err(err)) => Err(err),
        }
    }

    fn draw_at_inner<B>(
        &mut self,
        terminal: &mut SurfaceTerminal<B>,
        state: &AppState,
        now: Instant,
        plan: SurfaceFramePlan,
        native_frame: NativeSurfaceFramePlan,
        frame_index: u64,
    ) -> Result<NativeSurfaceDrawOutcome, B::Error>
    where
        B: SurfaceBackend,
    {
        let viewport = terminal.viewport_area();
        let width = viewport.width;
        let stream_active = state.is_streaming();
        let perf_config = state.ui.display_settings.performance;
        let mut native_frame = native_frame;
        let mut precomputed_live = native_frame.live_lines.take();
        let mut prepared_stream_append = native_frame.stream_append.take();
        let stream_commit_invalidated = native_frame.stream_commit_invalidated;
        self.history.note_viewport(width, stream_active);

        let options = history_options(state, width);
        let history_display = HistoryDisplayState::from(state);
        let session_header = || session_header_lines(state, width);
        // Feed the native history driver with the engine-authoritative
        // `&[RenderedCell]` slice directly. Engine-pushed content
        // (cancel marker, resume scrollback, hooks, …) flows through
        // `MessageAppended` → `TranscriptView` → `cells()`.
        let cells = state.session.transcript.cells();
        let transcript_revision = state.session.transcript.revision();
        let history_start = perf_config.enabled.then(Instant::now);
        let history = if !plan.native_history_enabled() {
            HistoryEmissionOutcome::Noop
        } else {
            let history_display_changed = self
                .history_display
                .replace(history_display)
                .is_some_and(|previous| previous != history_display);
            let needs_reflow_replay = self.history.replay_due(now);
            if history_display_changed || needs_reflow_replay || stream_commit_invalidated {
                let cause = if history_display_changed {
                    "history_display_changed"
                } else if stream_commit_invalidated {
                    "stream_commit_invalidated"
                } else {
                    "reflow_debounce_due"
                };
                let outcome = self.replay_history(
                    terminal,
                    state,
                    plan,
                    cells,
                    transcript_revision,
                    options,
                    session_header(),
                    HistoryReplayMode {
                        stream_active,
                        cause,
                    },
                    &mut precomputed_live,
                    &mut prepared_stream_append,
                )?;
                tracing::debug!(
                    target: "tui::surface::replay",
                    cause,
                    reflow_due = needs_reflow_replay,
                    history_display_changed,
                    cells = cells.len(),
                    width = viewport.width,
                    height = viewport.height,
                    stream_active,
                    "history full replay",
                );
                outcome
            } else {
                let outcome = self.history.commit_prepared_append(
                    terminal,
                    &native_frame.finalized_history,
                    cells,
                )?;
                if matches!(outcome, HistoryEmissionOutcome::ReplayRequired) {
                    self.replay_history(
                        terminal,
                        state,
                        plan,
                        cells,
                        transcript_revision,
                        options,
                        session_header(),
                        HistoryReplayMode {
                            stream_active,
                            cause: "emitter_replay_required",
                        },
                        &mut precomputed_live,
                        &mut prepared_stream_append,
                    )?
                } else {
                    // The anchored finalize folded the single in-flight commit
                    // into this append; consume it so the next frame does not
                    // re-anchor against rows now part of the finalized message.
                    if let PreparedFinalizedHistory::Append(append) =
                        &native_frame.finalized_history
                        && append.consumed_stream_commit
                        && matches!(outcome, HistoryEmissionOutcome::Appended { .. })
                    {
                        self.stream.consume_commit();
                    }
                    outcome
                }
            }
        };
        let finalized_history_stats = history_insert_stats_for(terminal, &history);
        if plan.native_history_enabled()
            && let Some(stream_append) = prepared_stream_append.as_ref()
        {
            let stream_outcome = self.history.commit_stream_append(terminal, stream_append)?;
            if matches!(stream_outcome, HistoryEmissionOutcome::Appended { .. }) {
                self.stream.mark_stream_append_committed(stream_append);
            }
        }
        let history_elapsed = history_start.map(|start| start.elapsed());
        if let Some(elapsed) = history_elapsed
            && crate::perf::should_log_stage(perf_config, frame_index, elapsed)
        {
            tracing::debug!(
                target: crate::perf::TARGET,
                stage = "history",
                duration_us = crate::perf::duration_us(elapsed),
                history_outcome = history_outcome_name(&history),
                history_fast_noop = matches!(history, HistoryEmissionOutcome::FastNoop { .. }),
                transcript_revision,
                cells = cells.len(),
                rows = history_rows(&history),
                wrapped_rows = finalized_history_stats.wrapped_rows,
                buffer_updates = finalized_history_stats.buffer_updates,
                bytes_written = finalized_history_stats.bytes_written,
                invalidated = finalized_history_stats.invalidated,
                build_us = crate::perf::duration_us(finalized_history_stats.build_elapsed),
                lines_build_us =
                    crate::perf::duration_us(native_frame.finalized_history.lines_build_elapsed()),
                render_us = crate::perf::duration_us(native_frame.finalized_history.render_elapsed()),
                draw_us = crate::perf::duration_us(finalized_history_stats.draw_elapsed),
                flush_us = crate::perf::duration_us(finalized_history_stats.flush_elapsed),
                "tui frame history stage completed",
            );
        }

        let mut layout = FrameLayout::default();
        let viewport_start = perf_config.enabled.then(Instant::now);
        terminal.draw_viewport(|frame| {
            layout = render_interactive_viewport(
                frame,
                state,
                plan,
                &mut self.transcript_layout,
                precomputed_live,
            );
            if let Some(claim) = crate::cursor::compute_cursor(state, layout) {
                frame.set_cursor_claim(claim);
            }
        })?;
        let viewport_elapsed = viewport_start.map(|start| start.elapsed());
        if let Some(elapsed) = viewport_elapsed
            && crate::perf::should_log_stage(perf_config, frame_index, elapsed)
        {
            let stats = terminal.last_viewport_draw_stats();
            tracing::debug!(
                target: crate::perf::TARGET,
                stage = "viewport_draw",
                duration_us = crate::perf::duration_us(elapsed),
                buffer_updates = stats.buffer_updates,
                invalidated = stats.invalidated,
                input_bottom = layout.input.bottom(),
                viewport_bottom = terminal.viewport_area().bottom(),
                render_us = crate::perf::duration_us(stats.render_elapsed),
                diff_us = crate::perf::duration_us(stats.diff_elapsed),
                draw_us = crate::perf::duration_us(stats.draw_elapsed),
                flush_us = crate::perf::duration_us(stats.flush_elapsed),
                "tui frame viewport stage completed",
            );
        }

        Ok(NativeSurfaceDrawOutcome {
            #[cfg(test)]
            history,
            layout,
        })
    }

    /// Clear owned scrollback and re-render the finalized history, then rebuild
    /// the live tail. The single stream commit is invalidated FIRST so the
    /// post-replay live-tail prepare re-emits the full stable prefix — the
    /// leading streamed rows the clear just wiped — instead of only the
    /// increment (the loss half of the former split-watermark bug).
    #[allow(clippy::too_many_arguments)]
    fn replay_history<B>(
        &mut self,
        terminal: &mut SurfaceTerminal<B>,
        state: &AppState,
        plan: SurfaceFramePlan,
        cells: &[RenderedCell],
        transcript_revision: u64,
        options: HistoryLineRenderOptions<'_>,
        session_header: Vec<Line<'static>>,
        mode: HistoryReplayMode,
        precomputed_live: &mut Option<Vec<Line<'static>>>,
        prepared_stream_append: &mut Option<PreparedStreamAppend>,
    ) -> Result<HistoryEmissionOutcome, B::Error>
    where
        B: SurfaceBackend,
    {
        self.stream.invalidate_commit();
        let width = terminal.viewport_area().width;
        let outcome = self.history.replay_all_capped(
            terminal,
            session_header,
            cells,
            transcript_revision,
            options,
            mode,
        )?;
        if state.is_streaming() {
            let prepared = self.stream.prepare(state, width, plan);
            *precomputed_live = Some(prepared.lines);
            *prepared_stream_append = prepared.stream_append;
        }
        Ok(outcome)
    }

    pub(crate) fn reset(&mut self) {
        self.history.reset();
        self.stream.reset();
        self.transcript_layout.reset();
    }
}

fn history_outcome_name(outcome: &HistoryEmissionOutcome) -> &'static str {
    match outcome {
        HistoryEmissionOutcome::Noop => "noop",
        HistoryEmissionOutcome::FastNoop { .. } => "fast_noop",
        HistoryEmissionOutcome::Appended { .. } => "appended",
        HistoryEmissionOutcome::Replayed { .. } => "replayed",
        HistoryEmissionOutcome::ReplayRequired => "replay_required",
    }
}

fn history_rows(outcome: &HistoryEmissionOutcome) -> u16 {
    match outcome {
        HistoryEmissionOutcome::Appended { rows, .. }
        | HistoryEmissionOutcome::Replayed { rows, .. } => *rows,
        HistoryEmissionOutcome::Noop
        | HistoryEmissionOutcome::FastNoop { .. }
        | HistoryEmissionOutcome::ReplayRequired => 0,
    }
}

fn history_insert_stats_for<B>(
    terminal: &SurfaceTerminal<B>,
    outcome: &HistoryEmissionOutcome,
) -> HistoryInsertStats
where
    B: SurfaceBackend,
{
    match outcome {
        HistoryEmissionOutcome::Appended { .. } | HistoryEmissionOutcome::Replayed { .. } => {
            terminal.last_history_insert_stats()
        }
        HistoryEmissionOutcome::Noop
        | HistoryEmissionOutcome::FastNoop { .. }
        | HistoryEmissionOutcome::ReplayRequired => HistoryInsertStats::default(),
    }
}

impl From<&AppState> for HistoryDisplayState {
    fn from(state: &AppState) -> Self {
        Self {
            show_system_reminders: state.ui.show_system_reminders,
            show_thinking: state.ui.show_thinking,
            syntax_highlighting: state.ui.display_settings.syntax_highlighting,
            theme_hash: UiStyles::new(&state.ui.theme).theme_hash(),
        }
    }
}

fn history_options(state: &AppState, width: u16) -> HistoryLineRenderOptions<'_> {
    HistoryLineRenderOptions {
        styles: UiStyles::new(&state.ui.theme),
        width,
        syntax_highlighting: state.ui.display_settings.syntax_highlighting,
        show_system_reminders: state.ui.show_system_reminders,
        show_thinking: state.ui.show_thinking,
        cwd: state.session.working_dir.as_deref(),
        kb_handle: Some(&state.ui.kb_handle),
        replay_cache_policy: state.ui.display_settings.native_replay_cache,
        reasoning_metadata: Some(&state.session.reasoning_metadata),
        subagent_summaries: Some(&state.session.subagent_summaries),
    }
}

fn session_header_lines(state: &AppState, width: u16) -> Vec<ratatui::text::Line<'static>> {
    crate::presentation::header::header_history_lines(state, UiStyles::new(&state.ui.theme), width)
}

#[cfg(test)]
#[path = "controller.test.rs"]
mod tests;
