use std::time::Duration;
use std::time::Instant;

use pretty_assertions::assert_eq;
use ratatui::layout::Size;

use super::RESIZE_QUIET_PERIOD;
use super::ResizeAction;
use super::ResizeDebounce;

#[test]
fn idle_without_a_pending_resize() {
    let mut debounce = ResizeDebounce::default();
    assert_eq!(debounce.poll(Instant::now()), ResizeAction::Idle);
}

#[test]
fn burst_of_resizes_applies_only_the_last_size_once() {
    // The drag: a burst of intermediate widths inside one quiet period must
    // collapse to exactly one applied size — the newest — and one paint.
    let mut debounce = ResizeDebounce::default();
    let start = Instant::now();
    for (offset_ms, width) in [(0, 100), (4, 90), (8, 80), (12, 70)] {
        let now = start + Duration::from_millis(offset_ms);
        debounce.observe(Size::new(width, 24), now);
        assert_eq!(
            debounce.poll(now),
            ResizeAction::Wait {
                after: RESIZE_QUIET_PERIOD
            },
            "a resize at +{offset_ms}ms must still be settling"
        );
    }

    // Each event restarted the period, so it expires 16 ms after the LAST one.
    let settled = start + Duration::from_millis(12) + RESIZE_QUIET_PERIOD;
    assert_eq!(
        debounce.poll(settled),
        ResizeAction::Apply(Size::new(70, 24)),
        "only the newest size may be applied"
    );
    assert_eq!(
        debounce.poll(settled),
        ResizeAction::Idle,
        "the applied resize must not fire twice"
    );
}

#[test]
fn a_newer_resize_pushes_the_deadline_out() {
    // Guards the reason this state lives here rather than in FrameRequester,
    // which coalesces by EARLIEST deadline: a debounce must extend, not keep
    // the first event's deadline.
    let mut debounce = ResizeDebounce::default();
    let start = Instant::now();
    debounce.observe(Size::new(100, 24), start);

    let later = start + Duration::from_millis(10);
    debounce.observe(Size::new(90, 24), later);
    // The first event's deadline has now passed; the second's has not.
    assert_eq!(
        debounce.poll(start + RESIZE_QUIET_PERIOD),
        ResizeAction::Wait {
            after: Duration::from_millis(10)
        },
        "a newer resize must extend the quiet period past the first deadline"
    );
    assert_eq!(
        debounce.poll(later + RESIZE_QUIET_PERIOD),
        ResizeAction::Apply(Size::new(90, 24))
    );
}

#[test]
fn a_single_resize_applies_after_one_quiet_period() {
    let mut debounce = ResizeDebounce::default();
    let start = Instant::now();
    debounce.observe(Size::new(120, 40), start);
    assert_eq!(
        debounce.poll(start + RESIZE_QUIET_PERIOD),
        ResizeAction::Apply(Size::new(120, 40))
    );
}

#[test]
fn flush_adopts_a_pending_size_immediately() {
    // The force-repaint bypass: a resume or focus heal must not paint a stale
    // geometry, nor wait behind an in-flight drag.
    let mut debounce = ResizeDebounce::default();
    let now = Instant::now();
    debounce.observe(Size::new(100, 24), now);

    assert_eq!(debounce.flush(), Some(Size::new(100, 24)));
    assert_eq!(
        debounce.poll(now),
        ResizeAction::Idle,
        "a flushed resize must not also fire on its deadline"
    );
    assert_eq!(debounce.flush(), None, "nothing pending to flush");
}
