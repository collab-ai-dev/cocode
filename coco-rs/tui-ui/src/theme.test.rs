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
