use super::FrameRateLimiter;
use super::MIN_FRAME_INTERVAL;
use pretty_assertions::assert_eq;
use std::time::Duration;
use std::time::Instant;

#[test]
fn default_does_not_clamp() {
    let t0 = Instant::now();
    let limiter = FrameRateLimiter::default();
    assert_eq!(limiter.clamp_deadline(t0), t0);
}

#[test]
fn clamps_to_min_interval_since_last_emit() {
    let t0 = Instant::now();
    let mut limiter = FrameRateLimiter::default();

    assert_eq!(limiter.clamp_deadline(t0), t0);
    limiter.mark_emitted(t0);

    let too_soon = t0 + Duration::from_millis(1);
    assert_eq!(limiter.clamp_deadline(too_soon), t0 + MIN_FRAME_INTERVAL);
}

#[test]
fn requested_in_the_future_passes_through_unclamped() {
    let t0 = Instant::now();
    let mut limiter = FrameRateLimiter::default();
    limiter.mark_emitted(t0);

    let well_in_the_future = t0 + Duration::from_millis(50);
    assert_eq!(
        limiter.clamp_deadline(well_in_the_future),
        well_in_the_future
    );
}
