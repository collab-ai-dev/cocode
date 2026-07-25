use super::*;
use pretty_assertions::assert_eq;

const FRAMES: &[&str] = &["a", "b", "c"];

#[test]
fn test_motion_mode_defaults_to_animated() {
    assert_eq!(MotionMode::default(), MotionMode::Animated);
    assert!(MotionMode::default().is_animated());
}

#[test]
fn test_from_animations_enabled_maps_both_ways() {
    assert_eq!(
        MotionMode::from_animations_enabled(true),
        MotionMode::Animated
    );
    assert_eq!(
        MotionMode::from_animations_enabled(false),
        MotionMode::Reduced
    );
}

#[test]
fn test_animation_frame_cycles_frames_when_animated() {
    let frame = |elapsed_ms| {
        animation_frame(
            FRAMES,
            /*interval_ms*/ 100,
            elapsed_ms,
            MotionMode::Animated,
            ReducedMotionIndicator::Hidden,
        )
    };
    assert_eq!(frame(0), Some("a"));
    assert_eq!(frame(150), Some("b"));
    assert_eq!(frame(250), Some("c"));
    assert_eq!(frame(300), Some("a"));
}

#[test]
fn test_animation_frame_clamps_negative_elapsed() {
    assert_eq!(
        animation_frame(
            FRAMES,
            /*interval_ms*/ 100,
            /*elapsed_ms*/ -500,
            MotionMode::Animated,
            ReducedMotionIndicator::Hidden,
        ),
        Some("a")
    );
}

/// Turning animation off must not silently delete an affordance: the call site
/// says what stands in for it.
#[test]
fn test_animation_frame_uses_the_declared_reduced_fallback() {
    assert_eq!(
        animation_frame(
            FRAMES,
            /*interval_ms*/ 100,
            /*elapsed_ms*/ 150,
            MotionMode::Reduced,
            ReducedMotionIndicator::StaticGlyph(STATIC_ACTIVITY_GLYPH),
        ),
        Some(STATIC_ACTIVITY_GLYPH)
    );
    assert_eq!(
        animation_frame(
            FRAMES,
            /*interval_ms*/ 100,
            /*elapsed_ms*/ 150,
            MotionMode::Reduced,
            ReducedMotionIndicator::Hidden,
        ),
        None
    );
}

#[test]
fn test_animation_frame_reduced_output_does_not_vary_with_time() {
    let frame = |elapsed_ms| {
        animation_frame(
            FRAMES,
            /*interval_ms*/ 100,
            elapsed_ms,
            MotionMode::Reduced,
            ReducedMotionIndicator::StaticGlyph(STATIC_ACTIVITY_GLYPH),
        )
    };
    assert_eq!(frame(0), frame(10_000));
}

#[test]
fn test_animation_frame_handles_a_degenerate_frame_table() {
    assert_eq!(
        animation_frame(
            &[],
            /*interval_ms*/ 100,
            /*elapsed_ms*/ 0,
            MotionMode::Animated,
            ReducedMotionIndicator::Hidden,
        ),
        None
    );
    assert_eq!(
        animation_frame(
            FRAMES,
            /*interval_ms*/ 0,
            /*elapsed_ms*/ 0,
            MotionMode::Animated,
            ReducedMotionIndicator::Hidden,
        ),
        None
    );
}

/// Reduced motion still has to tick, or every elapsed-time readout freezes.
#[test]
fn test_frame_interval_keeps_a_slow_tick_under_reduced_motion() {
    let animated = Duration::from_millis(80);
    assert_eq!(MotionMode::Animated.frame_interval(animated), animated);
    assert_eq!(
        MotionMode::Reduced.frame_interval(animated),
        REDUCED_MOTION_FRAME_INTERVAL
    );
}

/// A call site that already ticks slower than the reduced cadence keeps its own.
#[test]
fn test_frame_interval_never_speeds_a_slow_call_site_up() {
    let slow = Duration::from_secs(5);
    assert_eq!(MotionMode::Reduced.frame_interval(slow), slow);
}
