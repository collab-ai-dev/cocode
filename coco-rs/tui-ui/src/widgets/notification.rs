//! Terminal notification backends — OSC escape sequences for 5 terminals.
//!
//! Terminal identity comes from [`crate::terminal_detect`]; this module only
//! maps an identity to the OSC dialect that terminal speaks. All writes are
//! best-effort; failures degrade silently to no notification.

use std::io::Write;

use crate::terminal_detect::Multiplexer;
use crate::terminal_detect::TerminalName;
use crate::terminal_detect::terminal_info;

/// Terminal-specific notification delivery method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationBackend {
    /// iTerm2 proprietary OSC 9;1 sequence.
    ITerm2,
    /// Plain OSC 9 (`ESC ] 9 ; body BEL`) — the widely-cloned simple form.
    Osc9,
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
    /// Auto-detect the backend from the terminal's identity.
    pub fn detect() -> Self {
        Self::for_terminal(terminal_info().name)
    }

    /// Map a terminal identity onto the OSC dialect it understands.
    ///
    /// Terminals with no notification protocol coco can rely on stay
    /// [`Self::Disabled`] rather than falling back to BEL: an unexpected bell
    /// is more disruptive than a missing notification.
    pub fn for_terminal(name: TerminalName) -> Self {
        match name {
            // WezTerm implements iTerm2's OSC 9;1.
            TerminalName::Iterm2 | TerminalName::WezTerm => Self::ITerm2,
            // Warp supports the plain OSC 9 form, not the 9;1 variant.
            TerminalName::Warp => Self::Osc9,
            TerminalName::Kitty => Self::Kitty,
            TerminalName::Ghostty => Self::Ghostty,
            TerminalName::AppleTerminal => Self::TerminalBell,
            TerminalName::Alacritty
            | TerminalName::GnomeTerminal
            | TerminalName::Hyper
            | TerminalName::Konsole
            | TerminalName::VsCode
            | TerminalName::Vte
            | TerminalName::WindowsTerminal
            | TerminalName::Dumb
            | TerminalName::Unknown => Self::Disabled,
        }
    }

    /// Emit the escape sequence(s) for this backend to `writer`.
    /// The TS code wraps OSC sequences for tmux/screen via DCS passthrough
    /// (`\x1bPtmux;\x1b...\x1b\\`). We detect the multiplexer via `$TMUX` /
    /// `$STY` and apply the same wrap here so users running inside tmux or
    /// GNU screen still get notifications forwarded to the outer terminal.
    pub fn send(self, writer: &mut impl Write, title: &str, message: &str) -> std::io::Result<()> {
        match self {
            Self::ITerm2 => write!(writer, "{}", wrap(&iterm2_osc(title, message))),
            Self::Osc9 => write!(writer, "{}", wrap(&osc9(title, message))),
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

/// Plain OSC 9 notification, BEL-terminated (Warp and other simple clones).
fn osc9(title: &str, message: &str) -> String {
    let display = if title.is_empty() {
        message.to_string()
    } else {
        format!("{title}: {message}")
    };
    format!("\x1b]9;{display}\x07")
}

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
    wrap_for(seq, terminal_info().multiplexer())
}

fn wrap_for(seq: &str, multiplexer: Option<Multiplexer>) -> String {
    match multiplexer {
        // tmux passthrough: ESC P tmux; ESC <payload with ESC doubled> ESC \
        Some(Multiplexer::Tmux) => {
            let escaped = seq.replace('\x1b', "\x1b\x1b");
            format!("\x1bPtmux;\x1b{escaped}\x1b\\")
        }
        // GNU screen DCS: ESC P <payload> ESC \
        Some(Multiplexer::Screen) => format!("\x1bP{seq}\x1b\\"),
        // Zellij has no documented passthrough; emit the sequence unwrapped
        // and let it be swallowed rather than leaking DCS bytes to the screen.
        Some(Multiplexer::Zellij) | None => seq.to_string(),
    }
}

#[cfg(test)]
#[path = "notification.test.rs"]
mod tests;
