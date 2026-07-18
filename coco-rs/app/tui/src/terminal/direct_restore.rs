//! Platform-specific restore path independent of the asynchronous stdout writer.

#[cfg(any(unix, windows))]
use std::fs::OpenOptions;
#[cfg(any(unix, windows))]
use std::io::IoSlice;
#[cfg(any(unix, windows))]
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::sync::atomic::Ordering;

use crossterm::terminal::disable_raw_mode;

#[cfg(any(unix, windows))]
#[derive(Debug, Clone, Copy)]
pub(super) struct DirectRestoreOptions {
    pub(super) leave_alt_screen: bool,
    pub(super) trailing_newline: bool,
}

/// Emit a best-effort restore in one non-blocking write. The caller supplies
/// an independent terminal sink; in particular, this function never acquires
/// Rust's process-global stdout lock.
#[cfg(any(unix, windows))]
pub(super) fn write_direct_restore_tail(
    writer: &mut impl Write,
    options: DirectRestoreOptions,
) -> bool {
    let alt_screen = if options.leave_alt_screen {
        b"\x1b[?1049l".as_slice()
    } else {
        b"".as_slice()
    };
    let trailing_newline = if options.trailing_newline {
        b"\r\n".as_slice()
    } else {
        b"".as_slice()
    };
    let parts = [
        IoSlice::new(coco_tui_ui::engine::restore_seq::RESTORE_PREFIX),
        IoSlice::new(alt_screen),
        IoSlice::new(coco_tui_ui::engine::restore_seq::RESTORE_SUFFIX),
        IoSlice::new(trailing_newline),
    ];
    let expected = parts.iter().map(|part| part.len()).sum::<usize>();
    writer.write_vectored(&parts).ok() == Some(expected)
}

#[cfg(unix)]
fn restore_via_direct_tty(options: DirectRestoreOptions) -> bool {
    let Ok(mut tty) = OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open("/dev/tty")
    else {
        return false;
    };
    write_direct_restore_tail(&mut tty, options)
}

#[cfg(windows)]
fn restore_via_direct_tty(options: DirectRestoreOptions) -> bool {
    // Windows console writes have no `O_NONBLOCK` equivalent. Dispatch the
    // best-effort restore to a detached worker so a wedged ConPTY cannot make
    // the panic hook or teardown deadline wait past its bound. Return false:
    // the caller must keep the crash handler armed because completion is not
    // synchronously confirmed.
    let _ = std::thread::Builder::new()
        .name("tui-direct-restore".to_string())
        .spawn(move || {
            if let Ok(mut console) = OpenOptions::new().write(true).open("CONOUT$") {
                let _ = write_direct_restore_tail(&mut console, options);
            }
        });
    false
}

/// Panic cleanup must remain independent of the asynchronous frame writer.
/// That writer may be blocked while holding Rust's global stdout lock, so the
/// hook writes through a separate terminal handle (`/dev/tty` opened
/// non-blocking on Unix; a detached `CONOUT$` best-effort worker on Windows).
pub(super) fn after_panic() {
    let leave_alt_screen = super::MODAL_ALT_SCREEN_ACTIVE.swap(false, Ordering::AcqRel);
    #[cfg(any(unix, windows))]
    {
        let restored = restore_via_direct_tty(DirectRestoreOptions {
            leave_alt_screen,
            trailing_newline: false,
        });
        let raw_mode_disabled = disable_raw_mode().is_ok();
        if restored && raw_mode_disabled {
            coco_utils_crash_handler::disarm_tui_restore();
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = leave_alt_screen;
        let _ = super::restore_terminal();
    }
}

/// Last-resort restore when the background stdout writer misses its teardown
/// deadline. The normal queued tail remains authoritative and repairs state
/// again if the blocked writer later resumes.
pub(super) fn after_writer_timeout(leave_alt_screen: bool) {
    #[cfg(any(unix, windows))]
    {
        let restored = restore_via_direct_tty(DirectRestoreOptions {
            leave_alt_screen,
            trailing_newline: true,
        });
        let raw_mode_disabled = disable_raw_mode().is_ok();
        if restored && raw_mode_disabled {
            coco_utils_crash_handler::disarm_tui_restore();
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = leave_alt_screen;
        let _ = super::restore_terminal();
    }
}
