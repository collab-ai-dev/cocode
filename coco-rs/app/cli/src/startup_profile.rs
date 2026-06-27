//! Optional startup-phase profiler — enabled by `COCO_STARTUP_PROFILE`.
//!
//! Low-cost regression insurance for boot latency (modeled on jcode's
//! `startup_profile`). When enabled, [`init`] records `process_start`, [`mark`]
//! stamps named milestones, and [`report`] emits one `tracing::debug!` per phase
//! (delta + cumulative `duration_ms`) under the `coco_cli::startup` target, plus
//! a `total_ms` summary. Disabled by default: `init` leaves the profile unset,
//! so `mark`/`report` are no-ops.
//!
//! Surface it with `COCO_STARTUP_PROFILE=1 coco --log-level=debug` (or any sink
//! that captures the `coco_cli::startup` target at debug).

use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::PoisonError;
use std::time::Duration;
use std::time::Instant;

use coco_config::EnvKey;

static PROFILE: Mutex<Option<Profile>> = Mutex::new(None);

struct Profile {
    start: Instant,
    marks: Vec<(&'static str, Instant)>,
}

/// Pure truthiness test for the env value (kept separate from [`enabled`] so it
/// is unit-testable without mutating process env).
fn enabled_from(value: Option<&str>) -> bool {
    value.is_some_and(|v| {
        let v = v.trim();
        !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false")
    })
}

/// Whether `COCO_STARTUP_PROFILE` requests startup timings.
pub fn enabled() -> bool {
    enabled_from(coco_config::env::env_opt(EnvKey::CocoStartupProfile).as_deref())
}

fn lock() -> MutexGuard<'static, Option<Profile>> {
    PROFILE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Begin profiling. No-op (and leaves `mark`/`report` inert) unless [`enabled`].
pub fn init() {
    if !enabled() {
        return;
    }
    let now = Instant::now();
    *lock() = Some(Profile {
        start: now,
        marks: vec![("process_start", now)],
    });
}

/// Stamp a named milestone. No-op unless profiling is active.
pub fn mark(name: &'static str) {
    if let Some(profile) = lock().as_mut() {
        profile.marks.push((name, Instant::now()));
    }
}

/// Emit the recorded phases as `debug!` lines. No-op unless profiling is active.
pub fn report() {
    let guard = lock();
    let Some(profile) = guard.as_ref() else {
        return;
    };
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    for window in profile.marks.windows(2) {
        let (_, prev) = window[0];
        let (name, at) = window[1];
        tracing::debug!(
            target: "coco_cli::startup",
            phase = name,
            duration_ms = ms(at.duration_since(prev)),
            from_start_ms = ms(at.duration_since(profile.start)),
            "startup phase"
        );
    }
    if let Some((_, last)) = profile.marks.last() {
        tracing::debug!(
            target: "coco_cli::startup",
            total_ms = ms(last.duration_since(profile.start)),
            phases = profile.marks.len().saturating_sub(1),
            "startup profile complete"
        );
    }
}

#[cfg(test)]
#[path = "startup_profile.test.rs"]
mod tests;
