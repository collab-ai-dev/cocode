//! Time source abstraction for the TUI.
//!
//! Production code reads "now" through an `Arc<dyn Clock>` stashed on
//! `AppState` so tests can substitute a controllable [`MockClock`].
//! Avoids the `thread_local!` mock-clock pattern, which silently
//! fails as soon as any code path crosses an `await` boundary in a
//! multi-thread tokio runtime or spawns a sibling task — both of
//! which exist in this crate (`FrameScheduler`, `git_index_watcher`,
//! `theme::watcher`, `autocomplete::*_search`).
//!
//! Scope: only state-observable time reads — todo-panel sort, plan
//! task completion timestamps, status-clock pause / elapsed. Internal
//! scheduling timers (`FrameScheduler`, `history_reflow` debounce,
//! `keybinding_resolver::tick`) keep using raw `Instant::now()` /
//! `tokio::time` because there is no test that asserts behaviour at
//! a specific wall-clock instant for them.

use std::sync::Arc;
use std::time::Instant;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

/// Read-only clock abstraction. Production uses [`SystemClock`];
/// tests use [`MockClock`]. The trait bound includes `Send + Sync`
/// so an `Arc<dyn Clock>` survives `tokio::spawn` and crosses worker
/// threads, plus `Debug` so the holder (`AppState`) keeps its
/// auto-derived `Debug` impl.
pub trait Clock: Send + Sync + std::fmt::Debug {
    /// Monotonic-ish [`Instant`] reading. Used by render-time
    /// arithmetic on the status-indicator clock and any other
    /// elapsed-since-start computation. `Instant` cannot be created
    /// from raw `i64`, so the mock impl just freezes the underlying
    /// real value at construction time and offsets from there.
    fn now(&self) -> Instant;

    /// Unix-epoch millisecond reading. Used by the V2 task panel
    /// (completion-timestamp lift, all-completed hide) and the
    /// subagent / task start stamps. Wraps to 0 on systems with a
    /// clock behind the epoch.
    fn now_ms(&self) -> i64;
}

/// Production clock — reads the OS clock directly.
#[derive(Debug, Default)]
pub struct SystemClock;

impl SystemClock {
    /// Default Arc-wrapped instance for AppState construction.
    pub fn arc() -> Arc<dyn Clock> {
        Arc::new(SystemClock)
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn now_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }
}

/// Test clock — both `now()` and `now_ms()` come from a single
/// adjustable offset so tests can pin time to a specific instant and
/// step it forward deterministically.
///
/// `Instant` cannot be constructed from a chosen `i64`, so the mock
/// pins a real `Instant` at construction (`base_instant`) and offsets
/// from there. `now_ms()` is fully synthetic — tests set whatever
/// epoch-ms value they need.
#[cfg(any(test, feature = "testing"))]
#[derive(Debug)]
pub struct MockClock {
    base_instant: Instant,
    offset: std::sync::Mutex<MockOffset>,
}

#[cfg(any(test, feature = "testing"))]
#[derive(Debug, Clone, Copy)]
struct MockOffset {
    instant_offset_ms: i64,
    now_ms: i64,
}

#[cfg(any(test, feature = "testing"))]
#[allow(clippy::expect_used)] // mock-only: a poisoned lock is unreachable in tests
impl MockClock {
    /// Pin time to `now_ms` (epoch milliseconds). Subsequent
    /// [`Self::advance`] calls shift both [`Clock::now`] and
    /// [`Clock::now_ms`] forward by the same amount, keeping the
    /// two reads coherent.
    pub fn new(now_ms: i64) -> Self {
        Self {
            base_instant: Instant::now(),
            offset: std::sync::Mutex::new(MockOffset {
                instant_offset_ms: 0,
                now_ms,
            }),
        }
    }

    /// Advance the mock clock by `delta_ms` milliseconds (positive
    /// or negative). Both `now()` and `now_ms()` shift.
    pub fn advance(&self, delta_ms: i64) {
        let mut o = self.offset.lock().expect("mock clock poisoned");
        o.instant_offset_ms = o.instant_offset_ms.saturating_add(delta_ms);
        o.now_ms = o.now_ms.saturating_add(delta_ms);
    }

    /// Convenience for tests that want an `Arc<dyn Clock>`.
    pub fn arc(now_ms: i64) -> Arc<dyn Clock> {
        Arc::new(Self::new(now_ms))
    }
}

#[cfg(any(test, feature = "testing"))]
#[allow(clippy::expect_used)] // mock-only: a poisoned lock is unreachable in tests
impl Clock for MockClock {
    fn now(&self) -> Instant {
        let offset_ms = self
            .offset
            .lock()
            .expect("mock clock poisoned")
            .instant_offset_ms;
        if offset_ms >= 0 {
            self.base_instant + std::time::Duration::from_millis(offset_ms as u64)
        } else {
            self.base_instant - std::time::Duration::from_millis((-offset_ms) as u64)
        }
    }

    fn now_ms(&self) -> i64 {
        self.offset.lock().expect("mock clock poisoned").now_ms
    }
}

#[cfg(test)]
#[path = "clock.test.rs"]
mod tests;
