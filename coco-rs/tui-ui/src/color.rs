//! Terminal color-capability detection and truecolor→xterm-256 downsampling.
//!
//! Absorbed from jcode's `jcode-tui-style` color handling: terminals without
//! 24-bit color render `Color::Rgb` poorly (or not at all), so we detect the
//! capability once and quantize RGB to the nearest xterm-256 palette index when
//! truecolor is unavailable. Quantization picks the closer of the 6×6×6 color
//! cube and the 24-step grayscale ramp under a green-weighted distance.

use std::sync::OnceLock;

use ratatui::style::Color;

use crate::terminal_detect::TerminalName;
use crate::terminal_detect::terminal_info;

/// The terminal's color depth, detected once from the environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ColorCapability {
    /// 24-bit truecolor (`Color::Rgb` passes through unchanged).
    TrueColor,
    /// 256-color palette (`Color::Rgb` is downsampled to `Color::Indexed`).
    Ansi256,
    /// 16-color ANSI palette (`Color::Rgb` / `Color::Indexed` quantize to the
    /// nearest of the 16 system colors; `TERM=ansi`, `linux`, classic VTs).
    Basic,
    /// No color: `NO_COLOR` is set or `TERM=dumb`. Every color collapses to
    /// `Color::Reset` (terminal default); text modifiers still apply.
    None,
}

/// Environment signals consulted when detecting terminal color capability.
///
/// Kept as a plain struct (rather than reading env inside the detector) so the
/// heuristics are unit-testable without mutating process env. Terminal
/// *identity* is not re-sniffed here — it arrives already typed from
/// [`crate::terminal_detect`].
#[derive(Debug, Default, Clone, Copy)]
struct ColorEnv<'a> {
    /// `COLORTERM` — the canonical truecolor advertisement.
    colorterm: Option<&'a str>,
    /// Which emulator this is. Many truecolor terminals omit `COLORTERM`
    /// (notably on macOS app launches), so identity is the second signal.
    terminal: TerminalName,
    /// `TERM` — terminfo name, consulted only for the classic 16-color
    /// allow-list, which is a capability claim rather than an identity.
    term: Option<&'a str>,
    /// `NO_COLOR` is present and non-empty (per no-color.org): disable color.
    no_color: bool,
}

/// Detected color capability, cached for the process lifetime.
pub fn color_capability() -> ColorCapability {
    static CAP: OnceLock<ColorCapability> = OnceLock::new();
    *CAP.get_or_init(|| {
        let colorterm = std::env::var("COLORTERM").ok();
        let term = std::env::var("TERM").ok();
        let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
        detect_from_env(ColorEnv {
            colorterm: colorterm.as_deref(),
            terminal: terminal_info().name,
            term: term.as_deref(),
            no_color,
        })
    })
}

/// Terminals that render 24-bit color even when they advertise nothing.
fn is_truecolor_terminal(terminal: TerminalName) -> bool {
    match terminal {
        TerminalName::Alacritty
        | TerminalName::Ghostty
        | TerminalName::Hyper
        | TerminalName::Iterm2
        | TerminalName::Kitty
        | TerminalName::Warp
        | TerminalName::WezTerm => true,
        // Apple Terminal is 256-color only and must never be promoted. The
        // rest fall through to COLORTERM / the Ansi256 default.
        TerminalName::AppleTerminal
        | TerminalName::Dumb
        | TerminalName::GnomeTerminal
        | TerminalName::Konsole
        | TerminalName::Unknown
        | TerminalName::VsCode
        | TerminalName::Vte
        | TerminalName::WindowsTerminal => false,
    }
}

fn detect_from_env(env: ColorEnv<'_>) -> ColorCapability {
    // 0. Hard overrides: NO_COLOR and a dumb terminal disable color entirely,
    //    ahead of every truecolor heuristic. An *empty* TERM is NOT treated as
    //    no-color — it falls through exactly like an unset TERM (to COLORTERM /
    //    the Ansi256 default), so `TERM="" COLORTERM=truecolor` stays truecolor.
    if env.no_color || env.terminal == TerminalName::Dumb {
        return ColorCapability::None;
    }
    // 1. COLORTERM is the canonical signal when present.
    if let Some(value) = env.colorterm {
        let value = value.to_ascii_lowercase();
        if value.contains("truecolor") || value.contains("24bit") {
            return ColorCapability::TrueColor;
        }
    }
    // 2. Trust the identity of known-truecolor terminals, which frequently omit
    //    COLORTERM when launched from a desktop environment.
    if is_truecolor_terminal(env.terminal) {
        return ColorCapability::TrueColor;
    }
    // 3. Classic 16-color terminals that advertise no 256-color support.
    //    Conservative allow-list — a bare `TERM=xterm` still leans on
    //    COLORTERM/`Ansi256` above, since most such terminals do 256 colors.
    if let Some(term) = env.term {
        const BASIC_TERMS: [&str; 7] = [
            "ansi", "linux", "vt100", "vt220", "vt320", "cons25", "wsvt25",
        ];
        if BASIC_TERMS.contains(&term.to_ascii_lowercase().as_str()) {
            return ColorCapability::Basic;
        }
    }
    ColorCapability::Ansi256
}

/// Adapt a color to the given capability: pass truecolor through, otherwise
/// downsample `Color::Rgb` to the nearest xterm-256 index. Non-RGB colors
/// (named, already-indexed, reset) are returned unchanged.
#[allow(clippy::disallowed_methods)] // this IS the downsampler that produces palette indices
pub fn adapt_color(color: Color, capability: ColorCapability) -> Color {
    match capability {
        ColorCapability::TrueColor => color,
        ColorCapability::Ansi256 => match color {
            Color::Rgb(r, g, b) => Color::Indexed(rgb_to_xterm256(r, g, b)),
            _ => color,
        },
        ColorCapability::Basic => match color {
            Color::Rgb(r, g, b) => rgb_to_ansi16(r, g, b),
            // Downsample a 256-color index (only 16..=255 need it; 0..=15 are
            // already ANSI-16) back through RGB to the nearest of the 16.
            Color::Indexed(i) if i >= 16 => {
                let (r, g, b) = xterm256_to_rgb(i);
                rgb_to_ansi16(r, g, b)
            }
            Color::Indexed(i) => ansi16_color(i),
            _ => color,
        },
        // Monochrome: drop all color, keeping text modifiers (bold/dim/…).
        ColorCapability::None => Color::Reset,
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

/// Standard xterm RGB values for the 16 ANSI system colors (indices 0–15).
const ANSI16_RGB: [(i32, i32, i32); 16] = [
    (0, 0, 0),
    (205, 0, 0),
    (0, 205, 0),
    (205, 205, 0),
    (0, 0, 238),
    (205, 0, 205),
    (0, 205, 205),
    (229, 229, 229),
    (127, 127, 127),
    (255, 0, 0),
    (0, 255, 0),
    (255, 255, 0),
    (92, 92, 255),
    (255, 0, 255),
    (0, 255, 255),
    (255, 255, 255),
];

/// Map a 24-bit RGB triple to the nearest of the 16 ANSI system colors under a
/// green-weighted squared distance, returned as a named ANSI color. Named
/// colors serialize as the portable 30–37/90–97 SGR forms rather than the
/// 256-color `38;5;n` sequence that a Basic terminal may not understand.
pub fn rgb_to_ansi16(r: u8, g: u8, b: u8) -> Color {
    let target = (r as i32, g as i32, b as i32);
    let mut best = 0usize;
    let mut best_dist = i32::MAX;
    for (i, &c) in ANSI16_RGB.iter().enumerate() {
        let dist =
            2 * (target.0 - c.0).pow(2) + 4 * (target.1 - c.1).pow(2) + 3 * (target.2 - c.2).pow(2);
        if dist < best_dist {
            best_dist = dist;
            best = i;
        }
    }
    ansi16_color(best as u8)
}

fn ansi16_color(index: u8) -> Color {
    const COLORS: [Color; 16] = [
        Color::Black,
        Color::Red,
        Color::Green,
        Color::Yellow,
        Color::Blue,
        Color::Magenta,
        Color::Cyan,
        Color::Gray,
        Color::DarkGray,
        Color::LightRed,
        Color::LightGreen,
        Color::LightYellow,
        Color::LightBlue,
        Color::LightMagenta,
        Color::LightCyan,
        Color::White,
    ];
    COLORS[index.min(15) as usize]
}

/// Expand an xterm-256 palette index back to its RGB value: system colors
/// (0–15) via [`ANSI16_RGB`], the 6×6×6 cube (16–231), and the grayscale ramp
/// (232–255).
pub fn xterm256_to_rgb(i: u8) -> (u8, u8, u8) {
    match i {
        0..=15 => {
            let (r, g, b) = ANSI16_RGB[i as usize];
            (r as u8, g as u8, b as u8)
        }
        16..=231 => {
            const CUBE_STEPS: [u8; 6] = [0, 95, 135, 175, 215, 255];
            let i = i - 16;
            (
                CUBE_STEPS[(i / 36) as usize],
                CUBE_STEPS[((i / 6) % 6) as usize],
                CUBE_STEPS[(i % 6) as usize],
            )
        }
        232..=255 => {
            let v = 8 + 10 * (i - 232);
            (v, v, v)
        }
    }
}

#[cfg(test)]
#[path = "color.test.rs"]
mod tests;
