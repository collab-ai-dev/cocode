//! Wall-clock epoch helper.

use std::time::{SystemTime, UNIX_EPOCH};

/// Current Unix time in milliseconds, or `None` when the system clock is set
/// before the epoch (pre-1970).
///
/// This is a single, shared source of truth for epoch timestamps so the
/// refuse-on-None correctness contract isn't re-derived per crate: callers
/// that persist timestamps used for recency decay or ordering MUST refuse to
/// record on `None` (substituting `0` would poison the math). Callers that
/// need an infallible value apply their own explicit fallback at the edge
/// (e.g. `now_epoch_ms().unwrap_or(0)`).
pub fn now_epoch_ms() -> Option<i64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as i64)
}
