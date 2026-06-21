//! Presentation for the settings state.

use ratatui::prelude::Color;

use crate::i18n::t;
use crate::widgets::settings_panel::SettingsPanelState;
use crate::widgets::settings_panel::SettingsTab;
use crate::widgets::settings_panel::syntax_highlighting_status_for_display;
use coco_tui_ui::style::UiStyles;

pub(crate) fn settings_surface_content(
    s: &SettingsPanelState,
    styles: UiStyles<'_>,
) -> (String, String, Color) {
    let tab_bar = [
        (SettingsTab::Display, t!("dialog.settings_tab_display")),
        (SettingsTab::OutputStyle, t!("dialog.settings_tab_output")),
        (
            SettingsTab::Permissions,
            t!("dialog.settings_tab_permissions"),
        ),
        (SettingsTab::About, t!("dialog.settings_tab_about")),
    ]
    .iter()
    .map(|(tab, label)| {
        if *tab == s.active_tab {
            format!("[{label}]")
        } else {
            format!(" {label} ")
        }
    })
    .collect::<Vec<_>>()
    .join("  ");

    let items: Vec<String> = match s.active_tab {
        SettingsTab::Display => {
            let mut items: Vec<String> =
                vec![t!("dialog.settings_theme_hint").to_string(), String::new()];
            let marker = if s.is_syntax_highlighting_selected() {
                "▸ "
            } else {
                "  "
            };
            let active = if s.display_settings.syntax_highlighting.is_enabled() {
                "✓ "
            } else {
                "  "
            };
            let status = syntax_highlighting_status_for_display(s.display_settings.clone());
            items.push(String::new());
            items.push(format!(
                "{marker}{active}{}: {status}",
                t!("settings.syntax_highlighting")
            ));
            let marker = if s.is_copy_full_response_selected() {
                "▸ "
            } else {
                "  "
            };
            let active = if s.display_settings.copy_full_response {
                "✓ "
            } else {
                "  "
            };
            let status = if s.display_settings.copy_full_response {
                t!("settings.enabled")
            } else {
                t!("settings.disabled")
            };
            items.push(format!(
                "{marker}{active}{}: {status}",
                t!("settings.copy_full_response")
            ));
            items
        }
        SettingsTab::OutputStyle => {
            if s.output_styles.is_empty() {
                vec![t!("dialog.settings_no_custom_styles").to_string()]
            } else {
                s.output_styles
                    .iter()
                    .enumerate()
                    .map(|(i, style)| {
                        let marker = if i as i32 == s.selected { "▸ " } else { "  " };
                        format!("{marker}{style}")
                    })
                    .collect()
            }
        }
        SettingsTab::Permissions => {
            if s.permission_rules.is_empty() {
                vec![t!("dialog.settings_no_rules").to_string()]
            } else {
                s.permission_rules
                    .iter()
                    .map(|r| format!("  {} → {} ({})", r.tool, r.behavior, r.source))
                    .collect()
            }
        }
        SettingsTab::About => vec![
            t!("dialog.settings_about_title").to_string(),
            t!("dialog.settings_about_arch").to_string(),
            t!("dialog.settings_about_built").to_string(),
        ],
    };

    let body = format!(
        "{tab_bar}\n\n{}\n\n{}",
        items.join("\n"),
        t!("dialog.hints_settings")
    );
    (
        t!("dialog.title_settings").to_string(),
        body,
        styles.primary(),
    )
}

#[cfg(test)]
#[path = "settings.test.rs"]
mod tests;
