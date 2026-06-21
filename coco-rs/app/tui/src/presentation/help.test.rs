use super::*;
use pretty_assertions::assert_eq;

use crate::i18n::locale_test_guard;
use crate::theme::Theme;
use coco_tui_ui::style::UiStyles;

#[test]
fn help_content_renders_grouped_keymap_with_live_bindings() {
    let _locale = locale_test_guard("en");
    let state = AppState::default();
    let theme = Theme::default();

    let (title, body, border) = help_content(&state, UiStyles::new(&theme));

    assert_eq!(title, " Help ");
    assert_eq!(border, theme.primary);
    // Each section title from `KeymapGroup::title_key` should appear.
    assert!(body.contains("Cursor & history"));
    assert!(body.contains("Editing"));
    assert!(body.contains("Global hotkeys"));
    assert!(body.contains("Vim Normal mode"));
    // Spot-check rows from different groups (built-in verbs + markers
    // surface their static combo; the column is left-padded to 18 cols).
    assert!(body.contains("Ctrl+A             Move to beginning of line"));
    assert!(body.contains("!cmd               Run a shell command inline (skips the model)"));
    assert!(body.contains("@path              Autocomplete a file path, agent, or MCP resource"));
}

#[test]
fn help_content_localizes_to_zh() {
    let _locale = locale_test_guard("zh-CN");
    let state = AppState::default();
    let theme = Theme::default();

    let (title, body, _border) = help_content(&state, UiStyles::new(&theme));

    // help.title is " 帮助 " in zh-CN (single border-padded spaces).
    assert_eq!(title.trim(), "帮助");
    assert!(body.contains("光标 / 历史"));
    assert!(body.contains("Vim Normal 模式"));
}
