//! Terminal notification backends — OSC escape sequences for 5 terminals.
//!
//! Parity with TS `src/services/notifier.ts` + `src/ink/useTerminalNotification.ts`.
//! Detects the terminal from `$TERM_PROGRAM` / `$LC_TERMINAL` / `$TERM` and
//! emits the appropriate OSC sequence. All writes are best-effort; failures
//! degrade silently to no notification.

use std::io::Write;

/// Terminal-specific notification delivery method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationBackend {
    /// iTerm2 proprietary OSC 9;1 sequence.
    ITerm2,
    /// Kitty OSC 99 notification (title + body + focus action).
    Kitty,
    /// Ghostty OSC 777 notify protocol.
    Ghostty,
    /// Plain BEL (works on Apple Terminal with the right profile, tmux, etc.).
    TerminalBell,
    /// No notification channel available for this terminal.
    Disabled,
}

impl NotificationBackend {
    /// Auto-detect the backend from the environment.
    ///
    /// Matches TS `sendAuto()` behaviour: uses `$TERM_PROGRAM` first, then
    /// falls back to `$LC_TERMINAL` / `$TERM` for terminals that don't set
    /// `TERM_PROGRAM` (Kitty without its wrapper, Ghostty via SSH, etc.).
    pub fn detect() -> Self {
        let term_program = std::env::var("TERM_PROGRAM").unwrap_or_default();
        let lc_terminal = std::env::var("LC_TERMINAL").unwrap_or_default();
        let term = std::env::var("TERM").unwrap_or_default();

        match term_program.as_str() {
            "iTerm.app" => Self::ITerm2,
            "ghostty" => Self::Ghostty,
            "WezTerm" => Self::ITerm2, // wezterm supports OSC 9;1
            "Apple_Terminal" => Self::TerminalBell,
            _ if lc_terminal.eq_ignore_ascii_case("iterm2") => Self::ITerm2,
            _ if term.starts_with("xterm-kitty") || term_program == "kitty" => Self::Kitty,
            _ => Self::Disabled,
        }
    }

    /// Emit the escape sequence(s) for this backend to `writer`.
    ///
    /// The TS code wraps OSC sequences for tmux/screen via DCS passthrough
    /// (`\x1bPtmux;\x1b...\x1b\\`). We detect the multiplexer via `$TMUX` /
    /// `$STY` and apply the same wrap here so users running inside tmux or
    /// GNU screen still get notifications forwarded to the outer terminal.
    pub fn send(self, writer: &mut impl Write, title: &str, message: &str) -> std::io::Result<()> {
        match self {
            Self::ITerm2 => write!(writer, "{}", wrap(&iterm2_osc(title, message))),
            Self::Kitty => {
                let id = kitty_id();
                write!(writer, "{}", wrap(&kitty_title_osc(id, title)))?;
                write!(writer, "{}", wrap(&kitty_body_osc(id, message)))?;
                write!(writer, "{}", wrap(&kitty_commit_osc(id)))
            }
            Self::Ghostty => write!(writer, "{}", wrap(&ghostty_osc(title, message))),
            // BEL is emitted raw (no DCS wrap) so tmux's own bell-action
            // handler fires and propagates the visual cue.
            Self::TerminalBell => write!(writer, "\x07"),
            Self::Disabled => Ok(()),
        }
    }
}

/// `notify()` — detect backend and emit the sequence to stdout. Helper for
/// callers that don't need to hold the backend value (typical: one-shot
/// turn-completion notifications).
pub fn notify(title: &str, message: &str) {
    let mut out = std::io::stdout();
    let _ = NotificationBackend::detect().send(&mut out, title, message);
    let _ = out.flush();
}

// ── OSC sequence builders ──

/// iTerm2 OSC 9;1 notification (`OSC.ITERM2 == "9;1;"`).
/// Payload format: `\n\n{display}`.
fn iterm2_osc(title: &str, message: &str) -> String {
    let display = if title.is_empty() {
        message.to_string()
    } else {
        format!("{title}:\n{message}")
    };
    // OSC 9;1;<payload>ST
    format!("\x1b]9;1;\n\n{display}\x1b\\")
}

/// Kitty OSC 99 title frame (d=0 opens the notification, p=title marks body).
fn kitty_title_osc(id: u32, title: &str) -> String {
    format!("\x1b]99;i={id}:d=0:p=title;{title}\x1b\\")
}

/// Kitty OSC 99 body frame.
fn kitty_body_osc(id: u32, body: &str) -> String {
    format!("\x1b]99;i={id}:p=body;{body}\x1b\\")
}

/// Kitty OSC 99 commit frame (d=1 closes, a=focus raises the window).
fn kitty_commit_osc(id: u32) -> String {
    format!("\x1b]99;i={id}:d=1:a=focus;\x1b\\")
}

/// Ghostty OSC 777 notify protocol.
fn ghostty_osc(title: &str, message: &str) -> String {
    format!("\x1b]777;notify;{title};{message}\x1b\\")
}

/// Pick a Kitty notification id. TS uses `Math.floor(Math.random() * 10000)`.
/// We use nanoseconds modulo 10_000 to avoid pulling in a rand dependency.
fn kitty_id() -> u32 {
    use std::time::SystemTime;
    use std::time::UNIX_EPOCH;
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() % 10_000)
        .unwrap_or(0)
}

/// Wrap `seq` for tmux/screen DCS passthrough if either multiplexer is
/// active. Outside a multiplexer, returns the sequence unchanged.
fn wrap(seq: &str) -> String {
    if std::env::var_os("TMUX").is_some() {
        // tmux passthrough: ESC P tmux; ESC <payload with ESC doubled> ESC \
        let escaped = seq.replace('\x1b', "\x1b\x1b");
        return format!("\x1bPtmux;\x1b{escaped}\x1b\\");
    }
    if std::env::var_os("STY").is_some() {
        // GNU screen DCS: ESC P <payload> ESC \
        return format!("\x1bP{seq}\x1b\\");
    }
    seq.to_string()
}

#[cfg(test)]
#[path = "notification.test.rs"]
mod tests;
