//! Terminal dark/light detection for the `auto` theme setting.
//!
//! Mirrors claude-code's `utils/systemTheme.ts`: detection is based on the
//! terminal's actual *background* color (an OSC 11 query, performed by the
//! shell), not the OS appearance — a dark terminal on a light-mode OS still
//! resolves to dark. The parsed result is cached process-wide so `auto`
//! resolves without re-querying.
//!
//! This module is the pure, domain-free half: the `SystemTheme` value, the OSC
//! response parser (`theme_from_osc_color`), the `$COLORFGBG` heuristic
//! (`detect_from_colorfgbg`), and the cache. The actual terminal I/O (writing
//! the OSC 11 query and reading the reply with a timeout) lives in the shell
//! (`coco-tui`), which calls [`set_cached_system_theme`] once it parses a reply.

use std::sync::OnceLock;
use std::sync::RwLock;

/// Detected terminal background brightness.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SystemTheme {
    Dark,
    Light,
}

fn cache() -> &'static RwLock<Option<SystemTheme>> {
    static CACHE: OnceLock<RwLock<Option<SystemTheme>>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(None))
}

/// The OSC 11-detected background theme, or `None` until a probe parses a
/// reply. Callers fall back to `$COLORFGBG` / dark. Mirrors `getSystemThemeName`
/// minus the env seed (the shell owns env reads, keeping this crate pure).
pub fn cached_system_theme() -> Option<SystemTheme> {
    cache().read().ok().and_then(|guard| *guard)
}

/// Update the cached background theme — called once an OSC 11 response parses
/// (or by a live background-change watcher). Mirrors `setCachedSystemTheme`.
pub fn set_cached_system_theme(theme: SystemTheme) {
    if let Ok(mut guard) = cache().write() {
        *guard = Some(theme);
    }
}

/// Parse an OSC 10/11 color response into a theme via BT.709 relative
/// luminance (midpoint split: `> 0.5` is light). Accepts `rgb:R/G/B` (each
/// component 1–4 hex digits; a trailing `rgba:` alpha is ignored) and
/// `#RRGGBB` / `#RRRRGGGGBBBB`. Mirrors `themeFromOscColor`.
pub fn theme_from_osc_color(data: &str) -> Option<SystemTheme> {
    let (r, g, b) = parse_osc_rgb(data)?;
    let luminance = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    Some(if luminance > 0.5 {
        SystemTheme::Light
    } else {
        SystemTheme::Dark
    })
}

/// `$COLORFGBG` synchronous heuristic (rxvt convention): the trailing background
/// index 0–6 or 8 is dark; 7 and 9–15 are light. `None` if unset / unparseable.
/// Mirrors `detectFromColorFgBg`.
pub fn detect_from_colorfgbg(value: &str) -> Option<SystemTheme> {
    let bg = value.split(';').next_back()?.trim();
    let index: i32 = bg.parse().ok()?;
    if !(0..=15).contains(&index) {
        return None;
    }
    Some(if (0..=6).contains(&index) || index == 8 {
        SystemTheme::Dark
    } else {
        SystemTheme::Light
    })
}

/// Parse the RGB triple out of an OSC color payload into `[0,1]` components.
fn parse_osc_rgb(data: &str) -> Option<(f64, f64, f64)> {
    let data = data.trim();
    // `rgb:RRRR/GGGG/BBBB` (xterm/iTerm2/Terminal.app/kitty/…). `rgba:` adds a
    // trailing alpha component, which we ignore.
    if let Some(rest) = data
        .strip_prefix("rgba:")
        .or_else(|| data.strip_prefix("rgb:"))
    {
        let mut parts = rest.split('/');
        let r = hex_component(parts.next()?)?;
        let g = hex_component(parts.next()?)?;
        let b = hex_component(parts.next()?)?;
        return Some((r, g, b));
    }
    // `#RRGGBB` or `#RRRRGGGGBBBB` — split into three equal hex runs.
    if let Some(hex) = data.strip_prefix('#')
        && !hex.is_empty()
        && hex.len() % 3 == 0
    {
        let n = hex.len() / 3;
        return Some((
            hex_component(&hex[..n])?,
            hex_component(&hex[n..2 * n])?,
            hex_component(&hex[2 * n..])?,
        ));
    }
    None
}

/// Normalize a 1–4 digit hex string to a `[0,1]` intensity.
fn hex_component(hex: &str) -> Option<f64> {
    if hex.is_empty() || hex.len() > 4 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let value = i64::from_str_radix(hex, 16).ok()?;
    // 16^len - 1, i.e. the max value representable in `len` hex digits.
    let max = (1i64 << (4 * hex.len())) - 1;
    Some(value as f64 / max as f64)
}

#[cfg(test)]
#[path = "system_theme.test.rs"]
mod tests;
