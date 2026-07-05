//! Safe, feature-gated jemalloc arena purge + stats.
//!
//! The unsafe `mallctl` FFI is isolated in this crate per the workspace
//! "wrap unsafe deps in their own crate" rule; every other crate calls only
//! the safe API here. With the `jemalloc` feature OFF (the default) the crate
//! has no jemalloc dependency and every entry point is a trivial stub, so
//! non-jemalloc builds link nothing.
//!
//! The feature is meant to move in lockstep with installing jemalloc as the
//! global allocator (the `coco` binary's own `jemalloc` feature does both):
//! reading `stats.*` or purging arenas is only meaningful when jemalloc is the
//! allocator actually managing the process heap.

/// The concrete jemalloc backend. Present only when the feature is on AND the
/// target is non-Windows — jemalloc-sys has no MSVC build, so the deps are
/// absent there even if the feature flag is set transitively.
#[cfg(all(feature = "jemalloc", not(target_os = "windows")))]
mod imp;

/// Whether this build links the real jemalloc control backend. `false` makes
/// [`stats_snapshot`] return `None` and [`purge_all_arenas`] a no-op, so
/// callers never need their own `cfg`.
pub const ENABLED: bool = cfg!(all(feature = "jemalloc", not(target_os = "windows")));

/// A point-in-time snapshot of jemalloc's global allocator statistics.
///
/// Values are bytes. They are only current as of an `epoch` advance, which
/// [`stats_snapshot`] performs before reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JemallocStats {
    /// `stats.allocated`: bytes in live application allocations.
    pub allocated: u64,
    /// `stats.active`: bytes in pages backing active allocations.
    pub active: u64,
    /// `stats.resident`: bytes physically resident — the number that drives
    /// process RSS / physical-footprint pressure and that a purge shrinks.
    pub resident: u64,
    /// `stats.retained`: virtual bytes retained from the OS but not resident
    /// (address space kept for reuse, not counted against physical footprint).
    pub retained: u64,
}

/// Errors from a raw `mallctl` call. Boundary-tier (`thiserror`); main-trunk
/// callers convert at their edge if they need a `coco-error` classification.
#[derive(Debug, thiserror::Error)]
pub enum JemallocError {
    /// A `mallctl(name)` returned a non-zero errno.
    #[error("jemalloc mallctl({name}) failed: errno {code}")]
    Mallctl {
        /// The ctl name that failed (e.g. `arena.4096.purge`).
        name: &'static str,
        /// The `errno` jemalloc returned.
        code: i32,
    },
}

/// Read a fresh stats snapshot, or `None` when jemalloc control is unavailable
/// (feature off, Windows, or the underlying `mallctl` reads failed).
pub fn stats_snapshot() -> Option<JemallocStats> {
    #[cfg(all(feature = "jemalloc", not(target_os = "windows")))]
    {
        imp::stats_snapshot()
    }
    #[cfg(not(all(feature = "jemalloc", not(target_os = "windows"))))]
    {
        None
    }
}

/// Force every arena to return its dirty and muzzy pages to the OS
/// (`arena.<MALLCTL_ARENAS_ALL>.purge`, i.e. an immediate MADV_DONTNEED sweep).
///
/// This is the manual equivalent of what a jemalloc `background_thread` would
/// do on decay — which is why it matters on macOS, where jemalloc builds have
/// no background thread and page decay otherwise only advances lazily on
/// alloc/free activity. Returns a no-op `Ok(())` when the feature is disabled.
pub fn purge_all_arenas() -> Result<(), JemallocError> {
    #[cfg(all(feature = "jemalloc", not(target_os = "windows")))]
    {
        imp::purge_all_arenas()
    }
    #[cfg(not(all(feature = "jemalloc", not(target_os = "windows"))))]
    {
        Ok(())
    }
}

#[cfg(test)]
#[path = "lib.test.rs"]
mod tests;
