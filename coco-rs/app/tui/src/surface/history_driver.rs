//! Surface history orchestration for native scrollback.
//!
//! Phase 3d (§4): operates on `&[RenderedCell]` directly. The
//! `HistoryEmissionTracker` still tracks exactly-once emission by
//! engine message UUIDs, which are stable across the engine
//! `MessageAppended` events and survive resume reloads (each cell
//! carries `Arc<Message>` from the engine `MessageHistory`).
use std::time::Duration;
use std::time::Instant;

use ratatui::text::Line;

use crate::surface::line_fingerprint::RenderedLineFingerprint;
use crate::surface::line_fingerprint::fingerprint_lines;
use crate::surface::stream::PreparedStreamAppend;
use crate::transcript::cells::RenderedCell;
use crate::transcript::cells::committable_prefix_len;
use crate::transcript::emission::HistoryEmissionOutcome;
use crate::transcript::emission::HistoryEmissionPlan;
use crate::transcript::emission::HistoryEmissionTracker;
use crate::transcript::emission::finalize_after_stream_prefix;
use crate::transcript::render::DEFAULT_MAX_REFLOW_ROWS;
use crate::transcript::render::HistoryLineRenderOptions;
use crate::transcript::render::HistoryReplayCache;
use crate::transcript::render::render_finalized_history_lines;
use crate::transcript::render::render_replay_history_lines_cached;
use crate::transcript::stream::ScrollbackStreamCommit;
use coco_tui_ui::engine::history_insert::HistoryRows;
use coco_tui_ui::engine::history_insert::render_history_rows;
use coco_tui_ui::engine::history_reflow::HistoryReflowState;
use coco_tui_ui::engine::history_reflow::HistoryViewportChange;
use coco_tui_ui::engine::terminal::SurfaceBackend;
use coco_tui_ui::engine::terminal::SurfaceTerminal;

/// First and last visible row text of a `HistoryRows` block, for the
/// `tui::surface::insert` logs. Lets a repro show exactly WHAT content each
/// scrollback write carries — so a user line that gets inserted by two
/// different writes (logical double-emit) is distinguishable from one inserted
/// once but painted twice (terminal/width desync).
///
/// Returns empty strings when that debug log is disabled — the buffer scan is
/// not worth paying on every scrollback insert otherwise.
fn history_rows_first_last(rows: &HistoryRows) -> (String, String) {
    if !tracing::enabled!(target: "tui::surface::insert", tracing::Level::DEBUG) {
        return (String::new(), String::new());
    }
    let width = rows.width() as usize;
    if width == 0 || rows.is_empty() {
        return (String::new(), String::new());
    }
    let buffer = rows.buffer();
    let row_text = |row: u16| -> String {
        let start = row as usize * width;
        buffer
            .content
            .get(start..start + width)
            .map(|cells| {
                cells
                    .iter()
                    .map(ratatui::buffer::Cell::symbol)
                    .collect::<String>()
                    .trim_end()
                    .chars()
                    .take(100)
                    .collect()
            })
            .unwrap_or_default()
    };
    (row_text(0), row_text(rows.height().saturating_sub(1)))
}

/// Session header lines + their fingerprint, built by the controller only
/// when [`crate::presentation::header::header_input_key`] changes — the
/// per-frame fast path compares the precomputed fingerprint instead of
/// rebuilding and re-hashing the header every draw.
#[derive(Debug, Clone)]
pub(crate) struct SessionHeader {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) fingerprint: Vec<RenderedLineFingerprint>,
}

impl SessionHeader {
    pub(crate) fn new(lines: Vec<Line<'static>>) -> Self {
        let fingerprint = fingerprint_lines(&lines);
        Self { lines, fingerprint }
    }
}

#[derive(Debug, Default)]
pub(crate) struct SurfaceHistoryDriver {
    emitter: HistoryEmissionTracker,
    reflow: HistoryReflowState,
    header_fingerprint: Option<Vec<RenderedLineFingerprint>>,
    emitted_transcript_revision: Option<u64>,
    emitted_history_rows: u16,
    replay_cache: HistoryReplayCache,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct HistoryReplayMode {
    pub(crate) stream_active: bool,
    pub(crate) cause: &'static str,
}

#[derive(Debug)]
pub(crate) enum PreparedFinalizedHistory {
    Noop { transcript_revision: u64 },
    FastNoop { revision: u64 },
    Append(PreparedHistoryAppend),
    ReplayRequired,
}

#[derive(Debug)]
pub(crate) struct PreparedHistoryAppend {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) message_count: usize,
    pub(crate) transcript_revision: u64,
    pub(crate) header_fingerprint: Vec<RenderedLineFingerprint>,
    pub(crate) emitted_header: bool,
    pub(crate) rows: HistoryRows,
    /// Time spent building the candidate `Line`s (cell rendering — markdown,
    /// diffs, syntax highlighting, stream-prefix anchoring).
    pub(crate) lines_build_elapsed: Duration,
    /// Time spent rendering those lines into terminal rows.
    pub(crate) render_elapsed: Duration,
    /// The anchored finalize ran: the in-flight scrollback commit was folded
    /// into this append, so the controller must `consume_commit` after it
    /// lands. The commit is owned by `SurfaceStreamDriver` (single owner), so
    /// the history driver only signals the consumption.
    pub(crate) consumed_stream_commit: bool,
}

impl PreparedFinalizedHistory {
    pub(crate) fn expected_rows(&self) -> u16 {
        match self {
            Self::Append(append) => append.rows.height(),
            Self::Noop { .. } | Self::FastNoop { .. } | Self::ReplayRequired => 0,
        }
    }

    pub(crate) fn render_elapsed(&self) -> Duration {
        match self {
            Self::Append(append) => append.render_elapsed,
            Self::Noop { .. } | Self::FastNoop { .. } | Self::ReplayRequired => Duration::default(),
        }
    }

    pub(crate) fn lines_build_elapsed(&self) -> Duration {
        match self {
            Self::Append(append) => append.lines_build_elapsed,
            Self::Noop { .. } | Self::FastNoop { .. } | Self::ReplayRequired => Duration::default(),
        }
    }
}

impl SurfaceHistoryDriver {
    pub(crate) fn note_viewport(
        &mut self,
        width: u16,
        stream_active: bool,
    ) -> HistoryViewportChange {
        let change = self.reflow.note_viewport(width);
        if change.changed && self.reflow.replay_needed_for_viewport(width) {
            self.reflow.schedule_viewport_replay(width, stream_active);
        }
        change
    }

    pub(crate) fn replay_due(&self, now: Instant) -> bool {
        self.reflow.pending_is_due(now)
    }

    pub(crate) fn prepare_append(
        &self,
        session_header: &SessionHeader,
        cells: &[RenderedCell],
        transcript_revision: u64,
        options: HistoryLineRenderOptions<'_>,
        stream_commit: Option<&ScrollbackStreamCommit>,
    ) -> PreparedFinalizedHistory {
        // Cheap early-outs first: the header compare and the revision
        // fast-path read precomputed values, so an unchanged-transcript frame
        // (the common streaming/spinner case) pays neither the O(cells)
        // `committable_prefix_len` walk nor any line build.
        if self
            .header_fingerprint
            .as_ref()
            .is_some_and(|emitted| emitted != &session_header.fingerprint)
        {
            tracing::debug!(
                target: "tui::surface::replay",
                cause = "header_fingerprint_changed",
                cells_total = cells.len(),
                emitted_messages = self.emitter.emitted_count(),
                "history full replay required",
            );
            return PreparedFinalizedHistory::ReplayRequired;
        }

        let should_emit_header = self.header_fingerprint.is_none();
        if !should_emit_header && self.emitted_transcript_revision == Some(transcript_revision) {
            return PreparedFinalizedHistory::FastNoop {
                revision: transcript_revision,
            };
        }

        let end = committable_prefix_len(cells);
        let committable_cells = &cells[..end];
        let plan = self.emitter.plan(committable_cells);
        if matches!(plan, HistoryEmissionPlan::Noop) && !should_emit_header {
            return PreparedFinalizedHistory::Noop {
                transcript_revision,
            };
        }
        if matches!(plan, HistoryEmissionPlan::ReplayRequired) {
            tracing::debug!(
                target: "tui::surface::replay",
                cause = "emitter_uuid_prefix_mismatch",
                cells = committable_cells.len(),
                cells_total = cells.len(),
                emitted_messages = self.emitter.emitted_count(),
                "history full replay required",
            );
            return PreparedFinalizedHistory::ReplayRequired;
        }

        let start = match plan {
            HistoryEmissionPlan::Append { start } => start,
            HistoryEmissionPlan::Noop | HistoryEmissionPlan::ReplayRequired => end,
        };
        let lines_build_started = Instant::now();
        let header = should_emit_header.then(|| session_header.lines.clone());
        let Some(lines) = self.append_candidate_lines_from_plan(
            header,
            cells,
            start,
            end,
            options,
            stream_commit,
        ) else {
            return PreparedFinalizedHistory::ReplayRequired;
        };
        let lines_build_elapsed = lines_build_started.elapsed();
        let render_started = Instant::now();
        let rows = render_history_rows(lines, options.width);
        PreparedFinalizedHistory::Append(PreparedHistoryAppend {
            start,
            end,
            message_count: end - start,
            transcript_revision,
            header_fingerprint: session_header.fingerprint.clone(),
            emitted_header: should_emit_header,
            rows,
            lines_build_elapsed,
            render_elapsed: render_started.elapsed(),
            consumed_stream_commit: stream_commit.is_some(),
        })
    }

    pub(crate) fn commit_prepared_append<B>(
        &mut self,
        terminal: &mut SurfaceTerminal<B>,
        prepared: &PreparedFinalizedHistory,
        cells: &[RenderedCell],
    ) -> Result<HistoryEmissionOutcome, B::Error>
    where
        B: SurfaceBackend,
    {
        match prepared {
            PreparedFinalizedHistory::Noop {
                transcript_revision,
            } => {
                self.emitted_transcript_revision = Some(*transcript_revision);
                Ok(HistoryEmissionOutcome::Noop)
            }
            PreparedFinalizedHistory::FastNoop { revision } => {
                Ok(HistoryEmissionOutcome::FastNoop {
                    revision: *revision,
                })
            }
            PreparedFinalizedHistory::ReplayRequired => Ok(HistoryEmissionOutcome::ReplayRequired),
            PreparedFinalizedHistory::Append(append) => {
                let width = terminal.viewport_area().width;
                let viewport_top_before = terminal.viewport_area().top();
                let history_bottom_before = terminal.history_bottom_y();
                let (first_row, last_row) = history_rows_first_last(&append.rows);
                let rows = terminal.insert_history_rows(&append.rows)?;
                tracing::debug!(
                    target: "tui::surface::insert",
                    kind = "finalized_append",
                    start = append.start,
                    end = append.end,
                    expected_rows = append.rows.height(),
                    inserted_rows = rows,
                    viewport_top_before,
                    viewport_top_after = terminal.viewport_area().top(),
                    history_bottom_before,
                    history_bottom_after = terminal.history_bottom_y(),
                    width,
                    first_row = %first_row,
                    last_row = %last_row,
                    "scrollback insert: finalized history append",
                );
                self.emitted_history_rows = self.emitted_history_rows.saturating_add(rows);
                self.header_fingerprint = Some(append.header_fingerprint.clone());
                self.emitter.mark_emitted_through(cells, append.end);
                self.emitted_transcript_revision = Some(append.transcript_revision);
                tracing::trace!(
                    target: "tui::surface::append",
                    start = append.start,
                    end = append.end,
                    cells_total = cells.len(),
                    message_count = append.message_count,
                    rows,
                    emitted_header = append.emitted_header,
                    lines_build_elapsed_us = append.lines_build_elapsed.as_micros(),
                    render_elapsed_us = append.render_elapsed.as_micros(),
                    "history incremental append",
                );
                Ok(HistoryEmissionOutcome::Appended {
                    start: append.start,
                    message_count: append.message_count,
                    rows,
                })
            }
        }
    }

    pub(crate) fn commit_stream_append<B>(
        &mut self,
        terminal: &mut SurfaceTerminal<B>,
        append: &PreparedStreamAppend,
    ) -> Result<HistoryEmissionOutcome, B::Error>
    where
        B: SurfaceBackend,
    {
        let width = terminal.viewport_area().width;
        let viewport_top_before = terminal.viewport_area().top();
        let history_bottom_before = terminal.history_bottom_y();
        let (first_row, last_row) = history_rows_first_last(&append.rows);
        let rows = terminal.insert_history_rows(&append.rows)?;
        if rows == 0 {
            return Ok(HistoryEmissionOutcome::Noop);
        }
        tracing::debug!(
            target: "tui::surface::insert",
            kind = "stream_append",
            source_prefix_len = append.commit.source_prefix.len(),
            line_prefix_len = append.commit.line_len,
            expected_rows = append.rows.height(),
            inserted_rows = rows,
            viewport_top_before,
            viewport_top_after = terminal.viewport_area().top(),
            history_bottom_before,
            history_bottom_after = terminal.history_bottom_y(),
            width,
            first_row = %first_row,
            last_row = %last_row,
            "scrollback insert: stream stable append",
        );
        self.emitted_history_rows = self.emitted_history_rows.saturating_add(rows);
        // The scrollback commit (`SurfaceStreamDriver`, single owner) is advanced
        // by the controller via `mark_stream_append_committed` right after this
        // returns `Appended` — the rows and the commit move together.
        tracing::trace!(
            target: "tui::surface::append",
            source_prefix_len = append.commit.source_prefix.len(),
            line_prefix_len = append.commit.line_len,
            rows,
            "history stream prefix append",
        );
        Ok(HistoryEmissionOutcome::Appended {
            start: 0,
            message_count: 0,
            rows,
        })
    }

    #[cfg(test)]
    pub(crate) fn emit_append_only<B>(
        &mut self,
        terminal: &mut SurfaceTerminal<B>,
        session_header: Vec<Line<'static>>,
        cells: &[RenderedCell],
        transcript_revision: u64,
        options: HistoryLineRenderOptions<'_>,
    ) -> Result<HistoryEmissionOutcome, B::Error>
    where
        B: SurfaceBackend,
    {
        self.emit_after_stream_commit(
            terminal,
            session_header,
            cells,
            transcript_revision,
            options,
            None,
        )
    }

    /// Finalize with a pending in-flight scrollback commit (the anchored-suffix
    /// path). Exercises `finalize_after_stream_prefix` end to end.
    #[cfg(test)]
    pub(crate) fn emit_after_stream_commit<B>(
        &mut self,
        terminal: &mut SurfaceTerminal<B>,
        session_header: Vec<Line<'static>>,
        cells: &[RenderedCell],
        transcript_revision: u64,
        options: HistoryLineRenderOptions<'_>,
        stream_commit: Option<&ScrollbackStreamCommit>,
    ) -> Result<HistoryEmissionOutcome, B::Error>
    where
        B: SurfaceBackend,
    {
        let prepared = self.prepare_append(
            &SessionHeader::new(session_header),
            cells,
            transcript_revision,
            options,
            stream_commit,
        );
        self.commit_prepared_append(terminal, &prepared, cells)
    }

    pub(crate) fn replay_all_capped<B>(
        &mut self,
        terminal: &mut SurfaceTerminal<B>,
        session_header: Vec<Line<'static>>,
        cells: &[RenderedCell],
        transcript_revision: u64,
        options: HistoryLineRenderOptions<'_>,
        mode: HistoryReplayMode,
    ) -> Result<HistoryEmissionOutcome, B::Error>
    where
        B: SurfaceBackend,
    {
        let end = committable_prefix_len(cells);
        let committable_cells = &cells[..end];
        let started = Instant::now();
        let replay = render_replay_history_lines_cached(
            committable_cells,
            options,
            DEFAULT_MAX_REFLOW_ROWS,
            &mut self.replay_cache,
        );
        let render_elapsed = started.elapsed();
        let line_count = replay.lines.len();
        let outcome = self.replay_rows(
            terminal,
            session_header,
            committable_cells,
            transcript_revision,
            &replay.rows,
        )?;
        if replay.omitted_messages > 0 {
            tracing::info!(
                target: "tui::surface::replay",
                cause = mode.cause,
                max_rows = DEFAULT_MAX_REFLOW_ROWS,
                omitted_messages = replay.omitted_messages,
                rows = replay.rows.height(),
                lines = line_count,
                "history replay capped older messages",
            );
        }
        let area = terminal.viewport_area();
        self.reflow
            .mark_replayed_viewport(area.width, mode.stream_active);
        tracing::debug!(
            target: "tui::surface::replay",
            cause = mode.cause,
            render_elapsed_ms = render_elapsed.as_millis(),
            cells = committable_cells.len(),
            cells_total = cells.len(),
            cells_rendered = replay.stats.cells_rendered,
            finalized_render_calls = replay.stats.finalized_render_calls,
            lines = line_count,
            omitted_messages = replay.omitted_messages,
            cache_hit = replay.stats.cache_hit,
            cacheable = replay.stats.cacheable,
            cache_lookup = ?replay.stats.cache_lookup,
            cache_skip_reason = ?replay.stats.cache_skip_reason,
            cache_admitted = replay.stats.cache_admitted,
            key_build_elapsed_us = replay.stats.key_build_elapsed_us,
            cache_entries = replay.stats.cache_entries,
            cache_estimated_bytes = replay.stats.cache_estimated_bytes,
            cell_content_estimated_bytes = replay.stats.cell_content_estimated_bytes,
            replay_estimated_bytes = replay.stats.replay_estimated_bytes,
            cache_evictions = replay.stats.cache_evictions,
            width = area.width,
            rows = replay.rows.height(),
            height = area.height,
            stream_active = mode.stream_active,
            "history replay render completed",
        );
        Ok(outcome)
    }

    pub(crate) fn reset(&mut self) {
        self.emitter.reset();
        self.header_fingerprint = None;
        self.emitted_transcript_revision = None;
        self.emitted_history_rows = 0;
        self.reflow.clear();
        self.replay_cache.clear();
    }

    fn replay_rows<B>(
        &mut self,
        terminal: &mut SurfaceTerminal<B>,
        session_header: Vec<Line<'static>>,
        cells: &[RenderedCell],
        transcript_revision: u64,
        message_rows: &HistoryRows,
    ) -> Result<HistoryEmissionOutcome, B::Error>
    where
        B: SurfaceBackend,
    {
        let header_fingerprint = fingerprint_lines(&session_header);
        let viewport_area = terminal.viewport_area();
        terminal.clear_owned_scrollback()?;
        let width = viewport_area.width;
        let header_rows = render_history_rows(session_header, width);
        let Some(rendered) = HistoryRows::try_copy_tail_from_slices(
            width,
            &[
                header_rows.tail_slice(u16::MAX),
                message_rows.tail_slice(u16::MAX),
            ],
            u16::MAX,
        ) else {
            tracing::debug!(
                target: "tui::surface::replay",
                cause = "replay_rows_width_mismatch",
                header_width = header_rows.width(),
                message_width = message_rows.width(),
                width,
                "history replay row concat failed",
            );
            return Ok(HistoryEmissionOutcome::ReplayRequired);
        };
        // No viewport reseat after insert: `clear_owned_scrollback` zeroed the
        // viewport and `insert_history_rows` flowed it back down to the freshly
        // inserted history bottom (or pinned it at the screen bottom on
        // overflow). That seat is already correct — the old clamp/restore only
        // existed to undo `sync_surface_area`'s stale-anchor reposition, which
        // no longer happens (it anchors on the owned viewport top).
        let (first_row, last_row) = history_rows_first_last(&rendered);
        let rows = terminal.insert_history_rows(&rendered)?;
        tracing::debug!(
            target: "tui::surface::insert",
            kind = "replay_all",
            cells = cells.len(),
            expected_rows = rendered.height(),
            inserted_rows = rows,
            width,
            first_row = %first_row,
            last_row = %last_row,
            "scrollback insert: full replay (cleared scrollback then re-inserted)",
        );
        self.emitted_history_rows = rows;
        self.header_fingerprint = Some(header_fingerprint);
        self.emitter.mark_emitted_through(cells, cells.len());
        self.emitted_transcript_revision = Some(transcript_revision);
        tracing::debug!(
            target: "tui::surface::replay",
            message_count = cells.len(),
            rows,
            "history full replay completed",
        );
        Ok(HistoryEmissionOutcome::Replayed {
            message_count: cells.len(),
            rows,
        })
    }

    fn append_candidate_lines_from_plan(
        &self,
        // `Some(header_lines)` emits the session header before the cells; `None`
        // means it was already emitted (an incremental append).
        header: Option<Vec<Line<'static>>>,
        cells: &[RenderedCell],
        start: usize,
        end: usize,
        options: HistoryLineRenderOptions<'_>,
        stream_commit: Option<&ScrollbackStreamCommit>,
    ) -> Option<Vec<Line<'static>>> {
        if let Some(commit) = stream_commit {
            if header.is_some() {
                tracing::debug!(
                    target: "tui::surface::replay",
                    cause = "stream_commit_without_header",
                    "history full replay required",
                );
                return None;
            }
            return finalize_after_stream_prefix(cells, start, end, options, commit);
        }
        let mut lines = Vec::new();
        if let Some(header) = header {
            lines.extend(header);
        }
        lines.extend(render_finalized_history_lines(&cells[start..end], options));
        Some(lines)
    }
}

#[cfg(test)]
#[path = "history_driver.test.rs"]
mod tests;
