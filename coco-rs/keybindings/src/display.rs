//! Keystroke and chord display formatting. Two flavours:
//!
//! * Canonical: lowercase, modifier order `ctrl+alt+shift+meta+cmd`,
//!   readable key names. Used for hashing / equality / config files.
//! * Display: same shape but with platform-appropriate modifier names
//!   (`opt` on macOS, `alt` elsewhere; `cmd` on macOS, `super`
//!   elsewhere) and arrow glyphs (`↑↓←→`) instead of words.

use crate::parser::KeyChord;
use crate::parser::KeyCombo;

/// Display platforms we distinguish for modifier-name rendering.
/// WSL and unknown both render as Linux-style modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayPlatform {
    Macos,
    Windows,
    Linux,
}

impl DisplayPlatform {
    /// Detect from the host OS at runtime. Useful for the TUI's status
    /// bar; for testing pass an explicit platform.
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Macos
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Linux
        }
    }
}

/// Canonical string for a single combo. Uses fixed modifier order
/// (`ctrl+alt+shift+meta+cmd`) so two equivalent combos always render
/// to the same string.
pub fn keystroke_to_string(combo: &KeyCombo) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if combo.ctrl {
        parts.push("ctrl");
    }
    if combo.alt {
        parts.push("alt");
    }
    if combo.shift {
        parts.push("shift");
    }
    if combo.meta {
        parts.push("meta");
    }
    if combo.super_key {
        parts.push("cmd");
    }
    let key = key_to_display_name(&combo.key);
    let mut out = parts.join("+");
    if !out.is_empty() {
        out.push('+');
    }
    out.push_str(&key);
    out
}

/// Canonical multi-combo chord string (combos joined by space).
pub fn chord_to_string(chord: &KeyChord) -> String {
    chord
        .0
        .iter()
        .map(keystroke_to_string)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Platform-aware single-combo display string. Use in status bars,
/// help menus, and prompt placeholders.
///
/// Differences from [`keystroke_to_string`]:
/// * Alt/meta are equivalent in terminals — render as `opt` on macOS,
///   `alt` elsewhere.
/// * Super (cmd/win) renders as `cmd` on macOS, `super` elsewhere.
/// * The base key uses readable / glyph names (`↑↓←→`, `Esc`, `Space`).
pub fn keystroke_to_display_string(combo: &KeyCombo, platform: DisplayPlatform) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if combo.ctrl {
        parts.push("ctrl");
    }
    // Alt/meta collapse for terminal display — both render as `opt` on
    // macOS, `alt` elsewhere.
    if combo.alt || combo.meta {
        parts.push(if matches!(platform, DisplayPlatform::Macos) {
            "opt"
        } else {
            "alt"
        });
    }
    if combo.shift {
        parts.push("shift");
    }
    // Distinct platform rendering for super (cmd/win): `cmd` on macOS,
    // `super` elsewhere.
    if combo.super_key {
        parts.push(if matches!(platform, DisplayPlatform::Macos) {
            "cmd"
        } else {
            "super"
        });
    }
    let key = key_to_platform_display_name(&combo.key);
    let mut out = parts.join("+");
    if !out.is_empty() {
        out.push('+');
    }
    out.push_str(&key);
    out
}

/// Platform-aware chord display string.
pub fn chord_to_display_string(chord: &KeyChord, platform: DisplayPlatform) -> String {
    chord
        .0
        .iter()
        .map(|c| keystroke_to_display_string(c, platform))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Map an internal key name to a human-readable display name.
/// Arrow keys render as glyphs; `escape` → `Esc`; other named keys get
/// TitleCase or remain as-is.
fn key_to_display_name(key: &str) -> String {
    match key {
        "escape" => "Esc".into(),
        "space" | " " => "Space".into(),
        "tab" => "tab".into(),
        "enter" => "Enter".into(),
        "backspace" => "Backspace".into(),
        "delete" => "Delete".into(),
        "up" => "↑".into(),
        "down" => "↓".into(),
        "left" => "←".into(),
        "right" => "→".into(),
        "pageup" => "PageUp".into(),
        "pagedown" => "PageDown".into(),
        "home" => "Home".into(),
        "end" => "End".into(),
        other => other.into(),
    }
}

fn key_to_platform_display_name(key: &str) -> String {
    if let Some(rest) = key.strip_prefix('f')
        && !rest.is_empty()
        && rest.chars().all(|c| c.is_ascii_digit())
    {
        return format!("F{rest}");
    }
    key_to_display_name(key)
}

#[cfg(test)]
#[path = "display.test.rs"]
mod tests;
