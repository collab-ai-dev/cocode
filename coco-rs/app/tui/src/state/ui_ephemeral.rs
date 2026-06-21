//! UI-only ephemeral state that needs cross-handler scope but is not
//! conversation/engine state.
//!
//! Lives here so [`crate::state::SessionState`] doesn't accumulate
//! render-only fields. Each entry exists because some handler stamps
//! a timestamp / sample that a later render consumes — the data is
//! truly local to the TUI; the engine never reads it.
//!
//! ## Turn-lifecycle state is grouped into [`RunningTurn`]
//!
//! The fields that only make sense while an LLM turn is running
//! (`started_at`, `verb`, pause accumulator, current-pause anchor,
//! live token estimate)
//! live inside a single [`RunningTurn`] struct accessed through
//! [`UiEphemeralState::turn`]. Storing them as `Option<RunningTurn>`
//! instead of four independent `Option<…>` fields makes illegal
//! combinations (e.g. `verb = Some` with `started_at = None`)
//! unrepresentable — the type itself enforces the invariant that
//! [`crate::server_notification_handler::protocol`] used to enforce
//! by convention.
//!
//! ## Reset boundaries
//! - The turn-lifecycle fields rotate with the wire-level lifecycle:
//!   [`UiEphemeralState::start_turn`] at `TurnStarted`, then
//!   [`UiEphemeralState::end_turn`] at any terminal turn event. The
//!   final `total_paused_ms` is preserved in
//!   [`UiEphemeralState::last_total_paused_ms`] so a stalled paint
//!   between the terminal event and the next frame still resolves to
//!   a consistent elapsed value.
//! - `task_completion_timestamps` and `tasks_all_completed_since_ms`
//!   reset on the per-task diff in the `TaskPanelChanged` handler.

use std::collections::HashMap;
use std::time::Instant;

/// Per-turn UI clock state. Holds everything we'd otherwise have to
/// represent as four loosely-coupled `Option<…>` fields.
#[derive(Debug, Clone)]
pub(crate) struct RunningTurn {
    /// Wall-clock instant when the engine emitted `TurnStarted`.
    /// Anchor for [`UiEphemeralState::elapsed_ms`].
    pub(crate) started_at: Instant,
    /// Whimsical present-participle verb sampled once at turn start.
    /// Stable for the lifetime of this turn.
    pub(crate) verb: &'static str,
    /// Accumulated milliseconds the status timer has spent paused
    /// since [`Self::started_at`]. Folded forward every time a
    /// permission-prompt pause closes.
    pub(crate) total_paused_ms: i64,
    /// `Some(instant)` while the status timer is currently paused.
    /// Set on the first paint that observes a blocking prompt;
    /// cleared (with the elapsed folded into
    /// [`Self::total_paused_ms`]) when the prompt closes.
    pub(crate) pause_started_at: Option<Instant>,
    /// Count of streamed assistant-text characters in this running
    /// turn. Converted to an approximate output-token count for the
    /// live spinner; exact completed usage remains on `SessionState`.
    output_chars: i64,
    /// Set the moment the user requests cancellation (Esc / Ctrl+C),
    /// so the spinner can switch its verb to "Interrupting…"
    /// synchronously on the keypress — decoupled from the slower
    /// async teardown that only clears the turn on the terminal
    /// `TurnEnded(Interrupted)` event. Mirrors TS `resetLoadingState`
    /// running on the keypress while `abortController.abort()`
    /// propagates in the background. Auto-cleared when `end_turn`
    /// takes the whole [`RunningTurn`].
    interrupting: bool,
}

/// All UI-only ephemera that previously lived on `SessionState`.
#[derive(Debug, Clone, Default)]
pub(crate) struct UiEphemeralState {
    /// Per-turn state. `Some(_)` strictly between the wire-level
    /// `TurnStarted` and matching terminal turn event; `None` while
    /// the session is idle.
    pub(crate) turn: Option<RunningTurn>,
    /// Final `total_paused_ms` from the most recent turn, preserved
    /// after [`Self::end_turn`] so a paint between the terminal
    /// event and the next state mutation still resolves elapsed
    /// consistently. Reset on [`Self::start_turn`].
    pub(crate) last_total_paused_ms: i64,
    /// Unix-ms when each plan task transitioned to `Completed`.
    /// Powers the 30s priority lift in [`crate::widgets::todo_panel`].
    /// Stamped at the diff point in the `TaskPanelChanged` handler;
    /// GC'd on every snapshot.
    pub(crate) task_completion_timestamps: HashMap<String, i64>,
    /// Unix-ms at which the *entire* plan task list first became all
    /// completed (no Pending or InProgress remaining). The panel
    /// suppresses itself once this stamp is ≥5s old. Cleared the
    /// moment any non-completed task reappears.
    pub(crate) tasks_all_completed_since_ms: Option<i64>,
}

impl UiEphemeralState {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Start a new turn: anchor the start instant, sample the verb,
    /// and zero the pause accumulators. Idempotent — calling twice
    /// in a row simply re-anchors (rapid resubmission).
    pub(crate) fn start_turn(&mut self, verb: &'static str, started_at: Instant) {
        self.turn = Some(RunningTurn {
            started_at,
            verb,
            total_paused_ms: 0,
            pause_started_at: None,
            output_chars: 0,
            interrupting: false,
        });
        self.last_total_paused_ms = 0;
    }

    /// End the current turn. Preserves the final `total_paused_ms`
    /// in [`Self::last_total_paused_ms`] so a paint between the
    /// terminal event and the next state mutation sees the same
    /// frozen elapsed value the user just saw.
    pub(crate) fn end_turn(&mut self) {
        if let Some(turn) = self.turn.take() {
            self.last_total_paused_ms = turn.total_paused_ms;
        }
    }

    /// Tick the status-indicator pause clock based on the current
    /// `blocked` state. Call from each paint.
    ///
    /// Semantics:
    /// - No-op when no turn is running (anchor on the type, not on
    ///   the caller).
    /// - When a blocking prompt becomes active, anchor `pause_started_at`.
    /// - When the prompt closes, fold the paused interval into
    ///   `total_paused_ms` and clear the anchor.
    pub(crate) fn tick_pause_clock(&mut self, blocked: bool, now: Instant) {
        let Some(turn) = self.turn.as_mut() else {
            return;
        };
        match (blocked, turn.pause_started_at) {
            (true, None) => {
                turn.pause_started_at = Some(now);
            }
            (false, Some(started)) => {
                let dur = now.saturating_duration_since(started).as_millis() as i64;
                turn.total_paused_ms = turn.total_paused_ms.saturating_add(dur);
                turn.pause_started_at = None;
            }
            _ => {}
        }
    }

    /// Compute the displayed elapsed milliseconds for the status
    /// indicator. Returns 0 when no turn is running. Subtracts
    /// `total_paused_ms` plus any in-progress pause duration so the
    /// clock freezes while the user is at a permission prompt.
    pub(crate) fn elapsed_ms(&self, now: Instant) -> i64 {
        let Some(turn) = self.turn.as_ref() else {
            return 0;
        };
        let wall = now.saturating_duration_since(turn.started_at).as_millis() as i64;
        let current_pause = turn
            .pause_started_at
            .map(|p| now.saturating_duration_since(p).as_millis() as i64)
            .unwrap_or(0);
        (wall - turn.total_paused_ms - current_pause).max(0)
    }

    /// `Some(verb)` while a turn is running; `None` otherwise.
    pub(crate) fn current_verb(&self) -> Option<&'static str> {
        self.turn.as_ref().map(|t| t.verb)
    }

    /// Flag the running turn as cancellation-requested. Idempotent and
    /// a no-op when no turn is running. Lets the spinner reflect the
    /// interrupt on the same frame as the keypress, before the engine
    /// confirms teardown with `TurnEnded(Interrupted)`.
    pub(crate) fn mark_interrupting(&mut self) {
        if let Some(turn) = self.turn.as_mut() {
            let already = turn.interrupting;
            turn.interrupting = true;
            // Log only on the transition so a repeated Esc/Ctrl+C doesn't
            // spam — the first line's timestamp diffed against the engine's
            // `TurnEnded(Interrupted)` is the "Interrupting…" visible window.
            if !already {
                tracing::debug!(
                    target: "coco_tui::interrupt",
                    "ui: spinner verb → interrupting (cancel requested mid-turn)"
                );
            }
        }
    }

    /// `true` once the user has requested cancellation for the running
    /// turn and the engine has not yet emitted the terminal event.
    pub(crate) fn is_interrupting(&self) -> bool {
        self.turn.as_ref().is_some_and(|t| t.interrupting)
    }

    /// Add streamed assistant text to the running-turn token estimate.
    /// Character-count approximation while the turn is still in flight;
    /// the exact API usage only arrives on `TurnCompleted`.
    pub(crate) fn add_output_delta(&mut self, delta: &str) {
        let Some(turn) = self.turn.as_mut() else {
            return;
        };
        turn.output_chars = turn
            .output_chars
            .saturating_add(delta.chars().count() as i64);
    }

    /// Approximate live output tokens for the current running turn.
    /// Uses the common `chars / 4` estimate; intentionally stays
    /// separate from completed `session.token_usage`.
    pub(crate) fn live_output_tokens(&self) -> i64 {
        self.turn
            .as_ref()
            .map(|turn| (turn.output_chars.saturating_add(3)) / 4)
            .unwrap_or(0)
    }

    /// `Some(started_at)` while a turn is running; `None` otherwise.
    /// Read by callers that need the raw anchor (e.g. legacy
    /// duration arithmetic in `TurnCompleted` telemetry).
    pub(crate) fn turn_started_at(&self) -> Option<Instant> {
        self.turn.as_ref().map(|t| t.started_at)
    }

    /// `true` while a turn is running. Convenience for renderers
    /// that gate on turn activity without caring about the anchor.
    pub(crate) fn turn_active(&self) -> bool {
        self.turn.is_some()
    }

    /// `true` while the running turn's status clock is paused on a
    /// blocking tool-permission prompt. The spinner glyph is a pure
    /// function of the paused-aware `elapsed_ms`
    /// ([`coco_tui_ui::widgets::status_indicator`]), so a paused turn
    /// renders an identical frame every tick — animating it is wasted
    /// work. Pairs with [`Self::turn_active`].
    pub(crate) fn turn_paused(&self) -> bool {
        self.turn
            .as_ref()
            .is_some_and(|turn| turn.pause_started_at.is_some())
    }
}

#[cfg(test)]
#[path = "ui_ephemeral.test.rs"]
mod tests;
