use super::DisplayPlatform;
use super::chord_to_display_string;
use super::chord_to_string;
use super::keystroke_to_display_string;
use super::keystroke_to_string;
use crate::parser::parse_chord;
use crate::parser::parse_combo;

#[test]
fn canonical_string_orders_modifiers_consistently() {
    let combo = parse_combo("shift+ctrl+a").unwrap();
    assert_eq!(keystroke_to_string(&combo), "ctrl+shift+a");
}

#[test]
fn canonical_renders_arrow_glyphs() {
    let combo = parse_combo("up").unwrap();
    assert_eq!(keystroke_to_string(&combo), "↑");
}

#[test]
fn canonical_renders_named_keys() {
    assert_eq!(keystroke_to_string(&parse_combo("escape").unwrap()), "Esc");
    assert_eq!(keystroke_to_string(&parse_combo("space").unwrap()), "Space");
    assert_eq!(keystroke_to_string(&parse_combo("enter").unwrap()), "Enter");
}

#[test]
fn macos_uses_opt_for_alt() {
    let combo = parse_combo("alt+k").unwrap();
    assert_eq!(
        keystroke_to_display_string(&combo, DisplayPlatform::Macos),
        "opt+k",
    );
    assert_eq!(
        keystroke_to_display_string(&combo, DisplayPlatform::Linux),
        "alt+k",
    );
    assert_eq!(
        keystroke_to_display_string(&combo, DisplayPlatform::Windows),
        "alt+k",
    );
}

#[test]
fn macos_collapses_meta_to_opt() {
    let combo = parse_combo("meta+k").unwrap();
    assert_eq!(
        keystroke_to_display_string(&combo, DisplayPlatform::Macos),
        "opt+k",
    );
}

#[test]
fn cmd_renders_as_cmd_on_macos_super_elsewhere() {
    let combo = parse_combo("cmd+c").unwrap();
    assert_eq!(
        keystroke_to_display_string(&combo, DisplayPlatform::Macos),
        "cmd+c",
    );
    assert_eq!(
        keystroke_to_display_string(&combo, DisplayPlatform::Linux),
        "super+c",
    );
}

#[test]
fn cmd_canonical_includes_cmd_token() {
    // canonical (no platform branch) puts `cmd` after `meta`.
    let combo = parse_combo("cmd+c").unwrap();
    assert_eq!(super::keystroke_to_string(&combo), "cmd+c");
}

#[test]
fn chord_display_joins_with_space() {
    let chord = parse_chord("ctrl+x ctrl+k").unwrap();
    assert_eq!(
        chord_to_display_string(&chord, DisplayPlatform::Linux),
        "ctrl+x ctrl+k",
    );
    assert_eq!(chord_to_string(&chord), "ctrl+x ctrl+k");
}

#[test]
fn display_renders_function_keys_uppercase() {
    let combo = parse_combo("f2").unwrap();
    assert_eq!(
        keystroke_to_display_string(&combo, DisplayPlatform::Linux),
        "F2",
    );
    assert_eq!(keystroke_to_string(&combo), "f2");
}

#[test]
fn current_platform_returns_a_platform() {
    // Smoke test — we can't assert specific value without knowing
    // host, but it must be one of the three.
    let platform = DisplayPlatform::current();
    assert!(matches!(
        platform,
        DisplayPlatform::Macos | DisplayPlatform::Windows | DisplayPlatform::Linux,
    ));
}
