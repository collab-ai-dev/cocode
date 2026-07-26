//! One-shot startup terminal probe: background color, synchronized output, and
//! keyboard-enhancement support in a single round-trip.
//!
//! Every one of these facts is a question only the terminal can answer, and
//! each answer costs the same thing: coco must own terminal input for the reply
//! window, and whatever the user typed in that window is consumed with it. Three
//! separate probes therefore cost three input-stealing windows and three
//! timeouts stacked end to end. This module asks all three questions in one
//! write, reads one buffer, and closes the window once.
//!
//! The queries are ordered so a single fence terminates the read: OSC 11
//! (background), DECRQM mode 2026 (synchronized output), `CSI ? u` (kitty
//! keyboard protocol), then DA1. Every terminal answers DA1, and it answers it
//! *last*, so its arrival proves the earlier answers either came or never will —
//! the full timeout is only ever paid by a terminal that answers nothing.
//!
//! Strictly best-effort: on non-tty, no reply, or any parse miss the caller
//! keeps its existing defaults. Runs at most once per process. Unix only (the
//! queries need raw-mode tty I/O); a no-op stub elsewhere.

use std::time::Duration;

use coco_tui_ui::system_theme::SystemTheme;

/// What one startup probe learned. Every field is `None` when the terminal did
/// not answer that particular query — absence is never evidence of a `false`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StartupProbe {
    /// Dark/light classified from the OSC 11 background reply.
    pub(crate) background: Option<SystemTheme>,
    /// Whether DECRQM recognized mode 2026 (synchronized output).
    pub(crate) synchronized_update: Option<bool>,
    /// Whether the terminal answered the kitty keyboard-protocol query.
    pub(crate) keyboard_enhancement: Option<bool>,
}

/// Probe the terminal once per process and publish each answer to its consumer
/// cache. Subsequent calls are no-ops. `timeout` bounds the whole exchange, not
/// each query.
pub(crate) fn probe_terminal_once(timeout: Duration) {
    use std::sync::OnceLock;
    static PROBED: OnceLock<()> = OnceLock::new();
    if PROBED.set(()).is_err() {
        return;
    }
    let probe = run_probe(timeout);
    if let Some(theme) = probe.background {
        coco_tui_ui::system_theme::set_cached_system_theme(theme);
    }
    if let Some(supported) = probe.synchronized_update {
        coco_tui_ui::engine::compatibility::set_synchronized_update_supported(supported);
    }
    if let Some(supported) = probe.keyboard_enhancement {
        coco_tui_ui::engine::compatibility::set_keyboard_enhancement_supported(supported);
    }
    tracing::debug!(
        target: "coco_tui::terminal_probe",
        background = ?probe.background,
        synchronized_update = ?probe.synchronized_update,
        keyboard_enhancement = ?probe.keyboard_enhancement,
        "startup terminal probe finished",
    );
}

#[cfg(not(unix))]
fn run_probe(_timeout: Duration) -> StartupProbe {
    StartupProbe::default()
}

/// Queries sent as one write, in reply order. DA1 is last so its reply fences
/// the read.
#[cfg(unix)]
const QUERIES: &[u8] = b"\x1b]11;?\x07\x1b[?2026$p\x1b[?u\x1b[c";

#[cfg(unix)]
fn run_probe(timeout: Duration) -> StartupProbe {
    use std::io::Write;

    use std::io::IsTerminal;

    // Only probe a real interactive terminal — never a pipe / redirect / SDK.
    //
    // No `/dev/tty` fallback, deliberately: it would let the probe read from the
    // controlling terminal in exactly the cases where `setup_terminal` is about
    // to refuse to start (it requires tty stdin *and* stdout), so the only thing
    // the fallback could add is swallowing a keystroke on the way to an error.
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return StartupProbe::default();
    }

    // The replies carry no newline and would echo in cooked mode, so the
    // exchange needs raw mode. The guard restores the prior mode on every exit
    // path (`setup_terminal` re-enters raw mode later, idempotently).
    let Ok(_raw) = RawModeGuard::enable() else {
        return StartupProbe::default();
    };
    let mut stdout = std::io::stdout();
    if stdout.write_all(QUERIES).is_err() || stdout.flush().is_err() {
        return StartupProbe::default();
    }
    let Some(reply) = read_until_da1(timeout) else {
        return StartupProbe::default();
    };
    parse_probe_reply(&reply)
}

/// Split one reply buffer into the three answers. Written against a plain byte
/// slice so the parsing is testable without a terminal.
#[cfg_attr(not(unix), allow(dead_code))]
fn parse_probe_reply(buf: &[u8]) -> StartupProbe {
    StartupProbe {
        background: extract_osc11_payload(buf)
            .as_deref()
            .and_then(coco_tui_ui::system_theme::theme_from_osc_color),
        synchronized_update: Some(parse_decrpm_2026(buf).unwrap_or(false)),
        keyboard_enhancement: Some(has_kitty_keyboard_reply(buf)),
    }
}

/// Extract the payload after the `ESC ] 11 ;` introducer up to its terminator
/// (e.g. `rgb:1e1e/1e1e/1e1e`). Returns `None` if no OSC 11 reply is present.
#[cfg_attr(not(unix), allow(dead_code))]
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

/// Parse a DECRPM reply for mode 2026: `ESC [ ? 2026 ; Ps $ y`. `Ps` is 0 when
/// the mode is unrecognized and 1/2/3/4 when set/reset/perm-set/perm-reset —
/// any non-zero value means the terminal supports synchronized output.
#[cfg_attr(not(unix), allow(dead_code))]
fn parse_decrpm_2026(buf: &[u8]) -> Option<bool> {
    let start = find_subslice(buf, b"\x1b[?2026;")? + b"\x1b[?2026;".len();
    let rest = &buf[start..];
    let end = rest.iter().position(|b| !b.is_ascii_digit())?;
    let ps: u16 = std::str::from_utf8(&rest[..end]).ok()?.parse().ok()?;
    Some(ps != 0)
}

/// Whether the terminal answered the kitty keyboard query with `CSI ? <flags> u`.
///
/// Terminals without the protocol ignore an unknown private CSI entirely, so a
/// reply is the whole signal; the advertised flags are not consulted, because
/// coco pushes its own flag set rather than adopting the current one.
#[cfg_attr(not(unix), allow(dead_code))]
fn has_kitty_keyboard_reply(buf: &[u8]) -> bool {
    private_csi_replies(buf).any(|(_, final_byte)| final_byte == b'u')
}

/// Whether `buf` contains the DA1 fence — a private CSI reply ending in `c`.
///
/// Scanning for a bare `c` byte would be wrong here: OSC 11 answers in hex, so
/// a background of `rgb:cccc/cccc/cccc` carries one.
#[cfg_attr(not(unix), allow(dead_code))]
fn da1_reply_complete(buf: &[u8]) -> bool {
    private_csi_replies(buf).any(|(_, final_byte)| final_byte == b'c')
}

/// Iterate the private CSI replies (`ESC [ ? … <final>`) in `buf`, yielding the
/// parameter bytes and the final byte of each. Incomplete trailing sequences are
/// skipped — a reply that is still arriving is not yet an answer.
#[cfg_attr(not(unix), allow(dead_code))]
fn private_csi_replies(buf: &[u8]) -> impl Iterator<Item = (&[u8], u8)> {
    const INTRODUCER: &[u8] = b"\x1b[?";
    let mut cursor = 0usize;
    std::iter::from_fn(move || {
        let start = find_subslice(&buf[cursor..], INTRODUCER)? + cursor + INTRODUCER.len();
        // CSI final bytes are 0x40..=0x7E; parameter and intermediate bytes
        // (digits, `;`, `$`) all sort below that range. A sequence still in
        // flight has no final byte yet, and is not an answer.
        let offset = buf[start..]
            .iter()
            .position(|&b| (0x40..=0x7e).contains(&b))?;
        cursor = start + offset + 1;
        Some((&buf[start..start + offset], buf[start + offset]))
    })
}

/// First index of `needle` within `haystack`, if present.
#[cfg_attr(not(unix), allow(dead_code))]
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Read until the DA1 fence arrives or `timeout` elapses. Uses `poll` so a
/// non-responding terminal can never hang.
///
/// A timeout still returns whatever arrived: a terminal that answered OSC 11 but
/// never sends DA1 has still told us its background color, and discarding that
/// would waste the one window where asking was possible.
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
    let mut buf: Vec<u8> = Vec::with_capacity(128);
    let mut chunk = [0u8; 128];

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return (!buf.is_empty()).then_some(buf);
        }
        let ts = Timespec {
            tv_sec: remaining.as_secs() as i64,
            tv_nsec: i64::from(remaining.subsec_nanos()),
        };
        let mut fds = [PollFd::new(&stdin, PollFlags::IN)];
        match poll(&mut fds, Some(&ts)) {
            Ok(0) => return (!buf.is_empty()).then_some(buf),
            Ok(_) => {}
            Err(_) => return (!buf.is_empty()).then_some(buf),
        }
        if !fds[0].revents().contains(PollFlags::IN) {
            return (!buf.is_empty()).then_some(buf);
        }
        match stdin.lock().read(&mut chunk) {
            Ok(0) => return (!buf.is_empty()).then_some(buf),
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if da1_reply_complete(&buf) || buf.len() > 512 {
                    return Some(buf);
                }
            }
            Err(_) => return (!buf.is_empty()).then_some(buf),
        }
    }
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

#[cfg(test)]
#[path = "terminal_probe.test.rs"]
mod tests;
