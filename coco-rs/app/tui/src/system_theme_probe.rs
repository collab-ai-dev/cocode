//! Best-effort OSC 11 terminal-background probe for the `auto` theme.
//!
//! Writes the OSC 11 "query background color" sequence, reads the reply with a
//! bounded timeout, parses it via [`coco_tui_ui::system_theme`], and caches the
//! resulting [`SystemTheme`](coco_tui_ui::system_theme::SystemTheme) so `auto`
//! resolves to dark/light from the *actual* terminal background (mirrors
//! claude-code's `systemThemeWatcher`).
//!
//! Strictly best-effort and bounded: on any non-response, parse miss, non-tty,
//! or error it silently does nothing and the caller falls back to `$COLORFGBG`
//! / dark. Runs at most once per process. Unix only (the query needs raw-mode
//! tty I/O); a no-op stub elsewhere.

use std::time::Duration;

/// Probe the terminal background once per process (best-effort). Subsequent
/// calls are no-ops. `timeout` bounds the wait for the terminal's reply.
#[cfg(unix)]
pub(crate) fn probe_terminal_background_once(timeout: Duration) {
    use std::sync::OnceLock;
    static PROBED: OnceLock<()> = OnceLock::new();
    if PROBED.set(()).is_err() {
        return;
    }
    if let Some(theme) = query_osc11_background(timeout) {
        coco_tui_ui::system_theme::set_cached_system_theme(theme);
    }
}

#[cfg(not(unix))]
pub(crate) fn probe_terminal_background_once(_timeout: Duration) {}

#[cfg(unix)]
fn query_osc11_background(timeout: Duration) -> Option<coco_tui_ui::system_theme::SystemTheme> {
    use std::io::IsTerminal;
    use std::io::Write;

    // Only probe a real interactive terminal — never a pipe / redirect / SDK.
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return None;
    }

    // The OSC 11 reply has no newline and would be echoed in cooked mode, so the
    // query needs raw mode. The guard restores the prior mode on every exit path
    // (setup_terminal re-enters raw mode later, idempotently).
    let _raw = RawModeGuard::enable().ok()?;

    let mut stdout = std::io::stdout();
    // BEL-terminated form is the most widely understood across terminals.
    stdout.write_all(b"\x1b]11;?\x07").ok()?;
    stdout.flush().ok()?;

    let reply = read_osc_reply(timeout)?;
    let payload = extract_osc11_payload(&reply)?;
    coco_tui_ui::system_theme::theme_from_osc_color(&payload)
}

/// Read bytes from stdin until an OSC reply terminator (BEL or ST) arrives or
/// `timeout` elapses. Uses `poll` so a non-responding terminal can never hang.
#[cfg(unix)]
fn read_osc_reply(timeout: Duration) -> Option<Vec<u8>> {
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
                if osc_reply_complete(&buf) || buf.len() > 256 {
                    return Some(buf);
                }
            }
            Err(_) => return None,
        }
    }
}

/// Whether `buf` already contains an OSC string terminator (BEL or `ESC \`).
#[cfg(unix)]
fn osc_reply_complete(buf: &[u8]) -> bool {
    buf.contains(&0x07) || find_subslice(buf, b"\x1b\\").is_some()
}

/// Extract the payload after the `ESC ] 11 ;` introducer up to its terminator
/// (e.g. `rgb:1e1e/1e1e/1e1e`). Returns `None` if no OSC 11 reply is present.
#[cfg(unix)]
fn extract_osc11_payload(buf: &[u8]) -> Option<String> {
    let start = find_subslice(buf, b"\x1b]11;")? + b"\x1b]11;".len();
    let rest = &buf[start..];
    let end = rest
        .iter()
        .position(|&b| b == 0x07)
        .or_else(|| find_subslice(rest, b"\x1b\\"))
        .unwrap_or(rest.len());
    Some(String::from_utf8_lossy(&rest[..end]).into_owned())
}

/// First index of `needle` within `haystack`, if present.
#[cfg(unix)]
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Raw-mode guard that is a no-op when raw mode is already on — see the
/// matching guard in `sync_update_probe`. If `setup_terminal` already armed raw
/// mode for the session, unconditionally disabling it on drop would drop the
/// whole session into cooked mode; only restore if THIS guard enabled raw.
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
#[path = "system_theme_probe.test.rs"]
mod tests;
