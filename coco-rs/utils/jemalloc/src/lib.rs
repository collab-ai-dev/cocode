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
    /// A heap-profile dump path contained an interior NUL byte, so it cannot
    /// be passed to `prof.dump` as a C string.
    #[error("heap profile dump path contains an interior NUL byte")]
    InvalidDumpPath,
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

/// Render jemalloc's full `malloc_stats_print` report, or `None` when
/// jemalloc control is unavailable or the report cannot be captured.
///
/// This is intentionally separate from [`stats_snapshot`]: the report is
/// large and should only be requested at a diagnostic threshold crossing.
pub fn stats_print() -> Option<String> {
    #[cfg(all(feature = "jemalloc", not(target_os = "windows")))]
    {
        imp::stats_print()
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

/// Whether heap profiling can record samples in this process: requires the
/// jemalloc backend (see [`ENABLED`]) AND `prof:true` in jemalloc's startup
/// conf (`opt.prof` is fixed at init; profiling cannot be switched on later).
/// The workspace bakes it into libjemalloc via `JEMALLOC_SYS_WITH_MALLOC_CONF`
/// (.cargo/config.toml) rather than the `MALLOC_CONF` env, which would leak to
/// every child process.
pub fn heap_profiling_available() -> bool {
    #[cfg(all(feature = "jemalloc", not(target_os = "windows")))]
    {
        imp::heap_profiling_available()
    }
    #[cfg(not(all(feature = "jemalloc", not(target_os = "windows"))))]
    {
        false
    }
}

/// Toggle jemalloc's `prof.active` sampling gate at runtime. Only meaningful
/// when [`heap_profiling_available`] — writing the ctl is otherwise either an
/// error (non-prof build) or a no-op jemalloc ignores. No-op `Ok(())` when the
/// backend is disabled.
pub fn set_heap_profiling_active(active: bool) -> Result<(), JemallocError> {
    #[cfg(all(feature = "jemalloc", not(target_os = "windows")))]
    {
        imp::set_heap_profiling_active(active)
    }
    #[cfg(not(all(feature = "jemalloc", not(target_os = "windows"))))]
    {
        let _ = active;
        Ok(())
    }
}

/// Dump the sampled live heap (`prof.dump`) to `path` in jemalloc's `.heap`
/// format — analyze with `jeprof` or `jemalloc-pprof`. Captures only
/// allocations made while `prof.active` sampling was on. No-op `Ok(())` when
/// the backend is disabled; callers gate on [`heap_profiling_available`].
pub fn dump_heap_profile(path: &std::path::Path) -> Result<(), JemallocError> {
    #[cfg(all(feature = "jemalloc", not(target_os = "windows")))]
    {
        imp::dump_heap_profile(path)
    }
    #[cfg(not(all(feature = "jemalloc", not(target_os = "windows"))))]
    {
        let _ = path;
        Ok(())
    }
}

#[cfg(test)]
#[path = "lib.test.rs"]
mod tests;
