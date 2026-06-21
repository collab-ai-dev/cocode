//! Terminal color-capability detection and truecolor→xterm-256 downsampling.
//!
//! Absorbed from jcode's `jcode-tui-style` color handling: terminals without
//! 24-bit color render `Color::Rgb` poorly (or not at all), so we detect the
//! capability once and quantize RGB to the nearest xterm-256 palette index when
//! truecolor is unavailable. Quantization picks the closer of the 6×6×6 color
//! cube and the 24-step grayscale ramp under a green-weighted distance.

use std::sync::OnceLock;

use ratatui::style::Color;

/// The terminal's color depth, detected once from the environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ColorCapability {
    /// 24-bit truecolor (`Color::Rgb` passes through unchanged).
    TrueColor,
    /// 256-color palette (`Color::Rgb` is downsampled to `Color::Indexed`).
    Ansi256,
}

/// Environment signals consulted when detecting terminal color capability.
///
/// Kept as a plain struct (rather than reading env inside the detector) so the
/// heuristics are unit-testable without mutating process env.
#[derive(Debug, Default, Clone, Copy)]
struct ColorEnv<'a> {
    /// `COLORTERM` — the canonical truecolor advertisement.
    colorterm: Option<&'a str>,
    /// `TERM_PROGRAM` — GUI terminal identity; many truecolor terminals set
    /// this but omit `COLORTERM` (notably on macOS app launches).
    term_program: Option<&'a str>,
    /// `TERM` — terminfo name, substring-matched as a last resort.
    term: Option<&'a str>,
    /// Set when a terminal-specific env var implying truecolor is present
    /// (`GHOSTTY_*`, `WEZTERM_*`, `KITTY_WINDOW_ID`).
    truecolor_env_marker: bool,
}

/// Detected color capability, cached for the process lifetime.
pub fn color_capability() -> ColorCapability {
    static CAP: OnceLock<ColorCapability> = OnceLock::new();
    *CAP.get_or_init(|| {
        let colorterm = std::env::var("COLORTERM").ok();
        let term_program = std::env::var("TERM_PROGRAM").ok();
        let term = std::env::var("TERM").ok();
        let truecolor_env_marker = std::env::var_os("GHOSTTY_RESOURCES_DIR").is_some()
            || std::env::var_os("GHOSTTY_BIN_DIR").is_some()
            || std::env::var_os("WEZTERM_EXECUTABLE").is_some()
            || std::env::var_os("WEZTERM_PANE").is_some()
            || std::env::var_os("KITTY_WINDOW_ID").is_some();
        detect_from_env(ColorEnv {
            colorterm: colorterm.as_deref(),
            term_program: term_program.as_deref(),
            term: term.as_deref(),
            truecolor_env_marker,
        })
    })
}

fn detect_from_env(env: ColorEnv<'_>) -> ColorCapability {
    // 1. COLORTERM is the canonical signal when present.
    if let Some(value) = env.colorterm {
        let value = value.to_ascii_lowercase();
        if value.contains("truecolor") || value.contains("24bit") {
            return ColorCapability::TrueColor;
        }
    }
    // 2. Trust the identity of known-truecolor GUI terminals, which frequently
    //    omit COLORTERM when launched from a desktop environment.
    if let Some(program) = env.term_program {
        let program = program.to_ascii_lowercase();
        const TRUECOLOR_PROGRAMS: [&str; 6] = [
            "ghostty",
            "iterm.app",
            "wezterm",
            "warp",
            "alacritty",
            "hyper",
        ];
        if TRUECOLOR_PROGRAMS.iter().any(|p| program.contains(p)) {
            return ColorCapability::TrueColor;
        }
    }
    // 3. Terminal-specific env markers (GHOSTTY_*, WEZTERM_*, KITTY_WINDOW_ID).
    if env.truecolor_env_marker {
        return ColorCapability::TrueColor;
    }
    // 4. TERM substring as a last resort.
    if let Some(term) = env.term {
        let term = term.to_ascii_lowercase();
        const TRUECOLOR_TERMS: [&str; 4] = ["kitty", "ghostty", "alacritty", "wezterm"];
        if TRUECOLOR_TERMS.iter().any(|t| term.contains(t)) {
            return ColorCapability::TrueColor;
        }
    }
    ColorCapability::Ansi256
}

/// Adapt a color to the given capability: pass truecolor through, otherwise
/// downsample `Color::Rgb` to the nearest xterm-256 index. Non-RGB colors
/// (named, already-indexed, reset) are returned unchanged.
#[allow(clippy::disallowed_methods)] // this IS the downsampler that produces palette indices
pub fn adapt_color(color: Color, capability: ColorCapability) -> Color {
    match (capability, color) {
        (ColorCapability::Ansi256, Color::Rgb(r, g, b)) => Color::Indexed(rgb_to_xterm256(r, g, b)),
        _ => color,
    }
}

/// Build an RGB color adapted to the terminal's detected capability *at call
/// time*. On truecolor terminals this is `Color::Rgb`; otherwise it is
/// downsampled to the nearest xterm-256 index.
///
/// Use this for render-time-*computed* colors (gradients, focus pulses, blended
/// diff highlights) that never pass through the static-palette
/// `Theme::downsample()` pass. Static theme colors are already adapted at load.
#[allow(clippy::disallowed_methods)] // call-time downsampler; the point is to emit indices
pub fn rgb(r: u8, g: u8, b: u8) -> Color {
    adapt_color(Color::Rgb(r, g, b), color_capability())
}

/// Adapt an already-built color to the terminal's detected capability at call
/// time. Truecolor passes through; `Color::Rgb` downsamples on Ansi256
/// terminals; non-RGB colors are unchanged.
pub fn adapt_runtime(color: Color) -> Color {
    adapt_color(color, color_capability())
}

/// Map a 24-bit RGB triple to the nearest xterm-256 palette index, choosing the
/// closer of the 6×6×6 color cube (indices 16–231) and the grayscale ramp
/// (232–255) under a green-weighted squared distance.
pub fn rgb_to_xterm256(r: u8, g: u8, b: u8) -> u8 {
    const CUBE_STEPS: [i32; 6] = [0, 95, 135, 175, 215, 255];

    fn nearest_cube_index(v: i32) -> usize {
        let mut best = 0usize;
        let mut best_dist = i32::MAX;
        for (i, &step) in CUBE_STEPS.iter().enumerate() {
            let dist = (v - step).abs();
            if dist < best_dist {
                best_dist = dist;
                best = i;
            }
        }
        best
    }

    // Eye is most sensitive to green; weight the channels accordingly.
    fn weighted_dist(a: (i32, i32, i32), b: (i32, i32, i32)) -> i32 {
        2 * (a.0 - b.0).pow(2) + 4 * (a.1 - b.1).pow(2) + 3 * (a.2 - b.2).pow(2)
    }

    let (r, g, b) = (r as i32, g as i32, b as i32);

    // Candidate 1: color cube.
    let (ri, gi, bi) = (
        nearest_cube_index(r),
        nearest_cube_index(g),
        nearest_cube_index(b),
    );
    let cube_index = 16 + 36 * ri + 6 * gi + bi;
    let cube_rgb = (CUBE_STEPS[ri], CUBE_STEPS[gi], CUBE_STEPS[bi]);

    // Candidate 2: grayscale ramp (232..=255 → values 8, 18, …, 238).
    let gray_level = ((r + g + b) / 3 - 8).clamp(0, 230);
    let gray_step = ((gray_level + 5) / 10).min(23);
    let gray_value = 8 + 10 * gray_step;
    let gray_index = 232 + gray_step;
    let gray_rgb = (gray_value, gray_value, gray_value);

    let target = (r, g, b);
    if weighted_dist(target, gray_rgb) < weighted_dist(target, cube_rgb) {
        gray_index as u8
    } else {
        cube_index as u8
    }
}

#[cfg(test)]
#[path = "color.test.rs"]
mod tests;
