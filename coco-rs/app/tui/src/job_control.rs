//! Process suspend / resume (Ctrl+Z / fg) on Unix.
//!
//! In raw mode the terminal no longer translates Ctrl+Z into a
//! `SIGTSTP` for us — the keystroke just lands as an ordinary
//! `KeyEvent`. To get the vim / less-style "Ctrl+Z drops you to the
//! shell, `fg` brings me back" behaviour we intercept the key in
//! [`crate::app::App::convert_crossterm_event`] and call
//! [`Tui::trigger_suspend`](crate::terminal::Tui::trigger_suspend),
//! which delegates here.
//!
//! Flow (Unix):
//!
//! 1. [`crate::terminal::leave_tui_modes`] — turn off raw mode, leave any
//!    state alt-screen, and disable bracketed paste / focus change reporting.
//! 2. Show the cursor on a fresh normal-buffer row so the shell sees its
//!    prompt.
//! 3. Record the pending resume flag for the next draw.
//! 4. `libc::kill(0, SIGTSTP)` — kernel stops the process group. The
//!    call returns synchronously, but the next user-mode instruction
//!    doesn't execute until SIGCONT arrives (default `fg` behaviour,
//!    or external `kill -CONT $pid`).
//! 5. [`crate::terminal::enter_tui_modes`] — re-arm raw mode etc.
//! 6. [`SuspendContext::take_resume_pending`] is consumed inside
//!    [`crate::terminal::Tui::draw`] on the next frame, which clears the
//!    native surface and forces a full repaint. If a large state is still
//!    active, `Tui` re-enters alt-screen through normal state placement.
//!
//! Windows: no `SIGTSTP`; all entry points become no-ops.
//!
//! Design notes live in `docs/coco-rs/ui/rendering-hardening-and-rollback.md`.

#[cfg(unix)]
use std::io;
#[cfg(unix)]
use std::sync::Arc;
#[cfg(unix)]
use std::sync::Mutex;

#[cfg(unix)]
use crossterm::cursor::MoveToNextLine;
#[cfg(unix)]
use crossterm::cursor::Show;
#[cfg(unix)]
use crossterm::execute;

// ───────────────────────── Unix implementation ─────────────────────────

/// State machine for suspend / resume. Cheap to construct; the only
/// inner mutable bit is the "a `Ctrl+Z → fg` cycle just happened" flag
/// guarded by a Mutex (Mutex chosen over RefCell because `Tui` is
/// `Send`-able and we want to keep that property without re-checking).
#[cfg(unix)]
#[derive(Default)]
pub struct SuspendContext {
    resume_pending: Arc<Mutex<bool>>,
}

#[cfg(unix)]
impl SuspendContext {
    pub fn new() -> Self {
        Self::default()
    }

    /// Drop into shell, blocking until SIGCONT brings us back.
    ///
    /// Errors come from the terminal-mode IO (raw mode toggle, escape
    /// sequence write) or the `libc::kill` syscall. The pending resume
    /// flag is set even if `kill` succeeds — see step 3 of the module
    /// docs.
    pub fn suspend(&self) -> io::Result<()> {
        let mut stdout = io::stdout();

        // 1. Disable our private terminal modes so the shell can read
        //    user input normally (otherwise keystrokes wouldn't echo).
        crate::terminal::leave_tui_modes()?;

        // 2. Show the cursor on a fresh row in the normal buffer. This
        //    also undoes any `Hide` queued by the post-draw cursor pin
        //    path before yielding to the shell.
        if let Err(err) = execute!(stdout, MoveToNextLine(1), Show) {
            return restore_after_suspend_error(&mut stdout, err);
        }

        // 3. Record what the next draw needs to do.
        if let Ok(mut guard) = self.resume_pending.lock() {
            *guard = true;
        }

        // 4. Deliver SIGTSTP to our process group. `kill(0, sig)` hits
        //    every process in the caller's pgroup, including the
        //    caller, so the kernel stops us before returning to user
        //    mode. Control resumes here only after SIGCONT.
        let rc = unsafe { libc::kill(0, libc::SIGTSTP) };
        if rc != 0 {
            return restore_after_suspend_error(&mut stdout, io::Error::last_os_error());
        }

        // 5. We're back. Re-arm TUI modes; surface clear happens on the next
        //    `Tui::draw` via `take_resume_pending`.
        crate::terminal::enter_tui_modes(&mut stdout)?;
        Ok(())
    }

    /// Consume the pending resume flag. Called from
    /// [`crate::terminal::Tui::draw`] at the top of each frame so the
    /// surface repaint happens before render reads from the terminal.
    pub fn take_resume_pending(&self) -> bool {
        self.resume_pending
            .lock()
            .map(|mut guard| std::mem::take(&mut *guard))
            .unwrap_or(false)
    }
}

#[cfg(unix)]
fn restore_after_suspend_error(stdout: &mut io::Stdout, err: io::Error) -> io::Result<()> {
    match crate::terminal::enter_tui_modes(stdout) {
        Ok(()) => Err(err),
        Err(restore_err) => Err(io::Error::new(
            restore_err.kind(),
            format!(
                "failed to restore TUI modes after suspend error ({err}); restore error: {restore_err}"
            ),
        )),
    }
}

// ─────────────────────── Non-Unix no-op stubs ──────────────────────────

#[cfg(not(unix))]
#[derive(Default)]
pub struct SuspendContext;

#[cfg(not(unix))]
impl SuspendContext {
    pub fn new() -> Self {
        Self
    }

    pub fn suspend(&self) -> std::io::Result<()> {
        Ok(())
    }

    pub fn take_resume_pending(&self) -> bool {
        false
    }
}

#[cfg(test)]
#[path = "job_control.test.rs"]
mod tests;
