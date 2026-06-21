//! Reserved shortcuts. Three categories:
//!
//! * [`NON_REBINDABLE`] — hardcoded in coco-rs; user attempts to rebind
//!   produce a validator **error**.
//! * [`TERMINAL_RESERVED`] — intercepted by the terminal/OS; user
//!   bindings get a validator **warning** since the keystroke may never
//!   reach the application.
//! * [`MACOS_RESERVED`] — additional macOS-only shortcuts the OS
//!   intercepts (Cmd+C/V/X, Cmd+Q, etc.).

use crate::validator::Severity;

/// One reserved shortcut entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservedShortcut {
    /// Chord string in canonical form. Compared via
    /// [`normalize_key_for_comparison`].
    pub key: &'static str,
    /// Reason surfaced to the user.
    pub reason: &'static str,
    /// Whether this is a hard error or just a warning.
    pub severity: Severity,
}

/// Shortcuts that cannot be rebound — they are hardcoded in coco-rs.
pub const NON_REBINDABLE: &[ReservedShortcut] = &[
    ReservedShortcut {
        key: "ctrl+c",
        reason: "Cannot be rebound — used for interrupt/exit (hardcoded)",
        severity: Severity::Error,
    },
    ReservedShortcut {
        key: "ctrl+d",
        reason: "Cannot be rebound — used for exit (hardcoded)",
        severity: Severity::Error,
    },
    ReservedShortcut {
        key: "ctrl+m",
        reason: "Cannot be rebound — identical to Enter in terminals (both send CR)",
        severity: Severity::Error,
    },
];

/// Terminal/OS-reserved shortcuts that may not reach the application.
///
/// Note: `ctrl+s` (XOFF) and `ctrl+q` (XON) are deliberately **not** here
/// because most modern terminals disable flow control by default and we use
/// `ctrl+s` for the stash feature.
pub const TERMINAL_RESERVED: &[ReservedShortcut] = &[
    ReservedShortcut {
        key: "ctrl+z",
        reason: "Unix process suspend (SIGTSTP)",
        severity: Severity::Warning,
    },
    ReservedShortcut {
        key: "ctrl+\\",
        reason: "Terminal quit signal (SIGQUIT)",
        severity: Severity::Error,
    },
];

/// macOS-only OS-intercepted shortcuts.
pub const MACOS_RESERVED: &[ReservedShortcut] = &[
    ReservedShortcut {
        key: "cmd+c",
        reason: "macOS system copy",
        severity: Severity::Error,
    },
    ReservedShortcut {
        key: "cmd+v",
        reason: "macOS system paste",
        severity: Severity::Error,
    },
    ReservedShortcut {
        key: "cmd+x",
        reason: "macOS system cut",
        severity: Severity::Error,
    },
    ReservedShortcut {
        key: "cmd+q",
        reason: "macOS quit application",
        severity: Severity::Error,
    },
    ReservedShortcut {
        key: "cmd+w",
        reason: "macOS close window/tab",
        severity: Severity::Error,
    },
    ReservedShortcut {
        key: "cmd+tab",
        reason: "macOS app switcher",
        severity: Severity::Error,
    },
    ReservedShortcut {
        key: "cmd+space",
        reason: "macOS Spotlight",
        severity: Severity::Error,
    },
];

/// Return all reserved shortcuts for the current platform — non-rebindable
/// first (highest priority), terminal-reserved next, macOS additions on
/// macOS hosts.
pub fn get_reserved_shortcuts() -> Vec<ReservedShortcut> {
    let mut reserved: Vec<ReservedShortcut> = NON_REBINDABLE
        .iter()
        .chain(TERMINAL_RESERVED.iter())
        .cloned()
        .collect();
    if cfg!(target_os = "macos") {
        reserved.extend(MACOS_RESERVED.iter().cloned());
    }
    reserved
}

/// Normalize a chord string for equality comparison: lowercase, sorted
/// modifiers, alias-collapsed (`option`/`opt` → `alt`, `command`/`cmd`
/// → `cmd`, `control` → `ctrl`).
///
/// Whitespace separates chord steps (each step normalized
/// independently); `+` separates a step's modifiers and base key.
pub fn normalize_key_for_comparison(key: &str) -> String {
    key.split_whitespace()
        .map(normalize_step)
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalize_step(step: &str) -> String {
    let mut modifiers: Vec<&'static str> = Vec::new();
    let mut main_key = String::new();

    for part in step.split('+') {
        let lower = part.trim().to_ascii_lowercase();
        match lower.as_str() {
            "ctrl" | "control" => modifiers.push("ctrl"),
            "alt" | "opt" | "option" => modifiers.push("alt"),
            "shift" => modifiers.push("shift"),
            "meta" => modifiers.push("meta"),
            "cmd" | "command" => modifiers.push("cmd"),
            "super" | "win" => modifiers.push("super"),
            other => main_key = other.to_string(),
        }
    }

    modifiers.sort_unstable();
    modifiers.dedup();
    let mut parts: Vec<String> = modifiers.into_iter().map(String::from).collect();
    parts.push(main_key);
    parts.join("+")
}

/// Look up whether a chord string is reserved on the current platform.
///
/// Returns the matching [`ReservedShortcut`] or `None` if the chord is
/// safe to bind.
pub fn lookup_reserved(chord: &str) -> Option<ReservedShortcut> {
    let canonical = normalize_key_for_comparison(chord);
    get_reserved_shortcuts()
        .into_iter()
        .find(|r| normalize_key_for_comparison(r.key) == canonical)
}

#[cfg(test)]
#[path = "reserved.test.rs"]
mod tests;
