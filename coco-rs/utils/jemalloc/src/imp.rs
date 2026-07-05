//! Real jemalloc backend — the only place in the workspace that touches the
//! `mallctl` FFI. Compiled solely when `feature = "jemalloc"` on a non-Windows
//! target, where `tikv-jemalloc-{ctl,sys}` are linked.

use std::ffi::CString;
use std::mem;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::ptr;

use libc::c_char;
use libc::c_void;
use tikv_jemalloc_ctl::epoch;
use tikv_jemalloc_ctl::stats;

use crate::JemallocError;
use crate::JemallocStats;

/// `MALLCTL_ARENAS_ALL` from `jemalloc_macros.h`: the pseudo-arena index that
/// makes `arena.<i>.purge` act on every arena at once. Baked in as a literal
/// because the ctl name has to be a compile-time NUL-terminated byte string.
const PURGE_CTL: &[u8] = b"arena.4096.purge\0";
/// Startup-time profiling switch: true only when jemalloc was built with
/// `--enable-prof` AND its startup conf (baked-in `--with-malloc-conf`, or the
/// `MALLOC_CONF` env) contains `prof:true`. Read-only; fixed at init.
const OPT_PROF_CTL: &[u8] = b"opt.prof\0";
/// Runtime sampling gate; writable whenever `opt.prof` is on.
const PROF_ACTIVE_CTL: &[u8] = b"prof.active\0";
/// Write-a-filename action ctl: dumps the sampled live heap to that path.
const PROF_DUMP_CTL: &[u8] = b"prof.dump\0";

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

pub(crate) fn heap_profiling_available() -> bool {
    // `tikv-jemalloc-ctl` has no typed wrapper for the `prof.*`/`opt.prof`
    // ctls, so these three go through the same raw `mallctl` this module
    // already uses for the purge. jemalloc's `bool` is the C99 1-byte bool;
    // read/write it as `u8`.
    let mut value: u8 = 0;
    let mut len = mem::size_of::<u8>();
    // SAFETY: ctl name is 'static and NUL-terminated; oldp/oldlenp describe a
    // valid 1-byte buffer matching the ctl's bool type. A non-prof jemalloc
    // build returns ENOENT, which we fold into "unavailable".
    let rc = unsafe {
        tikv_jemalloc_sys::mallctl(
            OPT_PROF_CTL.as_ptr() as *const c_char,
            &mut value as *mut u8 as *mut c_void,
            &mut len,
            ptr::null_mut(),
            0,
        )
    };
    rc == 0 && value != 0
}

pub(crate) fn set_heap_profiling_active(active: bool) -> Result<(), JemallocError> {
    let mut value = u8::from(active);
    // SAFETY: ctl name is 'static and NUL-terminated; newp/newlen describe a
    // valid 1-byte buffer matching the ctl's bool type.
    let rc = unsafe {
        tikv_jemalloc_sys::mallctl(
            PROF_ACTIVE_CTL.as_ptr() as *const c_char,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut value as *mut u8 as *mut c_void,
            mem::size_of::<u8>(),
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(JemallocError::Mallctl {
            name: "prof.active",
            code: rc,
        })
    }
}

pub(crate) fn dump_heap_profile(path: &Path) -> Result<(), JemallocError> {
    let cpath =
        CString::new(path.as_os_str().as_bytes()).map_err(|_| JemallocError::InvalidDumpPath)?;
    let mut filename: *const c_char = cpath.as_ptr();
    // SAFETY: `prof.dump` takes a `const char *` by value — newp points at the
    // pointer itself for exactly pointer-size bytes. `cpath` outlives the call,
    // and jemalloc copies the string before returning.
    let rc = unsafe {
        tikv_jemalloc_sys::mallctl(
            PROF_DUMP_CTL.as_ptr() as *const c_char,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut filename as *mut *const c_char as *mut c_void,
            mem::size_of::<*const c_char>(),
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(JemallocError::Mallctl {
            name: "prof.dump",
            code: rc,
        })
    }
}
