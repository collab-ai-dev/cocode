use ratatui::style::Color;

use super::Theme;
use super::ThemeName;

/// Regression guard: TS renders markdown `codespan` via `color('permission')`,
/// so inline code must be a cool permission/accent color in every built-in
/// theme — never a magenta-family ANSI color. `LightMagenta` / `Magenta` are
/// exactly what a custom terminal palette recolors to red, which is how inline
/// code paths kept reading as harsh pink/red.
#[test]
fn no_builtin_theme_uses_magenta_inline_code() {
    for &name in ThemeName::all() {
        let code_inline = Theme::from_name(name).code_inline;
        assert_ne!(
            code_inline,
            Color::LightMagenta,
            "{} inline code is LightMagenta (terminal palette recolors to red)",
            name.id()
        );
        assert_ne!(
            code_inline,
            Color::Magenta,
            "{} inline code is Magenta (terminal palette recolors to red)",
            name.id()
        );
    }
}

// ── G6: terminal-native polarity-safe theme ─────────────────────────

#[test]
fn terminal_theme_is_polarity_safe() {
    let t = Theme::from_name(ThemeName::Terminal);
    // Body and chrome inherit the terminal's own foreground.
    assert_eq!(t.text, Color::Reset);
    assert_eq!(t.primary, Color::Reset);
    assert_eq!(t.border, Color::Reset);
    assert_eq!(t.heading, Color::Reset);
    // Secondary text is de-emphasized by the DIM modifier, never a hardcoded
    // gray that washes out on tuned dark / SSH / degraded profiles.
    assert_eq!(t.text_dim, Color::Reset);
    assert_ne!(t.text_dim, Color::DarkGray);
    // Semantic status tokens still carry sparse ANSI-16 color.
    assert_eq!(t.success, Color::Green);
    assert_eq!(t.error, Color::Red);
    assert_eq!(t.warning, Color::Yellow);
    // No RGB anywhere — everything maps through the terminal's own palette.
    assert!(!matches!(t.accent, Color::Rgb(..)));
}

#[test]
fn terminal_theme_id_round_trips_and_is_listed() {
    assert_eq!(ThemeName::from_id("terminal"), Some(ThemeName::Terminal));
    assert_eq!(ThemeName::Terminal.id(), "terminal");
    assert!(ThemeName::all().contains(&ThemeName::Terminal));
}

#[test]
fn none_capability_drops_color_and_background_tints() {
    use crate::color::ColorCapability;
    let mut t = Theme::from_name(ThemeName::Dark);
    assert!(t.diff_added_bg.is_some());
    t.downsample(ColorCapability::None);
    // Every foreground color collapses to the terminal default.
    assert_eq!(t.primary, Color::Reset);
    assert_eq!(t.success, Color::Reset);
    // Background tints drop to None (inherit) rather than Some(Reset).
    assert_eq!(t.diff_added_bg, None);
    assert_eq!(t.diff_removed_bg, None);
    assert_eq!(t.user_message_bg, None);
    assert_eq!(t.code_bg, None);
}
