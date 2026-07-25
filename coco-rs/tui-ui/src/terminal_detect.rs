//! Typed terminal identity detection.
//!
//! Terminal identity is a closed set, so it is modelled as [`TerminalName`] /
//! [`Multiplexer`] rather than as raw `TERM_PROGRAM` strings compared at each
//! call site. Capability *heuristics* (color depth, OSC 8 support, native
//! scrollback) stay with their consumers — this module answers only "which
//! terminal is this", which those heuristics then key on.
//!
//! Every detector is written against an injectable `get_env` seam so the
//! heuristics are unit-testable without mutating process env.

use std::sync::OnceLock;

/// A terminal emulator coco can identify from the environment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum TerminalName {
    /// Apple Terminal (Terminal.app).
    AppleTerminal,
    /// Alacritty.
    Alacritty,
    /// GNOME Terminal.
    GnomeTerminal,
    /// Ghostty.
    Ghostty,
    /// Hyper.
    Hyper,
    /// iTerm2.
    Iterm2,
    /// kitty.
    Kitty,
    /// KDE Konsole.
    Konsole,
    /// Visual Studio Code's integrated terminal.
    VsCode,
    /// Any other VTE-backed terminal.
    Vte,
    /// Warp.
    Warp,
    /// WezTerm.
    WezTerm,
    /// Windows Terminal.
    WindowsTerminal,
    /// `TERM=dumb` — no cursor addressing, no color.
    Dumb,
    /// Unidentified, or identification hidden by a multiplexer.
    #[default]
    Unknown,
}

/// A terminal multiplexer between coco and the real emulator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Multiplexer {
    /// tmux.
    Tmux,
    /// GNU screen.
    Screen,
    /// Zellij.
    Zellij,
}

/// The detected terminal identity.
///
/// Multiplexers nest (tmux inside Zellij is ordinary), and the two questions
/// callers ask have different answers when they do: a DCS passthrough must
/// speak the *innermost* multiplexer's dialect, while a repaint or scrollback
/// decision is governed by *any* multiplexer in the chain. Hence
/// [`Self::multiplexer`] and [`Self::is_inside`] rather than one field.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TerminalInfo {
    /// The emulator, or [`TerminalName::Unknown`] when a multiplexer hides it.
    pub name: TerminalName,
    tmux: bool,
    screen: bool,
    zellij: bool,
}

impl TerminalInfo {
    /// The innermost multiplexer — the one whose passthrough dialect applies.
    pub fn multiplexer(&self) -> Option<Multiplexer> {
        if self.tmux {
            Some(Multiplexer::Tmux)
        } else if self.screen {
            Some(Multiplexer::Screen)
        } else if self.zellij {
            Some(Multiplexer::Zellij)
        } else {
            None
        }
    }

    /// Whether `multiplexer` is anywhere in the chain, inner or outer.
    pub fn is_inside(&self, multiplexer: Multiplexer) -> bool {
        match multiplexer {
            Multiplexer::Tmux => self.tmux,
            Multiplexer::Screen => self.screen,
            Multiplexer::Zellij => self.zellij,
        }
    }

    /// Whether any multiplexer sits between coco and the emulator.
    pub fn in_multiplexer(&self) -> bool {
        self.tmux || self.screen || self.zellij
    }
}

/// Read a non-empty environment variable.
fn env_lookup(name: &str) -> Option<String> {
    std::env::var_os(name).and_then(|value| {
        let text = value.to_string_lossy();
        (!text.is_empty()).then(|| text.into_owned())
    })
}

/// The detected terminal identity, cached for the process lifetime.
///
/// The environment a terminal reports its identity through does not change
/// after startup, so this is resolved once.
pub fn terminal_info() -> TerminalInfo {
    static INFO: OnceLock<TerminalInfo> = OnceLock::new();
    *INFO.get_or_init(|| terminal_info_with(env_lookup))
}

/// Detect terminal identity from an injectable environment.
///
/// An empty value is never a signal: terminals set these variables to identify
/// themselves, and an exported-but-empty variable means the opposite.
pub fn terminal_info_with<F>(get_env: F) -> TerminalInfo
where
    F: Fn(&str) -> Option<String>,
{
    let get_env = |name: &str| get_env(name).filter(|value| !value.is_empty());
    let any = |names: &[&str]| names.iter().any(|name| get_env(name).is_some());
    TerminalInfo {
        name: detect_name(&get_env),
        tmux: any(&["TMUX", "TMUX_PANE"]),
        screen: any(&["STY"]),
        zellij: any(&["ZELLIJ", "ZELLIJ_SESSION_NAME", "ZELLIJ_VERSION"]),
    }
}

fn detect_name<F>(get_env: &F) -> TerminalName
where
    F: Fn(&str) -> Option<String>,
{
    let term = get_env("TERM").unwrap_or_default().to_ascii_lowercase();
    if term == "dumb" {
        return TerminalName::Dumb;
    }

    // 1. `TERM_PROGRAM` is the canonical identity advertisement. Under a
    //    multiplexer it names the multiplexer, so those values fall through to
    //    the emulator-specific markers below, which multiplexers pass along.
    let term_program = get_env("TERM_PROGRAM")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let named = match term_program.as_str() {
        "iterm.app" => Some(TerminalName::Iterm2),
        "apple_terminal" => Some(TerminalName::AppleTerminal),
        "ghostty" => Some(TerminalName::Ghostty),
        "wezterm" => Some(TerminalName::WezTerm),
        "vscode" => Some(TerminalName::VsCode),
        "warpterminal" | "warp" => Some(TerminalName::Warp),
        "hyper" => Some(TerminalName::Hyper),
        "kitty" => Some(TerminalName::Kitty),
        "alacritty" => Some(TerminalName::Alacritty),
        _ => None,
    };
    if let Some(name) = named {
        return name;
    }

    // 2. iTerm2 forwards its identity over SSH through `LC_TERMINAL`.
    if get_env("LC_TERMINAL").is_some_and(|value| value.eq_ignore_ascii_case("iterm2")) {
        return TerminalName::Iterm2;
    }

    // 3. Emulator-specific markers. These survive SSH and multiplexers more
    //    often than `TERM_PROGRAM` does.
    for (var, name) in [
        ("WT_SESSION", TerminalName::WindowsTerminal),
        ("KITTY_WINDOW_ID", TerminalName::Kitty),
        ("WEZTERM_EXECUTABLE", TerminalName::WezTerm),
        ("WEZTERM_PANE", TerminalName::WezTerm),
        ("GHOSTTY_RESOURCES_DIR", TerminalName::Ghostty),
        ("GHOSTTY_BIN_DIR", TerminalName::Ghostty),
        ("ALACRITTY_WINDOW_ID", TerminalName::Alacritty),
        ("ALACRITTY_SOCKET", TerminalName::Alacritty),
        ("KONSOLE_VERSION", TerminalName::Konsole),
        ("GNOME_TERMINAL_SCREEN", TerminalName::GnomeTerminal),
        ("GNOME_TERMINAL_SERVICE", TerminalName::GnomeTerminal),
    ] {
        if get_env(var).is_some() {
            return name;
        }
    }

    // 4. `TERM` names a handful of emulators outright.
    for (needle, name) in [
        ("xterm-kitty", TerminalName::Kitty),
        ("xterm-ghostty", TerminalName::Ghostty),
        ("alacritty", TerminalName::Alacritty),
        ("wezterm", TerminalName::WezTerm),
        ("konsole", TerminalName::Konsole),
    ] {
        if term.contains(needle) {
            return name;
        }
    }

    // 5. Any remaining VTE backend identifies itself by version only.
    if get_env("VTE_VERSION").is_some() {
        return TerminalName::Vte;
    }

    TerminalName::Unknown
}

#[cfg(test)]
#[path = "terminal_detect.test.rs"]
mod tests;
