//! Resume hint printed after the TUI exits.
//!
//! After alt-screen exit, write a dim two-line nudge to stdout so the user
//! sees how to pick the session back up:
//!
//! ```text
//! Resume this session with:
//! coco-cli --resume <session-id>
//! ```
//!
//! Only emit when there is a session id AND a transcript file on disk.
//! TTY / interactive checks are implicit —
//! `coco_tui::terminal::setup_terminal` refuses to start without a
//! TTY, so any caller reaching this point already has both.
//!
//! Ordering invariant: callers MUST drop the TUI App (which triggers
//! `coco_tui::terminal::Tui::drop` → leaves alt-screen + disables raw
//! mode) BEFORE invoking this. Otherwise the hint scrolls inside the
//! alt-screen and disappears when the terminal restores the main
//! buffer.

use std::io::Write;
use std::io::{self};
use std::path::Path;

use coco_session::TranscriptStore;

use crate::paths::project_paths;

/// ANSI `dim` (`SGR 2`) escape sequences.
const DIM_ON: &str = "\x1b[2m";
const DIM_OFF: &str = "\x1b[22m";

/// Render the dim-styled resume hint lines for `session_id`. Pure
/// function: the file-existence guard is decided by the caller. Split
/// out from [`print_resume_hint`] so the format is unit-testable
/// without touching the filesystem or the global stdout.
///
/// One outer SGR `dim` pair wraps the whole multi-line block.
fn render(session_id: &str) -> String {
    format!("{DIM_ON}\nResume this session with:\ncoco-cli --resume {session_id}\n{DIM_OFF}")
}

/// Print the "Resume this session with: coco-cli --resume <id>" hint.
///
/// No-op when `session_id` is `None` (the user quit before
/// `ServerNotification::SessionStarted` reached the TUI) or when the
/// transcript file is missing (the session had no work worth
/// resuming). Errors writing to stdout are swallowed — the process is
/// already on the way out and there is nothing useful to do with an
/// I/O failure here.
pub fn print_resume_hint(cwd: &Path, session_id: Option<&str>) {
    let Some(sid) = session_id else {
        return;
    };
    if sid.is_empty() {
        return;
    }
    let store = TranscriptStore::new(project_paths(cwd));
    if !store.exists(sid) {
        return;
    }
    let mut stdout = io::stdout().lock();
    let _ = stdout.write_all(render(sid).as_bytes());
    let _ = stdout.flush();
}

#[cfg(test)]
#[path = "resume_hint.test.rs"]
mod tests;
