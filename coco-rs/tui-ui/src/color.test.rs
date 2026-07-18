// This module tests color quantization, so it constructs Color::Rgb inputs and
// asserts Color::Indexed outputs directly.
#![allow(clippy::disallowed_methods)]

use pretty_assertions::assert_eq;

use super::ColorCapability;
use super::ColorEnv;
use super::adapt_color;
use super::detect_from_env;
use super::rgb_to_ansi16;
use super::rgb_to_xterm256;
use super::xterm256_to_rgb;
use ratatui::style::Color;

/// Build a `ColorEnv` carrying only `COLORTERM`, for the canonical-signal tests.
fn colorterm(value: Option<&str>) -> ColorEnv<'_> {
    ColorEnv {
        colorterm: value,
        ..Default::default()
    }
}

/// Build a `ColorEnv` carrying only `TERM`.
fn term_env(term: &str) -> ColorEnv<'_> {
    ColorEnv {
        term: Some(term),
        ..Default::default()
    }
}

#[test]
fn test_rgb_to_xterm256_pure_black_maps_to_cube_origin() {
    assert_eq!(rgb_to_xterm256(0, 0, 0), 16);
}

#[test]
fn test_rgb_to_xterm256_pure_white_maps_to_cube_max() {
    assert_eq!(rgb_to_xterm256(255, 255, 255), 231);
}

#[test]
fn test_rgb_to_xterm256_mid_gray_prefers_grayscale_ramp() {
    // 128,128,128 sits exactly on a grayscale-ramp value (244) and off the cube.
    assert_eq!(rgb_to_xterm256(128, 128, 128), 244);
}

#[test]
fn test_rgb_to_xterm256_saturated_red_maps_to_cube() {
    assert_eq!(rgb_to_xterm256(255, 0, 0), 196);
}

#[test]
fn test_adapt_color_downsamples_rgb_only_on_ansi256() {
    assert_eq!(
        adapt_color(Color::Rgb(255, 0, 0), ColorCapability::Ansi256),
        Color::Indexed(196)
    );
    assert_eq!(
        adapt_color(Color::Rgb(255, 0, 0), ColorCapability::TrueColor),
        Color::Rgb(255, 0, 0)
    );
}

#[test]
fn test_adapt_color_passes_non_rgb_through() {
    assert_eq!(
        adapt_color(Color::Red, ColorCapability::Ansi256),
        Color::Red
    );
    assert_eq!(
        adapt_color(Color::Indexed(42), ColorCapability::Ansi256),
        Color::Indexed(42)
    );
}

#[test]
fn test_detect_from_env_truecolor_markers() {
    assert_eq!(
        detect_from_env(colorterm(Some("truecolor"))),
        ColorCapability::TrueColor
    );
    assert_eq!(
        detect_from_env(colorterm(Some("24bit"))),
        ColorCapability::TrueColor
    );
    assert_eq!(
        detect_from_env(colorterm(Some("TrueColor"))),
        ColorCapability::TrueColor
    );
}

#[test]
fn test_detect_from_env_defaults_to_ansi256() {
    assert_eq!(
        detect_from_env(colorterm(Some(""))),
        ColorCapability::Ansi256
    );
    assert_eq!(
        detect_from_env(colorterm(Some("256color"))),
        ColorCapability::Ansi256
    );
    assert_eq!(detect_from_env(colorterm(None)), ColorCapability::Ansi256);
    assert_eq!(
        detect_from_env(ColorEnv::default()),
        ColorCapability::Ansi256
    );
}

#[test]
fn test_detect_from_env_trusts_truecolor_term_programs_without_colorterm() {
    // macOS GUI launches frequently omit COLORTERM; trust TERM_PROGRAM.
    for program in [
        "ghostty",
        "iTerm.app",
        "WezTerm",
        "Warp",
        "alacritty",
        "Hyper",
    ] {
        assert_eq!(
            detect_from_env(ColorEnv {
                term_program: Some(program),
                ..Default::default()
            }),
            ColorCapability::TrueColor,
            "TERM_PROGRAM={program} should imply truecolor"
        );
    }
    // Apple Terminal is 256-color only and must NOT be promoted.
    assert_eq!(
        detect_from_env(ColorEnv {
            term_program: Some("Apple_Terminal"),
            ..Default::default()
        }),
        ColorCapability::Ansi256
    );
}

#[test]
fn test_detect_from_env_trusts_terminal_specific_env_marker() {
    assert_eq!(
        detect_from_env(ColorEnv {
            truecolor_env_marker: true,
            ..Default::default()
        }),
        ColorCapability::TrueColor
    );
}

#[test]
fn test_detect_from_env_matches_truecolor_term_substring() {
    for term in ["xterm-kitty", "xterm-ghostty", "alacritty", "wezterm"] {
        assert_eq!(
            detect_from_env(ColorEnv {
                term: Some(term),
                ..Default::default()
            }),
            ColorCapability::TrueColor,
            "TERM={term} should imply truecolor"
        );
    }
    // Plain 256color terminfo stays Ansi256.
    assert_eq!(
        detect_from_env(ColorEnv {
            term: Some("xterm-256color"),
            ..Default::default()
        }),
        ColorCapability::Ansi256
    );
}

// ── G4: Basic / None degradation tiers ──────────────────────────────

#[test]
fn no_color_forces_none_over_truecolor() {
    // NO_COLOR wins even when COLORTERM advertises truecolor.
    let env = ColorEnv {
        no_color: true,
        colorterm: Some("truecolor"),
        ..Default::default()
    };
    assert_eq!(detect_from_env(env), ColorCapability::None);
}

#[test]
fn dumb_term_is_none() {
    assert_eq!(detect_from_env(term_env("dumb")), ColorCapability::None);
}

#[test]
fn classic_terminals_are_basic() {
    for term in ["ansi", "linux", "vt100", "vt220", "cons25"] {
        assert_eq!(
            detect_from_env(term_env(term)),
            ColorCapability::Basic,
            "{term} should be Basic"
        );
    }
}

#[test]
fn bare_xterm_stays_ansi256() {
    // Not in the 16-color allow-list — most such terminals do 256 colors.
    assert_eq!(detect_from_env(term_env("xterm")), ColorCapability::Ansi256);
}

#[test]
fn adapt_color_basic_quantizes_to_sixteen() {
    assert!(matches!(
        adapt_color(Color::Rgb(255, 0, 0), ColorCapability::Basic),
        Color::Red | Color::LightRed
    ));
    // A 256-cube index downsamples through RGB.
    assert!(matches!(
        adapt_color(Color::Indexed(196), ColorCapability::Basic),
        Color::Red | Color::LightRed
    ));
    // An already-ANSI index is normalized to the named equivalent so the
    // backend never emits a 256-color escape for a Basic terminal.
    assert_eq!(
        adapt_color(Color::Indexed(9), ColorCapability::Basic),
        Color::LightRed
    );
}

#[test]
fn adapt_color_none_is_monochrome() {
    assert_eq!(
        adapt_color(Color::Rgb(1, 2, 3), ColorCapability::None),
        Color::Reset
    );
    assert_eq!(adapt_color(Color::Red, ColorCapability::None), Color::Reset);
    assert_eq!(
        adapt_color(Color::Indexed(5), ColorCapability::None),
        Color::Reset
    );
}

#[test]
fn rgb_to_ansi16_maps_primaries() {
    assert!(matches!(
        rgb_to_ansi16(255, 0, 0),
        Color::Red | Color::LightRed
    ));
    assert_eq!(rgb_to_ansi16(0, 0, 0), Color::Black);
    assert!(matches!(
        rgb_to_ansi16(255, 255, 255),
        Color::Gray | Color::White
    ));
}

#[test]
fn xterm256_to_rgb_covers_all_ranges() {
    assert_eq!(xterm256_to_rgb(16), (0, 0, 0)); // cube origin
    assert_eq!(xterm256_to_rgb(231), (255, 255, 255)); // cube max
    assert_eq!(xterm256_to_rgb(244), (128, 128, 128)); // mid grayscale
    assert_eq!(xterm256_to_rgb(0), (0, 0, 0)); // ANSI black
}

#[test]
fn empty_term_does_not_preempt_colorterm() {
    // TERM="" is treated like unset — it must not force None ahead of COLORTERM.
    assert_eq!(
        detect_from_env(ColorEnv {
            term: Some(""),
            colorterm: Some("truecolor"),
            ..Default::default()
        }),
        ColorCapability::TrueColor
    );
    // With nothing else, empty TERM defaults to Ansi256 like an unset TERM.
    assert_eq!(detect_from_env(term_env("")), ColorCapability::Ansi256);
}
