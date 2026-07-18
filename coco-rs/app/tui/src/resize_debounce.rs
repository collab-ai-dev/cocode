//! Resize quiet period (grok-build port plan, item A2).
//!
//! Dragging a terminal's edge emits one resize event per intermediate width.
//! Painting each of them re-wraps the live tail at a width that is about to be
//! replaced — every frame of the drag misses the width-keyed wrap cache, which
//! is both visible re-wrap churn and wasted CPU. Hold the newest size until the
//! drag has stopped moving for [`RESIZE_QUIET_PERIOD`], then paint once.
//!
//! This layers with — and does not replace — the 75 ms history-replay debounce
//! in `coco_tui_ui::engine::history_reflow`: the viewport settles first, the
//! source-backed history replay follows.
//!
//! The deadline is tracked here rather than deferred to `FrameRequester`,
//! because the scheduler coalesces by *earliest* deadline
//! (`next_deadline.min(draw_at)`): repeated `schedule_frame_in` calls during a
//! drag would keep the first event's deadline instead of pushing it out, which
//! is a fixed delay, not a debounce.

use std::time::Duration;
use std::time::Instant;

use ratatui::layout::Size;

/// Matches grok's `RESIZE_DEBOUNCE`. Long enough for a continuous drag to
/// coalesce into one relayout, short enough that a single resize still feels
/// immediate.
pub(crate) const RESIZE_QUIET_PERIOD: Duration = Duration::from_millis(16);

/// Holds the most recent resize until its quiet period elapses.
#[derive(Debug, Default)]
pub(crate) struct ResizeDebounce {
    pending: Option<Pending>,
}

#[derive(Debug, Clone, Copy)]
struct Pending {
    size: Size,
    deadline: Instant,
}

/// What the frame path should do about the pending resize, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResizeAction {
    /// No resize in flight — paint normally.
    Idle,
    /// The drag stopped: adopt this size, then paint.
    Apply(Size),
    /// Still settling. Paint nothing at this doomed width; come back in
    /// `after`.
    Wait { after: Duration },
}

impl ResizeDebounce {
    /// Record a resize, restarting the quiet period. Returns how long to wait
    /// before asking again.
    pub(crate) fn observe(&mut self, size: Size, now: Instant) -> Duration {
        self.pending = Some(Pending {
            size,
            deadline: now + RESIZE_QUIET_PERIOD,
        });
        RESIZE_QUIET_PERIOD
    }

    /// Consume the pending size once its quiet period has elapsed.
    pub(crate) fn poll(&mut self, now: Instant) -> ResizeAction {
        match self.pending {
            None => ResizeAction::Idle,
            Some(pending) if now >= pending.deadline => {
                self.pending = None;
                ResizeAction::Apply(pending.size)
            }
            Some(pending) => ResizeAction::Wait {
                after: pending.deadline.saturating_duration_since(now),
            },
        }
    }

    /// Adopt any pending size immediately, bypassing the quiet period.
    ///
    /// For force-repaint paths — SIGCONT resume, focus heal — which must not
    /// paint a stale geometry and must not be delayed behind a drag.
    pub(crate) fn flush(&mut self) -> Option<Size> {
        self.pending.take().map(|pending| pending.size)
    }
}

#[cfg(test)]
#[path = "resize_debounce.test.rs"]
mod tests;
