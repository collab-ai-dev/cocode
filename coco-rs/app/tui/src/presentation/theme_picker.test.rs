use ratatui::text::Line;

use super::theme_picker_lines;
use crate::state::ThemePickerState;
use crate::theme::ThemeChoice;
use crate::theme::ThemeSetting;
use coco_tui_ui::display::SyntaxHighlighting;
use coco_tui_ui::style::UiStyles;
use coco_tui_ui::theme::Theme;

fn line_text(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

fn fixture() -> ThemePickerState {
    let choices = vec![
        ThemeChoice {
            setting: ThemeSetting::Auto,
            id: "auto".to_string(),
            label: "Auto (match terminal)".to_string(),
        },
        ThemeChoice {
            setting: ThemeSetting::Named("dark".to_string()),
            id: "dark".to_string(),
            label: "Dark mode".to_string(),
        },
    ];
    ThemePickerState {
        choices,
        selected: 1,
        original_setting: ThemeSetting::Named("dark".to_string()),
    }
}

#[test]
fn renders_numbered_list_cursor_diff_and_footer() {
    let picker = fixture();
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let lines = theme_picker_lines(&picker, SyntaxHighlighting::Enabled, styles, 60, 12);
    let joined: String = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

    // Friendly labels + numbering from the reusable list.
    assert!(joined.contains("1. Auto (match terminal)"), "{joined}");
    assert!(joined.contains("2. Dark mode"), "{joined}");
    // Focus cursor on the selected (Dark mode) row; ✔ marks the saved theme.
    let dark_row = lines
        .iter()
        .map(line_text)
        .find(|t| t.contains("Dark mode"))
        .expect("dark row");
    assert!(
        dark_row.contains("❯ "),
        "selected row missing cursor: {dark_row}"
    );
    assert!(dark_row.contains('✔'), "saved theme missing ✔: {dark_row}");
    // Live diff-preview sample is present.
    assert!(
        joined.contains("Hello, Claude"),
        "diff sample missing:\n{joined}"
    );
    // Dashed rule around the diff box.
    assert!(joined.contains('╌'), "dashed rule missing:\n{joined}");
}

#[test]
fn unsaved_themes_have_no_active_marker() {
    let picker = fixture();
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let lines = theme_picker_lines(&picker, SyntaxHighlighting::Disabled, styles, 60, 12);
    let auto_row = lines
        .iter()
        .map(line_text)
        .find(|t| t.contains("Auto (match terminal)"))
        .expect("auto row");
    assert!(
        !auto_row.contains('✔'),
        "non-saved theme should not be marked: {auto_row}"
    );
}
