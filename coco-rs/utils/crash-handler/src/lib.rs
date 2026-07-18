//! Two-tier, async-signal-safe terminal restore on fatal faults.
//!
//! coco paints every frame inside a synchronized-update (`?2026h`/`?2026l`)
//! window, so a hard fault (SIGSEGV / SIGBUS / … raised inside a C dependency
//! such as jemalloc, ring, or whisper) *mid-frame* freezes the visible
//! terminal — it reads as a total lockup and the user must run `reset` blind.
//! The unwind panic hook never runs for these signals.
//!
//! This crate installs async-signal-safe handlers that, on a fatal fault:
//! 1. restore the saved (cooked) termios so the shell is usable again, and
//! 2. if armed, `write(2)` a caller-supplied restore byte sequence (coco's
//!    `RESTORE_SEQ`, whose first bytes close the sync-update window),
//!
//! then re-raise the signal with the default disposition so the process still
//! dies with the correct signal / core-dump behavior.
//!
//! Two tiers, matching the plan:
//! - [`install_terminal_restore_only`] — install the handlers + snapshot
//!   termios at the very top of `main`, before the TUI starts. Even a crash
//!   during early startup then leaves a cooked terminal.
//! - [`arm_tui_restore`] / [`disarm_tui_restore`] — toggled from
//!   `enter_tui_modes` / `leave_tui_modes` so the handler also emits the raw
//!   `RESTORE_SEQ` while (and only while) TUI modes are active.
//!
//! ## Concurrency model
//!
//! The armed state is a single [`AtomicBool`] and the restore sequence is a
//! set-once [`OnceLock`]. Because one atomic load decides whether to write, and
//! the sequence (a `&'static [u8]`, ptr + len) is immutable once set, the signal
//! handler can never observe a torn pointer/length pair. `arm_tui_restore` is
//! single-sequence by design (coco always arms the one `RESTORE_SEQ` const); a
//! second, *different* sequence is a misuse and trips a debug assertion rather
//! than silently racing.
//!
//! Installing SIGSEGV/SIGBUS handlers replaces std's own stack-overflow
//! reporter, so a stack overflow no longer prints std's "has overflowed its
//! stack" message — it restores the terminal and re-raises. Non-overflow faults
//! on worker threads still run on std's per-thread alternate signal stack.
//!
//! # Safety exception
//!
//! Like `coco-process-hardening`, this crate wraps the minimal `libc` FFI
//! (`sigaction`, `sigaltstack`, `tcgetattr`, `tcsetattr`, `write`, `raise`)
//! that cannot be expressed in safe Rust. The handler body touches only
//! async-signal-safe primitives — an atomic load, a `OnceLock` read-after-init,
//! `tcsetattr`, `write`, `raise` — with no allocation, no locks, and no
//! formatting.

use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

/// Whether a TUI restore write is currently armed. A single atomic is the whole
/// arm/disarm decision, so the handler never reads an inconsistent state.
static ARMED: AtomicBool = AtomicBool::new(false);

/// The restore byte sequence (coco's `RESTORE_SEQ`), set once on the first arm.
/// A `&'static [u8]` is immutable ptr + len, so the handler reads it atomically.
static SEQUENCE: OnceLock<&'static [u8]> = OnceLock::new();

/// Arm the fault handler to also write `seq` to the terminal on a fatal fault.
///
/// Call from `enter_tui_modes` with coco's `RESTORE_SEQ`. The sequence is
/// recorded on the first call and reused thereafter; arming a *different*
/// sequence is unsupported (single fixed restore blob) and trips a debug
/// assertion.
pub fn arm_tui_restore(seq: &'static [u8]) {
    let stored = SEQUENCE.get_or_init(|| seq);
    debug_assert!(
        std::ptr::eq(*stored, seq),
        "crash-handler armed with a second, different restore sequence"
    );
    ARMED.store(true, Ordering::Release);
}

/// Disarm the TUI restore (call from `leave_tui_modes`): the handler falls back
/// to termios-only restore.
pub fn disarm_tui_restore() {
    ARMED.store(false, Ordering::Release);
}

/// Whether a TUI restore sequence is currently armed.
pub fn is_armed() -> bool {
    ARMED.load(Ordering::Acquire)
}

/// Install the fatal-fault handlers and snapshot the terminal's cooked termios.
///
/// Idempotent; best-effort (a failed `tcgetattr` simply skips termios restore).
/// Call once at the top of `main`, before entering TUI modes.
#[cfg(unix)]
pub fn install_terminal_restore_only() {
    imp::install();
}

/// No async-signal-safe terminal restore on non-unix targets; the unwind panic
/// hook remains the only cleanup path.
#[cfg(not(unix))]
pub fn install_terminal_restore_only() {}

#[cfg(unix)]
mod imp {
    use super::ARMED;
    use super::Ordering;
    use super::SEQUENCE;
    use std::os::raw::c_int;
    use std::os::raw::c_void;
    use std::sync::OnceLock;
    use std::sync::atomic::AtomicBool;

    static INSTALLED: AtomicBool = AtomicBool::new(false);
    static SAVED_TERMIOS: OnceLock<SavedTermios> = OnceLock::new();

    /// A snapshot of the terminal's cooked termios. `libc::termios` is a POD C
    /// struct; a read-only copy is safe to share across threads and read from a
    /// signal handler.
    struct SavedTermios(libc::termios);
    // SAFETY: POD, only ever read after being set once.
    unsafe impl Send for SavedTermios {}
    unsafe impl Sync for SavedTermios {}

    /// Alternate signal-stack size so a stack-overflow SIGSEGV can still run the
    /// handler. Clears `MINSIGSTKSZ` on both Linux and macOS and comfortably
    /// holds the (allocation-free) handler frame.
    const ALT_STACK_SIZE: usize = 64 * 1024;

    const FATAL_SIGNALS: [c_int; 5] = [
        libc::SIGSEGV,
        libc::SIGBUS,
        libc::SIGILL,
        libc::SIGFPE,
        libc::SIGABRT,
    ];

    pub(super) fn install() {
        if INSTALLED.swap(true, Ordering::SeqCst) {
            return;
        }
        // SAFETY: called once, early in `main`, before other threads exist. Each
        // FFI call is a direct libc wrapper with valid, initialized arguments.
        unsafe {
            let mut termios: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(libc::STDIN_FILENO, &mut termios) == 0 {
                let _ = SAVED_TERMIOS.set(SavedTermios(termios));
            }
            install_alt_stack();

            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = handle_fault as *const () as usize;
            action.sa_flags = libc::SA_ONSTACK | libc::SA_NODEFER | libc::SA_RESETHAND;
            libc::sigemptyset(&mut action.sa_mask);
            for &sig in &FATAL_SIGNALS {
                libc::sigaction(sig, &action, std::ptr::null_mut());
            }
        }
    }

    /// Register a leaked alternate signal stack (lives for the process lifetime).
    ///
    /// # Safety
    /// Must be called from `install` (single-threaded, pre-TUI).
    unsafe fn install_alt_stack() {
        let stack = vec![0u8; ALT_STACK_SIZE].into_boxed_slice();
        let stack = Box::leak(stack);
        let ss = libc::stack_t {
            ss_sp: stack.as_mut_ptr().cast::<c_void>(),
            ss_flags: 0,
            ss_size: ALT_STACK_SIZE,
        };
        // SAFETY: `ss` describes a live, process-lifetime stack region.
        unsafe {
            libc::sigaltstack(&ss, std::ptr::null_mut());
        }
    }

    /// Async-signal-safe fault handler: restore termios, write the armed restore
    /// bytes, then re-raise. `SA_RESETHAND` already restored the default
    /// disposition, so `raise` delivers the real fatal action (e.g. core dump).
    extern "C" fn handle_fault(sig: c_int) {
        if let Some(saved) = SAVED_TERMIOS.get() {
            // SAFETY: tcsetattr is async-signal-safe; `saved` is a read-only POD.
            unsafe {
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &saved.0);
            }
        }
        // One atomic decides the write; the sequence is immutable once set, so
        // there is no torn ptr/len read.
        if ARMED.load(Ordering::Acquire)
            && let Some(seq) = SEQUENCE.get()
        {
            // SAFETY: write is async-signal-safe; `seq` is a live 'static slice.
            unsafe {
                let mut written = 0usize;
                while written < seq.len() {
                    let count = libc::write(
                        libc::STDOUT_FILENO,
                        seq.as_ptr().add(written).cast::<c_void>(),
                        seq.len() - written,
                    );
                    if count <= 0 {
                        break;
                    }
                    written += count as usize;
                }
            }
        }
        // SAFETY: raise is async-signal-safe.
        unsafe {
            libc::raise(sig);
        }
    }
}

#[cfg(test)]
#[path = "lib.test.rs"]
mod tests;
