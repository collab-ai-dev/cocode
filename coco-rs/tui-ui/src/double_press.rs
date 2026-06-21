//! Generic time-based double-press tracker.
//!
//! Models the TS `useDoublePress` hook (`src/hooks/useDoublePress.ts`):
//! each call to [`DoublePressTracker::poll`] is either the first press in
//! a window (caller arms a prompt + performs the first-press action) or
//! the second press completing the window (caller fires the double-press
//! action). [`DoublePressTracker::tick`] expires the window when no
//! second press arrives.
//!
//! Generic over `K` so the same primitive serves both Esc rewind
//! (`DoublePressTracker<()>`) and Ctrl+C / Ctrl+D exit
//! (`DoublePressTracker<ExitKey>`). Pressing key `B` while `A` is armed
//! re-arms with `B`, matching TS [`ExitState`] semantics — only one
//! "press again" prompt can be visible at a time.
//!
//! `K` is bounded on `Copy + PartialEq` because the tracker stores the
//! key by value and compares it on the second press; in practice `K` is
//! a small enum or `()`, never an owned heap type.

use std::time::Duration;
use std::time::Instant;

/// Internal state. Hidden because callers should query via
/// [`DoublePressTracker::pending`] rather than match on the enum.
#[derive(Debug, Clone)]
enum State<K> {
    Idle,
    Armed { key: K, until: Instant },
}

/// Outcome of [`DoublePressTracker::poll`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// First press in the window. Caller should perform any
    /// first-press side effect (e.g. cancel the current task) and
    /// render a "press again" hint until [`DoublePressTracker::tick`]
    /// clears it.
    First,
    /// Second press completing a double-press. Caller fires the
    /// double-press action. Tracker is back to idle.
    Double,
}

#[derive(Debug)]
pub struct DoublePressTracker<K> {
    state: State<K>,
    window: Duration,
}

impl<K: Copy + PartialEq> DoublePressTracker<K> {
    /// Create a tracker with the given second-press window.
    pub fn new(window: Duration) -> Self {
        Self {
            state: State::Idle,
            window,
        }
    }

    /// Register a key press at `now`. Reads-and-writes the internal
    /// state atomically: caller never has to worry about ordering with
    /// any "track timestamp before dispatch" code path.
    ///
    /// Re-arming with a *different* key resets the previous arm —
    /// pressing Ctrl+D while Ctrl+C is armed produces `First` (for
    /// Ctrl+D), not `Double`.
    pub fn poll(&mut self, key: K, now: Instant) -> Outcome {
        if let State::Armed { key: prev, until } = self.state
            && prev == key
            && now <= until
        {
            self.state = State::Idle;
            return Outcome::Double;
        }
        self.state = State::Armed {
            key,
            until: now + self.window,
        };
        Outcome::First
    }

    /// Return the currently armed key, if any. Used by the renderer
    /// to show a "press <key> again to <action>" hint.
    pub fn pending(&self) -> Option<&K> {
        match &self.state {
            State::Idle => None,
            State::Armed { key, .. } => Some(key),
        }
    }

    /// Expiry time of the current arm, if any. Used by callers that
    /// maintain several trackers in parallel and need to know which
    /// was armed most recently (the larger `until` wins). Returns
    /// `None` when idle.
    pub fn pending_until(&self) -> Option<Instant> {
        match self.state {
            State::Idle => None,
            State::Armed { until, .. } => Some(until),
        }
    }

    /// Clear any pending arm without firing. Used when the
    /// double-press semantics no longer apply — e.g. the user pressed
    /// Ctrl+C while a task was running, so we cancelled the task
    /// instead of arming an exit prompt.
    pub fn reset(&mut self) {
        self.state = State::Idle;
    }

    /// Advance time. Returns `true` if a pending arm was cleared by
    /// expiry — caller should request a redraw so the hint disappears.
    pub fn tick(&mut self, now: Instant) -> bool {
        if let State::Armed { until, .. } = self.state
            && now > until
        {
            self.state = State::Idle;
            return true;
        }
        false
    }
}

#[cfg(test)]
#[path = "double_press.test.rs"]
mod tests;
