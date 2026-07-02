//! Login picker presentation — flat, filterable provider list for the no-arg
//! `/login` form. Text-surface path (mirrors `presentation::model_picker`).

use ratatui::prelude::*;

use crate::modal_pane::login_picker::filtered;
use crate::state::LoginPickerState;
use coco_tui_ui::style::UiStyles;

pub(crate) fn content(l: &LoginPickerState, styles: UiStyles<'_>) -> (String, String, Color) {
    let rows = filtered(l);
    let filter_line = if l.filter.is_empty() {
        "Type to filter".to_string()
    } else {
        format!("Filter: {}", l.filter)
    };

    let body_rows = if rows.is_empty() {
        "  (no OAuth-capable providers configured)".to_string()
    } else {
        rows.iter()
            .enumerate()
            .map(|(i, e)| {
                let marker = if i as i32 == l.selected { "❯" } else { " " };
                let badge = if e.logged_in { "  ✓ logged in" } else { "" };
                format!("{marker} {} · {}{badge}", e.provider_display, e.auth_label)
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let hints = "↑/↓ select · Enter log in · Esc cancel";
    let body = format!("{filter_line}\n\n{body_rows}\n\n{hints}");
    ("Log in to a provider".to_string(), body, styles.primary())
}
