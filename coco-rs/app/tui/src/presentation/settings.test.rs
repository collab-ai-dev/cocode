use super::*;
use pretty_assertions::assert_eq;

use crate::i18n::locale_test_guard;
use crate::theme::Theme;
use crate::widgets::settings_panel::SettingsPanelState;
use coco_config::SettingSource;
use coco_config::Settings;
use coco_config::SettingsWithSource;
use coco_tui_ui::style::UiStyles;
use std::collections::HashMap;

#[test]
fn test_settings_lines_render_searchable_registry_and_runtime_values() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let state = SettingsPanelState::default();
    let display = Default::default();

    let (title, lines, border) =
        settings_lines(&state, &display, "terminal", UiStyles::new(&theme), 20);
    let body = lines_to_text(&lines);

    assert_eq!(title, " Settings ");
    assert_eq!(border, theme.primary);
    assert!(body.contains("Type to filter settings"));
    assert!(body.contains("Appearance · Theme  terminal"));
    assert!(body.contains("Appearance · Syntax highlighting"));
    assert!(body.contains("Terminal · Resize reflow rows"));
    assert!(body.contains("↑/↓ Navigate"));
    insta::assert_snapshot!("settings_registry", body);
}

#[test]
fn test_settings_lines_filter_by_registry_keyword() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let state = SettingsPanelState {
        filter: "clipboard".to_string(),
        ..Default::default()
    };
    let display = Default::default();

    let (_, lines, _) = settings_lines(&state, &display, "default", UiStyles::new(&theme), 20);
    let body = lines_to_text(&lines);

    assert!(body.contains("Copy full response"));
    assert!(!body.contains("Syntax highlighting"));
    assert!(body.contains("copy_full_response"));
}

#[test]
fn test_settings_lines_show_empty_filter_result() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let state = SettingsPanelState {
        filter: "does-not-exist".to_string(),
        ..Default::default()
    };
    let display = Default::default();

    let (_, lines, _) = settings_lines(&state, &display, "default", UiStyles::new(&theme), 20);

    assert!(lines_to_text(&lines).contains("No matching settings"));
}

#[test]
fn values_are_projected_from_live_runtime_settings() {
    let _locale = locale_test_guard("en");
    let mut display = crate::display_settings::DisplaySettings::default();
    display.copy_full_response = true;

    assert_eq!(
        setting_value(&display, "terminal", SettingId::Theme),
        "terminal"
    );
    assert_eq!(
        setting_value(&display, "terminal", SettingId::CopyFullResponse),
        "Enabled"
    );
    assert_eq!(
        setting_value(&display, "terminal", SettingId::TerminalTitle),
        "Disabled"
    );
}

#[test]
fn value_shows_live_higher_priority_source() {
    let _locale = locale_test_guard("en");
    let settings = SettingsWithSource {
        merged: Settings::default(),
        per_source: HashMap::from([(
            SettingSource::Project,
            serde_json::json!({ "syntax_highlighting": "full" }),
        )]),
        source_paths: HashMap::new(),
    };
    let display = crate::display_settings::DisplaySettings::from_settings_with_sources(&settings);

    assert!(setting_value(&display, "default", SettingId::SyntaxHighlighting).contains("project"));
}

fn lines_to_text(lines: &[Line<'_>]) -> String {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}
