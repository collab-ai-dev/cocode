//! Tabbed settings panel state content builder.

use ratatui::prelude::Color;

use crate::presentation::settings;
use crate::widgets::settings_panel::SettingsPanelState;
use coco_tui_ui::style::UiStyles;

pub(super) fn settings_surface_content(
    s: &SettingsPanelState,
    styles: UiStyles<'_>,
) -> (String, String, Color) {
    settings::settings_surface_content(s, styles)
}
