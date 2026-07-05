//! Real jemalloc backend — the only place in the workspace that touches the
//! `mallctl` FFI. Compiled solely when `feature = "jemalloc"` on a non-Windows
//! target, where `tikv-jemalloc-{ctl,sys}` are linked.

use std::ptr;

use libc::c_char;
use tikv_jemalloc_ctl::epoch;
use tikv_jemalloc_ctl::stats;

use crate::JemallocError;
use crate::JemallocStats;

/// `MALLCTL_ARENAS_ALL` from `jemalloc_macros.h`: the pseudo-arena index that
/// makes `arena.<i>.purge` act on every arena at once. Baked in as a literal
/// because the ctl name has to be a compile-time NUL-terminated byte string.
const PURGE_CTL: &[u8] = b"arena.4096.purge\0";

pub(crate) fn stats_snapshot() -> Option<JemallocStats> {
    // jemalloc caches `stats.*`; they only refresh when the epoch advances.
    // Advance first so the four reads below reflect the current heap. All of
    // these go through `tikv-jemalloc-ctl`'s safe typed wrappers.
    epoch::advance().ok()?;
    Some(JemallocStats {
        allocated: stats::allocated::read().ok()? as u64,
        active: stats::active::read().ok()? as u64,
        resident: stats::resident::read().ok()? as u64,
        retained: stats::retained::read().ok()? as u64,
    })
}

pub(crate) fn purge_all_arenas() -> Result<(), JemallocError> {
    // The purge ctl is write-only with a NULL value; jemalloc rejects any
    // non-NULL `newp` with EINVAL, so it cannot go through the typed ctl write
    // helpers (which always pass a value pointer). The documented form is
    // `mallctl("arena." STRINGIFY(MALLCTL_ARENAS_ALL) ".purge", NULL,NULL,NULL,0)`.
    //
    // SAFETY: `PURGE_CTL` is a 'static NUL-terminated ctl name; every in/out
    // buffer is NULL with zero length, which is exactly the "run the action"
    // mallctl form this ctl expects. jemalloc's `mallctl` is thread-safe.
    let rc = unsafe {
        tikv_jemalloc_sys::mallctl(
            PURGE_CTL.as_ptr() as *const c_char,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            0,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(JemallocError::Mallctl {
            name: "arena.4096.purge",
            code: rc,
        })
    }
}
