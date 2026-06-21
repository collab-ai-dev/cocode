//! Search algorithm constants.
//!
//! Centralizes magic numbers used in search ranking and fusion.
//! These values are based on academic research and industry best practices.

/// Seconds in a day (60 * 60 * 24).
///
/// Used for time-based calculations like recency scoring.
pub const SECONDS_PER_DAY: f32 = 86400.0;

/// Natural log of 2, used for exponential decay calculations.
///
/// ln(2) = 0.693147180559945...
/// Used in half-life decay formula: score = exp(-ln(2) * age / half_life)
pub const LN_2: f32 = std::f32::consts::LN_2;

/// Default RRF (Reciprocal Rank Fusion) k parameter.
///
/// Standard value from the original RRF paper (Cormack et al., 2009).
/// Higher values give more weight to lower-ranked items.
/// Formula: score = weight / (rank + k)
pub const DEFAULT_RRF_K: f32 = 60.0;

/// Default recency decay half-life in days.
///
/// Files modified this many days ago have 50% recency score.
/// After 2 half-lives (14 days), score is 25%, etc.
pub const DEFAULT_RECENCY_HALF_LIFE_DAYS: f32 = 7.0;

#[cfg(test)]
#[path = "constants.test.rs"]
mod tests;
