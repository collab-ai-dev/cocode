//! Motion policy for time-varying UI.
//!
//! Continuous animation is not free for everyone: it is a vestibular trigger
//! for some users, it makes a screen reader re-announce a line that has not
//! actually changed, and it turns a captured terminal log into noise. Every
//! animated surface therefore routes through [`MotionMode`], and every call
//! site must name what it shows instead — there is no implicit "animation off
//! means nothing renders", because a missing spinner reads as "coco is stuck".

use std::time::Duration;

/// Whether time-varying UI may animate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum MotionMode {
    /// Animate normally.
    #[default]
    Animated,
    /// Hold still: render one static frame and let the surrounding text carry
    /// the "work is in progress" signal.
    Reduced,
}

/// What an animated call site renders under [`MotionMode::Reduced`].
///
/// Named per call site rather than defaulted, so turning animation off can
/// never silently delete an affordance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReducedMotionIndicator {
    /// Render nothing — something adjacent already shows the activity.
    Hidden,
    /// Render one static glyph holding the animation's place.
    StaticGlyph(&'static str),
}

/// Glyph shown where a spinner would otherwise turn.
pub const STATIC_ACTIVITY_GLYPH: &str = "•";

impl MotionMode {
    pub fn from_animations_enabled(animations_enabled: bool) -> Self {
        if animations_enabled {
            Self::Animated
        } else {
            Self::Reduced
        }
    }

    pub fn is_animated(self) -> bool {
        matches!(self, Self::Animated)
    }

    /// Frame cadence to schedule under this mode.
    ///
    /// Reduced motion still needs a slow tick: elapsed-time readouts advance
    /// once a second whether or not anything spins, and dropping the tick
    /// entirely would freeze them.
    pub fn frame_interval(self, animated: Duration) -> Duration {
        match self {
            Self::Animated => animated,
            Self::Reduced => REDUCED_MOTION_FRAME_INTERVAL.max(animated),
        }
    }
}

/// Cadence under reduced motion: fast enough that a one-second elapsed readout
/// never looks stalled, slow enough to be invisible as motion.
const REDUCED_MOTION_FRAME_INTERVAL: Duration = Duration::from_millis(500);

/// Pick the animation frame for `elapsed_ms`, or the reduced-motion stand-in.
///
/// `frames` is the animation's frame table and `interval_ms` its period.
pub fn animation_frame(
    frames: &'static [&'static str],
    interval_ms: i64,
    elapsed_ms: i64,
    mode: MotionMode,
    reduced: ReducedMotionIndicator,
) -> Option<&'static str> {
    match mode {
        MotionMode::Animated => {
            if frames.is_empty() || interval_ms <= 0 {
                return None;
            }
            let index = (elapsed_ms.max(0) / interval_ms) % frames.len() as i64;
            frames.get(index as usize).copied()
        }
        MotionMode::Reduced => match reduced {
            ReducedMotionIndicator::Hidden => None,
            ReducedMotionIndicator::StaticGlyph(glyph) => Some(glyph),
        },
    }
}

#[cfg(test)]
#[path = "motion.test.rs"]
mod tests;
