// This module tests color quantization, so it constructs Color::Rgb inputs and
// asserts Color::Indexed outputs directly.
#![allow(clippy::disallowed_methods)]

use pretty_assertions::assert_eq;

use super::ColorCapability;
use super::ColorEnv;
use super::adapt_color;
use super::detect_from_env;
use super::rgb_to_xterm256;
use ratatui::style::Color;

/// Build a `ColorEnv` carrying only `COLORTERM`, for the canonical-signal tests.
fn colorterm(value: Option<&str>) -> ColorEnv<'_> {
    ColorEnv {
        colorterm: value,
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
