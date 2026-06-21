//! Limits how frequently frame-draw notifications may be emitted.
//!
//! Widgets sometimes request frame draws (via the shell's `FrameRequester`)
//! more frequently than a user can perceive. This limiter clamps draw
//! notifications to a maximum of 120 FPS to avoid wasted work. Ported
//! verbatim from `codex-rs/tui/src/tui/frame_rate_limiter.rs`.

use std::time::Duration;
use std::time::Instant;

/// A 120 FPS minimum frame interval (≈8.33 ms).
pub const MIN_FRAME_INTERVAL: Duration = Duration::from_nanos(8_333_334);

/// Remembers the most recent emitted draw, allowing deadlines to be
/// clamped forward so we never exceed [`MIN_FRAME_INTERVAL`].
#[derive(Debug, Default)]
pub struct FrameRateLimiter {
    last_emitted_at: Option<Instant>,
}

impl FrameRateLimiter {
    /// Returns `requested`, clamped forward if it would exceed the
    /// maximum frame rate.
    pub fn clamp_deadline(&self, requested: Instant) -> Instant {
        let Some(last_emitted_at) = self.last_emitted_at else {
            return requested;
        };
        let min_allowed = last_emitted_at
            .checked_add(MIN_FRAME_INTERVAL)
            .unwrap_or(last_emitted_at);
        requested.max(min_allowed)
    }

    /// Records that a draw notification was emitted at `emitted_at`.
    pub fn mark_emitted(&mut self, emitted_at: Instant) {
        self.last_emitted_at = Some(emitted_at);
    }
}

#[cfg(test)]
#[path = "frame_rate_limiter.test.rs"]
mod tests;
