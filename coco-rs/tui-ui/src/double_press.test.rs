use std::time::Duration;
use std::time::Instant;

use pretty_assertions::assert_eq;

use super::DoublePressTracker;
use super::Outcome;

const WINDOW: Duration = Duration::from_millis(800);

fn new<K: Copy + PartialEq>() -> DoublePressTracker<K> {
    DoublePressTracker::new(WINDOW)
}

#[test]
fn first_press_arms_and_returns_first() {
    let mut t = new::<u8>();
    let now = Instant::now();
    assert_eq!(t.poll(1, now), Outcome::First);
    assert_eq!(t.pending(), Some(&1));
}

#[test]
fn second_press_within_window_returns_double_and_resets() {
    let mut t = new::<u8>();
    let t0 = Instant::now();
    t.poll(1, t0);
    let t1 = t0 + Duration::from_millis(500);
    assert_eq!(t.poll(1, t1), Outcome::Double);
    assert_eq!(t.pending(), None);
}

#[test]
fn second_press_at_exact_boundary_still_fires() {
    let mut t = new::<u8>();
    let t0 = Instant::now();
    t.poll(1, t0);
    // `now <= until` — the boundary itself is a Double, not a First.
    assert_eq!(t.poll(1, t0 + WINDOW), Outcome::Double);
}

#[test]
fn second_press_after_window_is_a_fresh_first() {
    let mut t = new::<u8>();
    let t0 = Instant::now();
    t.poll(1, t0);
    let t1 = t0 + WINDOW + Duration::from_millis(1);
    assert_eq!(t.poll(1, t1), Outcome::First);
    // Still armed — the new arm starts fresh, not the old one.
    assert_eq!(t.pending(), Some(&1));
}

#[test]
fn different_key_within_window_re_arms_as_first() {
    let mut t = new::<u8>();
    let t0 = Instant::now();
    t.poll(1, t0);
    let t1 = t0 + Duration::from_millis(50);
    // Ctrl+D while Ctrl+C armed: re-arm Ctrl+D, do NOT fire double.
    assert_eq!(t.poll(2, t1), Outcome::First);
    assert_eq!(t.pending(), Some(&2));
}

#[test]
fn double_then_first_is_a_fresh_arm() {
    let mut t = new::<u8>();
    let t0 = Instant::now();
    t.poll(1, t0);
    t.poll(1, t0 + Duration::from_millis(100)); // Double
    // Third press starts a new sequence.
    let t2 = t0 + Duration::from_millis(200);
    assert_eq!(t.poll(1, t2), Outcome::First);
}

#[test]
fn reset_clears_arm() {
    let mut t = new::<u8>();
    let now = Instant::now();
    t.poll(1, now);
    t.reset();
    assert_eq!(t.pending(), None);
    // A subsequent press is a fresh First, not a Double.
    assert_eq!(t.poll(1, now + Duration::from_millis(50)), Outcome::First);
}

#[test]
fn tick_after_window_clears_and_returns_true() {
    let mut t = new::<u8>();
    let t0 = Instant::now();
    t.poll(1, t0);
    assert!(!t.tick(t0 + WINDOW)); // exact boundary: still armed
    assert_eq!(t.pending(), Some(&1));
    assert!(t.tick(t0 + WINDOW + Duration::from_millis(1)));
    assert_eq!(t.pending(), None);
}

#[test]
fn tick_while_idle_is_a_no_op() {
    let mut t = new::<u8>();
    assert!(!t.tick(Instant::now()));
    assert!(!t.tick(Instant::now() + Duration::from_secs(10)));
}

#[test]
fn tick_within_window_keeps_arm() {
    let mut t = new::<u8>();
    let t0 = Instant::now();
    t.poll(1, t0);
    assert!(!t.tick(t0 + Duration::from_millis(100)));
    assert_eq!(t.pending(), Some(&1));
}

#[test]
fn unit_key_works_for_single_key_trackers() {
    // The Esc + Ctrl+C + Ctrl+D trackers each use `()` — there is only
    // one key per tracker, but we still want the double-press semantics.
    // Verifies the bounds aren't accidentally exclusionary.
    let mut t = new::<()>();
    let now = Instant::now();
    assert_eq!(t.poll((), now), Outcome::First);
    assert_eq!(
        t.poll((), now + Duration::from_millis(100)),
        Outcome::Double
    );
}

#[test]
fn pending_until_reflects_arm_state() {
    let mut t = new::<()>();
    assert_eq!(t.pending_until(), None);
    let t0 = Instant::now();
    t.poll((), t0);
    assert_eq!(t.pending_until(), Some(t0 + WINDOW));
    t.reset();
    assert_eq!(t.pending_until(), None);
}
