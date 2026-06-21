//! KAIROS midnight-rollover detector.
//!
//! Holds a "last emitted date" latch and compares against the local
//! ISO date each turn; on flip it (a) emits a `date_change` attachment
//! for the model (handled separately via
//! [`core/system-reminder::DateChangeGenerator`]) and (b) under the
//! KAIROS feature invokes the per-day flush.
//!
//! This watcher owns **only** the KAIROS-side latch. The generic
//! `date_change` reminder has its own latch on the engine
//! ([`coco_state::ToolAppState::last_emitted_date`]) which fires for
//! every session regardless of mode. Two latches because they tick
//! independently: the reminder must fire on session-resume even when
//! the rollover happened while no turn was running, while the
//! KAIROS-side flush is per-turn and only meaningful inside an active
//! turn whose model can still see the new day.
//!
//! Why a sync mutex around `Option<NaiveDate>`: the slot updates on
//! every finalize-turn (low frequency), the read is constant-time,
//! and using `tokio::sync` here would force callers to hold an
//! awaitable lock across the runtime fan-out's `tokio::join!`. Plain
//! `std::sync::Mutex` keeps the lock acquisition synchronous and
//! contention-free in practice.

use std::sync::Mutex;

use chrono::DateTime;
use chrono::Local;
use chrono::NaiveDate;
use chrono::TimeZone;

/// Per-session "last seen local date" latch for KAIROS daily logs.
/// Returns the **previous** date (i.e. yesterday) when the calendar
/// day has flipped relative to the stored latch, [`None`] otherwise.
///
/// Returned date semantics: the watcher reports the day that just
/// ended (yesterday), not the new day. Callers archive yesterday's
/// transcript / log bucket on the signal.
#[derive(Debug, Default)]
pub struct KairosRolloverWatcher {
    /// `None` means "first finalize-turn since process start" — the
    /// watcher seeds the latch silently and yields `None` (no rollover
    /// yet). Subsequent calls compare against the stored value.
    last_seen: Mutex<Option<NaiveDate>>,
}

impl KairosRolloverWatcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// Compare `now_ms` against the stored latch.
    ///
    /// First call seeds with today's local date and returns `None`.
    /// Subsequent calls return `Some(previous_date)` only when the
    /// local calendar day has flipped; the latch advances to the new
    /// day in that case.
    pub fn tick(&self, now_ms: i64) -> Option<NaiveDate> {
        let today = local_date_from_millis(now_ms)?;
        let mut guard = self.last_seen.lock().ok()?;
        match *guard {
            None => {
                *guard = Some(today);
                None
            }
            Some(prev) if prev == today => None,
            Some(prev) => {
                *guard = Some(today);
                Some(prev)
            }
        }
    }

    /// Test helper: seed the latch with a specific date so the next
    /// `tick` can be made deterministic.
    #[cfg(test)]
    pub(crate) fn seed(&self, date: NaiveDate) {
        if let Ok(mut g) = self.last_seen.lock() {
            *g = Some(date);
        }
    }
}

/// Convert a UTC millisecond timestamp into a local-calendar
/// [`NaiveDate`]. The model sees the user's wall-clock date, not UTC.
/// Returns `None` only on
/// pathologically-large `now_ms` (>year 200,000) which can't occur in
/// practice; callers treat `None` as "skip this tick" rather than
/// panicking.
fn local_date_from_millis(now_ms: i64) -> Option<NaiveDate> {
    let utc: DateTime<chrono::Utc> = chrono::Utc.timestamp_millis_opt(now_ms).single()?;
    Some(utc.with_timezone(&Local).date_naive())
}

#[cfg(test)]
#[path = "rollover.test.rs"]
mod tests;
