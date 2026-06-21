//! Best-effort DECRQM probe for synchronized output (DECSET mode 2026).
//!
//! Writes the DECRQM "request mode 2026" sequence followed by a Primary Device
//! Attributes (DA1) query as a fence, reads the reply with a bounded timeout,
//! and records whether the terminal recognizes mode 2026 via
//! [`coco_tui_ui::engine::compatibility::set_synchronized_update_supported`].
//! The native surface uses the result to fall back to a non-flickering
//! grow-only viewport on terminals that lack synchronized update.
//!
//! Strictly best-effort and bounded: on any non-response, parse miss, non-tty,
//! or error it does nothing and the renderer assumes support (BSU/ESU is
//! emitted regardless; it is simply ignored by terminals that lack mode 2026).
//! Runs at most once per process. Unix only; a no-op stub elsewhere.

use std::time::Duration;

/// Probe synchronized-update support once per process (best-effort). Subsequent
/// calls are no-ops. `timeout` bounds the wait for the terminal's reply.
#[cfg(unix)]
pub(crate) fn probe_synchronized_update_once(timeout: Duration) {
    use std::sync::OnceLock;
    static PROBED: OnceLock<()> = OnceLock::new();
    if PROBED.set(()).is_err() {
        return;
    }
    if let Some(supported) = query_synchronized_update(timeout) {
        coco_tui_ui::engine::compatibility::set_synchronized_update_supported(supported);
    }
}

#[cfg(not(unix))]
pub(crate) fn probe_synchronized_update_once(_timeout: Duration) {}

#[cfg(unix)]
fn query_synchronized_update(timeout: Duration) -> Option<bool> {
    use std::io::IsTerminal;
    use std::io::Write;

    // Only probe a real interactive terminal — never a pipe / redirect / SDK.
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return None;
    }

    // DECRQM/DA1 replies have no newline and would echo in cooked mode, so the
    // query needs raw mode. The guard restores the prior mode on every exit
    // path (setup_terminal re-enters raw mode later, idempotently).
    let _raw = RawModeGuard::enable().ok()?;

    let mut stdout = std::io::stdout();
    // DECRQM mode 2026, then DA1 (`ESC [ c`) as a fence: every terminal answers
    // DA1, so we wait only for the DA1 round-trip rather than the full timeout
    // even when mode 2026 is unrecognized (no DECRPM reply at all).
    stdout.write_all(b"\x1b[?2026$p\x1b[c").ok()?;
    stdout.flush().ok()?;

    let reply = read_until_da1(timeout)?;
    // DA1 arrived. A recognized mode replies `ESC [ ? 2026 ; Ps $ y` with
    // Ps != 0; its absence (DA1 only) means mode 2026 is unsupported.
    Some(parse_decrpm_2026(&reply).unwrap_or(false))
}

/// Read stdin until a DA1 reply (CSI … `c`) arrives or `timeout` elapses. Uses
/// `poll` so a non-responding terminal can never hang.
#[cfg(unix)]
fn read_until_da1(timeout: Duration) -> Option<Vec<u8>> {
    use std::io::Read;
    use std::time::Instant;

    use rustix::event::PollFd;
    use rustix::event::PollFlags;
    use rustix::event::Timespec;
    use rustix::event::poll;

    let stdin = std::io::stdin();
    let deadline = Instant::now() + timeout;
    let mut buf: Vec<u8> = Vec::with_capacity(64);
    let mut chunk = [0u8; 64];

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return None;
        }
        let ts = Timespec {
            tv_sec: remaining.as_secs() as i64,
            tv_nsec: i64::from(remaining.subsec_nanos()),
        };
        let mut fds = [PollFd::new(&stdin, PollFlags::IN)];
        match poll(&mut fds, Some(&ts)) {
            Ok(0) => return None, // timed out with no input
            Ok(_) => {}
            Err(_) => return None,
        }
        if !fds[0].revents().contains(PollFlags::IN) {
            return None;
        }
        match stdin.lock().read(&mut chunk) {
            Ok(0) => return None,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if da1_reply_complete(&buf) || buf.len() > 256 {
                    return Some(buf);
                }
            }
            Err(_) => return None,
        }
    }
}

/// Whether `buf` contains the DA1 reply terminator. DA1 is `ESC [ ? … c`; the
/// DECRPM 2026 reply ends in `y`, so a `c` is unambiguously the DA1 fence we
/// sent last (and which the terminal answers after the DECRPM reply, if any).
#[cfg(unix)]
fn da1_reply_complete(buf: &[u8]) -> bool {
    buf.contains(&b'c')
}

/// Parse a DECRPM reply for mode 2026: `ESC [ ? 2026 ; Ps $ y`. `Ps` is 0 when
/// the mode is unrecognized and 1/2/3/4 when set/reset/perm-set/perm-reset —
/// any non-zero value means the terminal supports synchronized output.
#[cfg(unix)]
fn parse_decrpm_2026(buf: &[u8]) -> Option<bool> {
    let start = find_subslice(buf, b"\x1b[?2026;")? + b"\x1b[?2026;".len();
    let rest = &buf[start..];
    let end = rest.iter().position(|b| !b.is_ascii_digit())?;
    let ps: u16 = std::str::from_utf8(&rest[..end]).ok()?.parse().ok()?;
    Some(ps != 0)
}

/// First index of `needle` within `haystack`, if present.
#[cfg(unix)]
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Raw-mode guard that is a no-op when raw mode is already on.
///
/// `setup_terminal` arms raw mode for the whole session BEFORE this probe runs
/// (`App::new` precedes `install_theme`). Unconditionally disabling raw mode on
/// drop would restore cooked termios and leave the entire session in cooked
/// mode — focus reports echo as `^[[O`/`^[[I`, the placeholder/typed text fight,
/// and `ISIG` turns Ctrl+C into SIGINT. So only restore cooked mode if THIS
/// guard is the one that enabled raw mode.
#[cfg(unix)]
struct RawModeGuard {
    enabled_here: bool,
}

#[cfg(unix)]
impl RawModeGuard {
    fn enable() -> std::io::Result<Self> {
        let already_raw = crossterm::terminal::is_raw_mode_enabled()?;
        if !already_raw {
            crossterm::terminal::enable_raw_mode()?;
        }
        Ok(Self {
            enabled_here: !already_raw,
        })
    }
}

#[cfg(unix)]
impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if self.enabled_here {
            let _ = crossterm::terminal::disable_raw_mode();
        }
    }
}

#[cfg(all(test, unix))]
#[path = "sync_update_probe.test.rs"]
mod tests;
