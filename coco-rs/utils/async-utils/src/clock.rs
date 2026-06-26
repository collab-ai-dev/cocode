//! Injectable clock seam for deterministic time in tests.
//!
//! Ported from opencode's `TestClock` discipline (time-dependent flows advance
//! virtual time instead of sleeping). coco-rs already has a TUI-scoped clock in
//! `coco-tui-ui`, but that crate is a leaf UI primitive other layers must not
//! depend on. This is the workspace-shared version: production code takes a
//! `&dyn Clock` (or `Arc<dyn Clock>`) and reads time through it; tests inject
//! [`TestClock`] to pin and step time forward, so expiry/backoff/TTL logic is
//! verified without wall-clock flakiness.
//!
//! `TestClock` is gated behind the `testing` feature so it never ships in
//! release; downstream crates enable it in `[dev-dependencies]` (the same
//! pattern `app/tui` uses for `coco-tui-ui`'s `testing` feature).

use std::time::Instant;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

/// Read-only clock abstraction. Production uses [`SystemClock`]; tests use
/// [`TestClock`]. `Send + Sync + Debug` so an `Arc<dyn Clock>` can live on a
/// shared struct, cross threads, and not break a derived `Debug`.
pub trait Clock: Send + Sync + std::fmt::Debug {
    /// Monotonic [`Instant`] for elapsed-since arithmetic (backoff, throttles).
    fn now(&self) -> Instant;

    /// Wall-clock Unix time in milliseconds, for absolute deadlines (token
    /// expiry, TTLs). `i64` per the workspace integer convention; wraps to 0
    /// on systems whose clock is behind the epoch.
    fn now_unix_millis(&self) -> i64;
}

/// Production clock — reads the OS clock directly.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl SystemClock {
    /// `Arc`-wrapped instance for structs that hold an `Arc<dyn Clock>`.
    pub fn arc() -> std::sync::Arc<dyn Clock> {
        std::sync::Arc::new(SystemClock)
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn now_unix_millis(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }
}

#[cfg(any(test, feature = "testing"))]
mod test_clock {
    use super::Clock;
    use std::sync::Mutex;
    use std::time::Duration;
    use std::time::Instant;

    #[derive(Debug, Clone, Copy)]
    struct Offset {
        instant_offset_ms: i64,
        unix_ms: i64,
    }

    /// Test clock — both [`Clock::now`] and [`Clock::now_unix_millis`] derive
    /// from one adjustable offset, so a test pins time at construction and
    /// steps it with [`TestClock::advance_millis`], keeping the two reads
    /// coherent. `Instant` can't be built from a chosen value, so `now()` pins
    /// a real `Instant` at construction and offsets from there.
    #[derive(Debug)]
    pub struct TestClock {
        base_instant: Instant,
        offset: Mutex<Offset>,
    }

    impl TestClock {
        /// Pin time to `unix_ms` (epoch milliseconds), offset 0.
        pub fn new(unix_ms: i64) -> Self {
            Self {
                base_instant: Instant::now(),
                offset: Mutex::new(Offset {
                    instant_offset_ms: 0,
                    unix_ms,
                }),
            }
        }

        /// Step both clocks forward (or back, if negative) by `delta_ms`.
        pub fn advance_millis(&self, delta_ms: i64) {
            let mut o = self
                .offset
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            o.instant_offset_ms = o.instant_offset_ms.saturating_add(delta_ms);
            o.unix_ms = o.unix_ms.saturating_add(delta_ms);
        }

        /// Convenience for callers wanting an `Arc<dyn Clock>`.
        pub fn arc(unix_ms: i64) -> std::sync::Arc<dyn Clock> {
            std::sync::Arc::new(Self::new(unix_ms))
        }
    }

    impl Clock for TestClock {
        fn now(&self) -> Instant {
            let off = self
                .offset
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .instant_offset_ms;
            if off >= 0 {
                self.base_instant + Duration::from_millis(off as u64)
            } else {
                self.base_instant - Duration::from_millis((-off) as u64)
            }
        }

        fn now_unix_millis(&self) -> i64 {
            self.offset
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .unix_ms
        }
    }
}

#[cfg(any(test, feature = "testing"))]
pub use test_clock::TestClock;

#[cfg(test)]
#[path = "clock.test.rs"]
mod tests;
