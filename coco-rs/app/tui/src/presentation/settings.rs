//! Styled projection for the searchable settings browser.

use ratatui::prelude::*;

use crate::display_settings::DisplaySettings;
use crate::i18n::t;
use crate::settings_registry::SettingId;
use crate::widgets::settings_panel::SettingsPanelState;
use coco_tui_ui::display::SyntaxHighlighting;
use coco_tui_ui::style::UiStyles;
use coco_tui_ui::widgets::SelectItem;
use coco_tui_ui::widgets::SelectListStyle;
use coco_tui_ui::widgets::render_select_list;

pub(crate) fn settings_lines(
    state: &SettingsPanelState,
    display_settings: &DisplaySettings,
    theme_id: &str,
    styles: UiStyles<'_>,
    list_budget: usize,
) -> (String, Vec<Line<'static>>, Color) {
    let filtered = state.filtered_settings();
    let filter = if state.filter.is_empty() {
        t!("dialog.settings_filter_empty").to_string()
    } else {
        t!("dialog.settings_filter", query = state.filter.as_str()).to_string()
    };
    let mut lines = vec![dim_line(filter, styles), Line::default()];

    if filtered.is_empty() {
        lines.push(dim_line(t!("dialog.settings_no_matches"), styles));
    } else {
        let items: Vec<SelectItem> = filtered
            .iter()
            .map(|meta| {
                SelectItem::new(format!("{} · {}", meta.category.label(), meta.id.label()))
                    .with_secondary(setting_value(display_settings, theme_id, meta.id))
            })
            .collect();
        lines.extend(render_select_list(
            &items,
            state.selected.max(0) as usize,
            &SelectListStyle {
                numbered: false,
                visible_count: list_budget.saturating_sub(5).max(1),
            },
            styles,
        ));
    }

    if let Some(meta) = state.selected_setting() {
        lines.push(Line::default());
        let detail = format!(
            "{}  ·  {}  ·  {}",
            meta.id.description(),
            meta.kind.label(),
            meta.id.key()
        );
        lines.push(dim_line(detail, styles));
    }
    lines.push(Line::default());
    lines.push(dim_line(t!("dialog.hints_settings"), styles));

    (
        t!("dialog.title_settings").to_string(),
        lines,
        styles.primary(),
    )
}

fn setting_value(display: &DisplaySettings, theme_id: &str, id: SettingId) -> String {
    let value = match id {
        SettingId::Theme => theme_id.to_string(),
        SettingId::SyntaxHighlighting => syntax_highlighting_status(display.syntax_highlighting),
        SettingId::ShowThinking => enabled(display.show_thinking),
        SettingId::CopyFullResponse => enabled(display.copy_full_response),
        SettingId::Animations => enabled(display.motion.is_animated()),
        SettingId::TerminalTitle => {
            if display.terminal_title.is_empty() {
                t!("settings.disabled").to_string()
            } else {
                t!(
                    "dialog.settings_value_items",
                    count = display.terminal_title.len()
                )
                .to_string()
            }
        }
        SettingId::Tips => enabled(display.tips),
        SettingId::ReflowMaxRows => {
            let rows = display.max_reflow_rows.get();
            if rows == usize::MAX {
                t!("dialog.settings_value_unlimited").to_string()
            } else {
                t!("dialog.settings_value_rows", count = rows).to_string()
            }
        }
        SettingId::StatusLine => enabled(display.status_line.is_some()),
        SettingId::NativeReplayCache => enabled(display.native_replay_cache.enabled),
        SettingId::FrameTelemetry => enabled(display.performance.frame_enabled),
        SettingId::MemoryTelemetry => enabled(display.performance.memory_enabled),
    };
    if let Some(source) = display.overriding_source(id) {
        format!(
            "{value} ({})",
            t!("settings.overridden_by", source = source.as_str())
        )
    } else {
        value
    }
}

fn enabled(value: bool) -> String {
    if value {
        t!("settings.enabled").to_string()
    } else {
        t!("settings.disabled").to_string()
    }
}

pub(crate) fn syntax_highlighting_status(syntax_highlighting: SyntaxHighlighting) -> String {
    match syntax_highlighting {
        SyntaxHighlighting::Off => t!("settings.syntax_off").to_string(),
        SyntaxHighlighting::Lite => t!("settings.syntax_lite").to_string(),
        SyntaxHighlighting::Full => t!("settings.syntax_full").to_string(),
    }
}

fn dim_line(text: impl Into<String>, styles: UiStyles<'_>) -> Line<'static> {
    Line::from(Span::styled(text.into(), styles.dim_style()))
}

#[cfg(test)]
#[path = "settings.test.rs"]
mod tests;
