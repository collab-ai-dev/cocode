use pretty_assertions::assert_eq;

use super::SystemTheme;
use super::cached_system_theme;
use super::detect_from_colorfgbg;
use super::set_cached_system_theme;
use super::theme_from_osc_color;

#[test]
fn osc_rgb_black_is_dark_white_is_light() {
    assert_eq!(
        theme_from_osc_color("rgb:0000/0000/0000"),
        Some(SystemTheme::Dark)
    );
    assert_eq!(
        theme_from_osc_color("rgb:ffff/ffff/ffff"),
        Some(SystemTheme::Light)
    );
}

#[test]
fn osc_rgb_typical_dark_and_light_backgrounds() {
    // A common dark editor background (~#1e1e1e) → dark.
    assert_eq!(
        theme_from_osc_color("rgb:1e1e/1e1e/1e1e"),
        Some(SystemTheme::Dark)
    );
    // A light gray (~#eeeeee) → light.
    assert_eq!(
        theme_from_osc_color("rgb:eeee/eeee/eeee"),
        Some(SystemTheme::Light)
    );
}

#[test]
fn osc_rgb_accepts_short_components_and_alpha() {
    // Single hex digit per component (scaled by 16^1 - 1 = 15).
    assert_eq!(theme_from_osc_color("rgb:0/0/0"), Some(SystemTheme::Dark));
    assert_eq!(theme_from_osc_color("rgb:f/f/f"), Some(SystemTheme::Light));
    // A trailing alpha component is ignored.
    assert_eq!(
        theme_from_osc_color("rgba:0000/0000/0000/ffff"),
        Some(SystemTheme::Dark)
    );
}

#[test]
fn osc_hash_hex_forms() {
    assert_eq!(theme_from_osc_color("#000000"), Some(SystemTheme::Dark));
    assert_eq!(theme_from_osc_color("#ffffff"), Some(SystemTheme::Light));
    assert_eq!(
        theme_from_osc_color("#000000000000"),
        Some(SystemTheme::Dark)
    );
}

#[test]
fn osc_invalid_returns_none() {
    assert_eq!(theme_from_osc_color(""), None);
    assert_eq!(theme_from_osc_color("not-a-color"), None);
    assert_eq!(theme_from_osc_color("rgb:zz/00/00"), None);
    assert_eq!(theme_from_osc_color("rgb:0000/0000"), None); // too few components
    assert_eq!(theme_from_osc_color("#12345"), None); // not a multiple of 3
}

#[test]
fn colorfgbg_rxvt_rule() {
    // Trailing field is the background index.
    assert_eq!(detect_from_colorfgbg("15;0"), Some(SystemTheme::Dark)); // bg 0
    assert_eq!(detect_from_colorfgbg("0;15"), Some(SystemTheme::Light)); // bg 15
    assert_eq!(detect_from_colorfgbg("1;7"), Some(SystemTheme::Light)); // bg 7 (white)
    assert_eq!(detect_from_colorfgbg("15;8"), Some(SystemTheme::Dark)); // bg 8 (bright black)
    // Three-field form (fg;other;bg).
    assert_eq!(detect_from_colorfgbg("1;2;6"), Some(SystemTheme::Dark));
}

#[test]
fn colorfgbg_invalid_returns_none() {
    assert_eq!(detect_from_colorfgbg(""), None);
    assert_eq!(detect_from_colorfgbg("1;99"), None); // out of 0..=15
    assert_eq!(detect_from_colorfgbg("1;abc"), None);
}

#[test]
fn cache_roundtrips_set_value() {
    // Process-global cache; only this test mutates it. Exercise both values
    // sequentially so the assertion is order-independent of other tests.
    set_cached_system_theme(SystemTheme::Light);
    assert_eq!(cached_system_theme(), Some(SystemTheme::Light));
    set_cached_system_theme(SystemTheme::Dark);
    assert_eq!(cached_system_theme(), Some(SystemTheme::Dark));
}
