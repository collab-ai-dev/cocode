//! Native-surface live stream preparation.

use std::collections::hash_map::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;

use coco_tui_ui::display::SyntaxHighlighting;
use coco_tui_ui::style::UiStyles;
use ratatui::text::Line;

use crate::presentation::thinking::ThinkingDisplay;
use crate::presentation::thinking::ThinkingRenderInput;
use crate::presentation::thinking::estimate_reasoning_tokens;
use crate::presentation::thinking::render_thinking_block;
use crate::state::AppState;
use crate::state::PermissionDetail;
use crate::state::PermissionPromptState;
use crate::surface::modal::SurfaceFramePlan;
use crate::surface::viewport::active_exit_plan_prompt;
use crate::surface::viewport::build_live_tail_lines;
use crate::terminal::STREAMING_LIVE_TAIL_CAP;
use crate::transcript::stream::ScrollbackStreamCommit;
use crate::transcript::stream::StreamRenderController;
use crate::transcript::stream::StreamRenderInput;
use crate::transcript::stream::streaming_cursor_line;
use coco_tui_ui::engine::history_insert::HistoryRows;
use coco_tui_ui::engine::history_insert::render_history_rows;

// Deliberately NOT Clone: `committed` is the single scrollback-commit owner
// (tui-v2 §6.7-10); a clone would be the forbidden second copy.
#[derive(Debug, Default)]
pub(crate) struct SurfaceStreamDriver {
    controller: StreamRenderController,
    pending_plan: Option<PendingPlanProjectionCache>,
    /// The single record of stream rows already inserted into native scrollback
    /// (`None` until the first mid-stream stable commit). This is the SOLE owner
    /// of that fact — the finalize reads it through [`Self::commit`]. The
    /// anchored finalize re-proves agreement at the SOURCE level, so no per-row
    /// fingerprint is retained — soundness rests on the markdown
    /// prefix-stability property pinned in `transcript::stream` tests.
    committed: Option<ScrollbackStreamCommit>,
}

#[derive(Debug, Clone)]
struct PendingPlanProjectionCache {
    key: PendingPlanProjectionKey,
    lines: Vec<Line<'static>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingPlanProjectionKey {
    request_id: String,
    detail_hash: u64,
    width: u16,
    theme_hash: u64,
    syntax_highlighting: SyntaxHighlighting,
}

#[derive(Debug, Default)]
pub(crate) struct PreparedLiveTail {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) stream_append: Option<PreparedStreamAppend>,
    /// The rows already in scrollback are stale (render key changed) or belong
    /// to a different document (source replaced) — the surface must replay the
    /// finalized history before this frame's stream rows make sense.
    pub(crate) commit_invalidated: bool,
    /// `Some(hit)` when the stream projection ran this frame (`None` for
    /// view-mode / no-stream frames) — perf-log attribution only.
    pub(crate) stream_cache_hit: Option<bool>,
}

#[derive(Debug)]
pub(crate) struct PreparedStreamAppend {
    pub(crate) rows: HistoryRows,
    pub(crate) commit: ScrollbackStreamCommit,
}

impl PreparedStreamAppend {
    pub(crate) fn expected_rows(&self) -> u16 {
        self.rows.height()
    }
}

impl SurfaceStreamDriver {
    pub(crate) fn prepare(
        &mut self,
        state: &AppState,
        width: u16,
        plan: SurfaceFramePlan,
    ) -> PreparedLiveTail {
        let styles = UiStyles::new(&state.ui.theme);
        if active_exit_plan_prompt(state).is_none() {
            self.pending_plan = None;
        }
        if width == 0 || plan.finalized_history_in_viewport() {
            // View-mode / zero-width frame: no rows enter native scrollback, so
            // the commit is untouched (clearing it here is what let a later
            // frame re-emit the rows already in scrollback — duplication).
            return PreparedLiveTail {
                lines: build_live_tail_lines(state, styles, width, plan),
                stream_append: None,
                commit_invalidated: false,
                stream_cache_hit: None,
            };
        }

        let Some(streaming) = state.ui.streaming.as_ref() else {
            // Transient gap: event coalescing can fold a turn's last delta and
            // the next turn's first into one draw, so a `streaming == None`
            // frame is NOT a reliable end-of-stream signal. Reset only the
            // render caches (the single-slot in-flight memo would otherwise
            // pin the last response's render until the next stream); the
            // scrollback commit persists until the finalize consumes it or a
            // genuine identity change replays it. Clearing it here
            // re-committed already-present rows (duplication).
            self.controller.clear();
            crate::transcript::render::assistant::clear_in_flight_markdown_memo();
            return PreparedLiveTail {
                lines: self.pending_plan_lines(state, width, styles),
                stream_append: None,
                commit_invalidated: false,
                stream_cache_hit: None,
            };
        };

        let visible = streaming.visible_content();
        let render_key_input = StreamRenderInput {
            source: visible,
            generation: streaming.visible_generation,
            styles,
            width,
            syntax_highlighting: state.ui.display_settings.syntax_highlighting,
        };
        let projection = self.controller.render_projection(render_key_input);
        let projection_cache_hit = projection.cache_hit;
        let stable_line_len = projection.stable_lines.len();
        // CONTENT identity, not size, is the gate — and it does NOT depend on
        // the controller reset flag. A surviving commit is still valid iff its
        // exact source prefix is a prefix of the current stream AND its rows
        // fit within the current stable region under the same render key. This
        // self-heals a coalesced turn boundary (the new turn's source does not
        // start with the old commit's prefix → invalid) without falsely
        // invalidating a same-document resume after a transient `None` gap.
        let emitted_valid = self.committed.as_ref().is_some_and(|c| {
            c.render_key == projection.render_key
                && c.source_prefix.len() <= projection.stable_source_len
                && c.line_len <= stable_line_len
                && visible.starts_with(&c.source_prefix)
        });
        let commit_invalidated = self.committed.is_some() && !emitted_valid;
        let (emitted_line_len, emitted_source_len) = match self.committed.as_ref() {
            Some(c) if emitted_valid => (c.line_len, c.source_prefix.len()),
            _ => (0, 0),
        };

        let mut stream_append = None;
        let live_start_line = if stable_line_len > emitted_line_len
            && projection.stable_source_len > emitted_source_len
        {
            let append_rows_started = std::time::Instant::now();
            let rows =
                render_history_rows(projection.stable_lines[emitted_line_len..].to_vec(), width);
            let append_rows_elapsed = append_rows_started.elapsed();
            if !rows.is_empty() {
                tracing::debug!(
                    target: "tui::streaming",
                    appended_lines = stable_line_len - emitted_line_len,
                    prefix_lines = stable_line_len,
                    source_prefix_bytes = projection.stable_source_len,
                    append_rows_us = append_rows_elapsed.as_micros(),
                    "stream stable append prepared",
                );
                stream_append = Some(PreparedStreamAppend {
                    rows,
                    commit: ScrollbackStreamCommit {
                        source_prefix: visible[..projection.stable_source_len].to_string(),
                        line_len: stable_line_len,
                        render_key: projection.render_key,
                    },
                });
            }
            stable_line_len
        } else {
            emitted_line_len
        };

        // Display-cap (applied via the final `drain` below): when the user is
        // not scrolling, only the last `cap` markdown rows can survive, so skip
        // cloning rows that the drain would immediately discard.
        let cap = STREAMING_LIVE_TAIL_CAP as usize;
        let stable_tail = &projection.stable_lines[live_start_line..];
        let markdown_len = stable_tail.len() + projection.tail_lines.len();
        let markdown_skip = if state.ui.user_scrolled {
            0
        } else {
            markdown_len.saturating_sub(cap)
        };
        let mut lines: Vec<Line<'static>> =
            Vec::with_capacity(markdown_len.saturating_sub(markdown_skip) + 1);
        lines.extend(stable_tail.iter().skip(markdown_skip).cloned());
        lines.extend(
            projection
                .tail_lines
                .iter()
                .skip(markdown_skip.saturating_sub(stable_tail.len()))
                .cloned(),
        );
        if !lines.is_empty() {
            lines.push(streaming_cursor_line(styles));
        }
        if state.ui.show_thinking && !streaming.thinking.is_empty() {
            lines.extend(render_thinking_block(
                ThinkingRenderInput {
                    content: "",
                    duration_ms: None,
                    reasoning_tokens: Some(estimate_reasoning_tokens(&streaming.thinking)),
                    toggle_hint: None,
                    display: ThinkingDisplay::Collapsed,
                },
                styles,
            ));
        }

        // Display-cap the streaming tail to a constant height so the inline
        // viewport stops growing (then collapsing) with streamed content. Under
        // Policy A, capped leading rows remain in streaming source state but are
        // not native-scrollable until the finalized assistant message appends.
        // Skipped while the user is scrolling so they can read the full
        // in-flight tail.
        if !state.ui.user_scrolled && lines.len() > cap {
            lines.drain(0..lines.len() - cap);
        }
        self.append_pending_plan_lines(&mut lines, state, width, styles);

        // The stale commit describes rows in scrollback that no longer match
        // this frame; drop it so the surface replays and the live tail rebuilds
        // from a clean slate. (Replay also clears scrollback via the controller,
        // keeping the single commit and the actual scrollback in agreement.)
        if commit_invalidated {
            self.committed = None;
        }
        PreparedLiveTail {
            lines,
            stream_append,
            commit_invalidated,
            stream_cache_hit: Some(projection_cache_hit),
        }
    }

    /// The single record of stream rows already in native scrollback, read by
    /// the anchored finalize (`transcript::emission`).
    pub(crate) fn commit(&self) -> Option<&ScrollbackStreamCommit> {
        self.committed.as_ref()
    }

    /// Advance the commit after this frame's stream rows were inserted.
    pub(crate) fn mark_stream_append_committed(&mut self, append: &PreparedStreamAppend) {
        self.committed = Some(append.commit.clone());
    }

    /// The finalize consumed the in-flight rows into the committed message —
    /// the commit no longer describes pending in-flight scrollback.
    pub(crate) fn consume_commit(&mut self) {
        self.committed = None;
    }

    /// Scrollback was cleared (replay/reset) — the commit no longer describes
    /// anything in scrollback.
    pub(crate) fn invalidate_commit(&mut self) {
        self.committed = None;
    }

    pub(crate) fn reset(&mut self) {
        self.controller.clear();
        self.committed = None;
        self.pending_plan = None;
        crate::transcript::render::assistant::clear_in_flight_markdown_memo();
    }

    fn append_pending_plan_lines(
        &mut self,
        lines: &mut Vec<Line<'static>>,
        state: &AppState,
        width: u16,
        styles: UiStyles<'_>,
    ) {
        let pending = self.pending_plan_lines(state, width, styles);
        if pending.is_empty() {
            return;
        }
        if !lines.is_empty() {
            lines.push(Line::from(""));
        }
        lines.extend(pending);
    }

    fn pending_plan_lines(
        &mut self,
        state: &AppState,
        width: u16,
        styles: UiStyles<'_>,
    ) -> Vec<Line<'static>> {
        let Some(prompt) = active_exit_plan_prompt(state) else {
            self.pending_plan = None;
            return Vec::new();
        };
        let key = pending_plan_projection_key(prompt, width, styles, state);
        if let Some(cache) = self.pending_plan.as_ref()
            && cache.key == key
        {
            return cache.lines.clone();
        }
        let lines = crate::presentation::request::exit_plan_pending_history_lines(
            prompt,
            width,
            state.ui.display_settings.syntax_highlighting,
            styles,
        );
        self.pending_plan = Some(PendingPlanProjectionCache {
            key,
            lines: lines.clone(),
        });
        lines
    }
}

fn pending_plan_projection_key(
    prompt: &PermissionPromptState,
    width: u16,
    styles: UiStyles<'_>,
    state: &AppState,
) -> PendingPlanProjectionKey {
    PendingPlanProjectionKey {
        request_id: prompt.request_id.clone(),
        detail_hash: pending_plan_detail_hash(prompt),
        width,
        theme_hash: styles.theme_hash(),
        syntax_highlighting: state.ui.display_settings.syntax_highlighting,
    }
}

fn pending_plan_detail_hash(prompt: &PermissionPromptState) -> u64 {
    let mut hasher = DefaultHasher::new();
    if let PermissionDetail::ExitPlanMode {
        outcome,
        plan,
        plan_file_path,
        allowed_prompts,
    } = &prompt.detail
    {
        outcome.hash(&mut hasher);
        plan.hash(&mut hasher);
        plan_file_path.hash(&mut hasher);
        allowed_prompts.hash(&mut hasher);
    }
    hasher.finish()
}

#[cfg(test)]
#[path = "stream.test.rs"]
mod tests;
