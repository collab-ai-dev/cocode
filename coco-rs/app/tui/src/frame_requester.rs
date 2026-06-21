//! Frame-draw scheduling utilities for the TUI.
//!
//! This module exposes [`FrameRequester`], a lightweight handle that
//! widgets and background tasks can clone to request future redraws of
//! the TUI. Internally it spawns a [`FrameScheduler`] task that
//! coalesces many requests into a single notification on a broadcast
//! channel used by the main event loop. This keeps animations and
//! status updates smooth without redrawing more often than necessary,
//! and lets idle frames cost nothing.
//!
//! Follows the actor-style design described in
//! [“Actors with Tokio”](https://ryhl.io/blog/actors-with-tokio/) — a
//! dedicated scheduler task plus a lightweight request handle.
//!
//! Ported from `codex-rs/tui/src/tui/frame_requester.rs`. The only
//! coco-rs adjustments are the path-based test layout and `pub(crate)`
//! visibility (no out-of-crate consumer).

use std::time::Duration;
use std::time::Instant;

use tokio::sync::broadcast;
use tokio::sync::mpsc;

use coco_tui_ui::frame_rate_limiter::FrameRateLimiter;

/// Requester for scheduling future frame draws on the TUI event loop.
///
/// Handler half of an actor/handler pair with [`FrameScheduler`], which
/// coalesces multiple frame requests into a single draw notification.
///
/// Clones of this type can be freely shared across tasks so any code
/// path can trigger a redraw.
#[derive(Clone, Debug)]
pub(crate) struct FrameRequester {
    frame_schedule_tx: mpsc::UnboundedSender<Instant>,
}

impl FrameRequester {
    /// Create a new [`FrameRequester`] and spawn its associated
    /// [`FrameScheduler`] task.
    ///
    /// `draw_tx` is used to notify the TUI event loop of scheduled
    /// draws.
    pub(crate) fn new(draw_tx: broadcast::Sender<()>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let scheduler = FrameScheduler::new(rx, draw_tx);
        tokio::spawn(scheduler.run());
        Self {
            frame_schedule_tx: tx,
        }
    }

    /// Schedule a frame draw as soon as possible.
    pub(crate) fn schedule_frame(&self) {
        let _ = self.frame_schedule_tx.send(Instant::now());
    }

    /// Schedule a frame draw to occur after the specified duration.
    /// Used by `App::redraw` to self-arm the next spinner frame while
    /// a turn or stream is in flight, so the runtime sleeps between
    /// paints instead of riding a 50 ms wall-clock interval.
    pub(crate) fn schedule_frame_in(&self, dur: Duration) {
        let _ = self.frame_schedule_tx.send(Instant::now() + dur);
    }
}

#[cfg(test)]
impl FrameRequester {
    /// Create a no-op frame requester for tests.
    pub(crate) fn test_dummy() -> Self {
        let (tx, _rx) = mpsc::unbounded_channel();
        FrameRequester {
            frame_schedule_tx: tx,
        }
    }
}

/// Internal coalescing scheduler.
///
/// Spawned as a task. Draw notifications are clamped to a maximum of
/// 120 FPS via [`FrameRateLimiter`]. Idle ticks cost nothing — when no
/// frames are pending the scheduler sleeps for an effectively infinite
/// duration.
struct FrameScheduler {
    receiver: mpsc::UnboundedReceiver<Instant>,
    draw_tx: broadcast::Sender<()>,
    rate_limiter: FrameRateLimiter,
}

impl FrameScheduler {
    fn new(receiver: mpsc::UnboundedReceiver<Instant>, draw_tx: broadcast::Sender<()>) -> Self {
        Self {
            receiver,
            draw_tx,
            rate_limiter: FrameRateLimiter::default(),
        }
    }

    async fn run(mut self) {
        const ONE_YEAR: Duration = Duration::from_secs(60 * 60 * 24 * 365);
        let mut next_deadline: Option<Instant> = None;
        loop {
            let target = next_deadline.unwrap_or_else(|| Instant::now() + ONE_YEAR);
            let deadline = tokio::time::sleep_until(target.into());
            tokio::pin!(deadline);

            tokio::select! {
                draw_at = self.receiver.recv() => {
                    let Some(draw_at) = draw_at else {
                        // All senders dropped; exit cleanly.
                        break;
                    };
                    let draw_at = self.rate_limiter.clamp_deadline(draw_at);
                    next_deadline = Some(next_deadline.map_or(draw_at, |cur| cur.min(draw_at)));
                    // Continue without sending so the sleep branch
                    // fires once for any coalesced batch.
                    continue;
                }
                _ = &mut deadline => {
                    if next_deadline.is_some() {
                        next_deadline = None;
                        self.rate_limiter.mark_emitted(target);
                        let _ = self.draw_tx.send(());
                    }
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "frame_requester.test.rs"]
mod tests;
