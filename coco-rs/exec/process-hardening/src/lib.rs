//! Pre-main process hardening: ptrace disable, core dump prevention,
//! and dangerous environment variable sanitization.
//!
//! Designed to be called very early (e.g. via `#[ctor::ctor]`) before
//! any user-controlled data is processed.
//!
//! # Safety exception
//!
//! This crate uses `unsafe` for libc FFI calls (`prctl`, `ptrace`,
//! `setrlimit`) and `std::env::remove_var` (not thread-safe). These
//! are the minimal unsafe boundary required for process hardening and
//! cannot be expressed in safe Rust. All calls happen pre-main before
//! any threads are spawned.

#[cfg(unix)]
use std::ffi::OsString;

#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;

/// Perform platform-specific process hardening.
///
/// - **Linux**: `PR_SET_DUMPABLE=0` (prevents ptrace), `RLIMIT_CORE=0`,
///   removes `LD_*` environment variables.
/// - **macOS**: `PT_DENY_ATTACH` (prevents debugger), `RLIMIT_CORE=0`,
///   removes `DYLD_*` environment variables.
/// - **BSD**: `RLIMIT_CORE=0`, removes `LD_*` environment variables.
pub fn pre_main_hardening() {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    pre_main_hardening_linux();

    #[cfg(target_os = "macos")]
    pre_main_hardening_macos();

    #[cfg(any(target_os = "freebsd", target_os = "openbsd"))]
    pre_main_hardening_bsd();
}

#[cfg(any(target_os = "linux", target_os = "android"))]
const PRCTL_FAILED_EXIT_CODE: i32 = 5;

#[cfg(target_os = "macos")]
const PTRACE_DENY_ATTACH_FAILED_EXIT_CODE: i32 = 6;

#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
))]
const SET_RLIMIT_CORE_FAILED_EXIT_CODE: i32 = 7;

#[cfg(any(target_os = "linux", target_os = "android"))]
fn pre_main_hardening_linux() {
    // Disable ptrace attach / mark process non-dumpable.
    let ret_code = unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) };
    if ret_code != 0 {
        eprintln!(
            "ERROR: prctl(PR_SET_DUMPABLE, 0) failed: {}",
            std::io::Error::last_os_error()
        );
        std::process::exit(PRCTL_FAILED_EXIT_CODE);
    }

    set_core_file_size_limit_to_zero();
    remove_env_keys_with_prefix(b"LD_");
}

#[cfg(any(target_os = "freebsd", target_os = "openbsd"))]
fn pre_main_hardening_bsd() {
    set_core_file_size_limit_to_zero();
    remove_env_keys_with_prefix(b"LD_");
}

#[cfg(target_os = "macos")]
fn pre_main_hardening_macos() {
    // Prevent debuggers from attaching to this process.
    let ret_code = unsafe { libc::ptrace(libc::PT_DENY_ATTACH, 0, std::ptr::null_mut(), 0) };
    if ret_code == -1 {
        eprintln!(
            "ERROR: ptrace(PT_DENY_ATTACH) failed: {}",
            std::io::Error::last_os_error()
        );
        std::process::exit(PTRACE_DENY_ATTACH_FAILED_EXIT_CODE);
    }

    set_core_file_size_limit_to_zero();
    remove_env_keys_with_prefix(b"DYLD_");
}

#[cfg(unix)]
fn set_core_file_size_limit_to_zero() {
    let rlim = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };

    let ret_code = unsafe { libc::setrlimit(libc::RLIMIT_CORE, &rlim) };
    if ret_code != 0 {
        eprintln!(
            "ERROR: setrlimit(RLIMIT_CORE) failed: {}",
            std::io::Error::last_os_error()
        );
        std::process::exit(SET_RLIMIT_CORE_FAILED_EXIT_CODE);
    }
}

/// Remove all environment variables matching a prefix (e.g. `LD_*`, `DYLD_*`).
///
/// SAFETY: `std::env::remove_var` is not thread-safe. This must only be
/// called before any threads are spawned.
#[cfg(unix)]
fn remove_env_keys_with_prefix(prefix: &[u8]) {
    let keys: Vec<OsString> = std::env::vars_os()
        .filter_map(|(key, _)| {
            key.as_os_str()
                .as_bytes()
                .starts_with(prefix)
                .then_some(key)
        })
        .collect();

    for key in keys {
        unsafe {
            std::env::remove_var(key);
        }
    }
}

#[cfg(test)]
#[path = "lib.test.rs"]
mod tests;
