//! Source-backed native-history replay scheduling, keyed on terminal **width**.
//!
//! History re-wrap depends solely on the wrap width: rows already in native
//! scrollback do not re-wrap when the interactive viewport grows or shrinks
//! (e.g. the live tail expanding during streaming). Keying on height as well
//! would schedule a full-history replay on every per-frame height change — the
//! streaming flicker we explicitly avoid — so only width is tracked here.

use std::time::Duration;
use std::time::Instant;

pub const HISTORY_REFLOW_DEBOUNCE: Duration = Duration::from_millis(75);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryViewportChange {
    pub initialized: bool,
    pub changed: bool,
}

#[derive(Debug, Default, Clone)]
pub struct HistoryReflowState {
    last_observed_width: Option<u16>,
    last_replayed_width: Option<u16>,
    pending_width: Option<u16>,
    pending_until: Option<Instant>,
}

impl HistoryReflowState {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn note_viewport(&mut self, width: u16) -> HistoryViewportChange {
        let previous_width = self.last_observed_width.replace(width);
        if previous_width.is_none() {
            self.last_replayed_width = Some(width);
        }
        HistoryViewportChange {
            initialized: previous_width.is_none(),
            changed: previous_width.is_some_and(|previous_width| previous_width != width),
        }
    }

    pub fn replay_needed_for_viewport(&self, width: u16) -> bool {
        self.last_replayed_width != Some(width) && self.pending_width != Some(width)
    }

    pub fn schedule_viewport_replay(&mut self, width: u16, _stream_active: bool) {
        self.pending_width = Some(width);
        self.schedule_pending();
    }

    fn schedule_pending(&mut self) {
        self.pending_until = Some(Instant::now() + HISTORY_REFLOW_DEBOUNCE);
    }

    pub fn schedule_immediate(&mut self) {
        self.pending_until = Some(Instant::now());
    }

    pub fn pending_is_due(&self, now: Instant) -> bool {
        self.pending_until.is_some_and(|deadline| now >= deadline)
    }

    pub fn pending_viewport(&self) -> Option<u16> {
        self.pending_width
    }

    pub fn clear_pending(&mut self) {
        self.pending_width = None;
        self.pending_until = None;
    }

    pub fn mark_replayed_viewport(&mut self, width: u16, _stream_active: bool) {
        self.last_replayed_width = Some(width);
        self.clear_pending();
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn force_due_for_test(&mut self) {
        self.pending_until = Some(Instant::now() - Duration::from_millis(1));
    }
}

#[cfg(test)]
#[path = "history_reflow.test.rs"]
mod tests;
