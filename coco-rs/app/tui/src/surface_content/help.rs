//! Help state content builder.

use ratatui::prelude::Color;

use crate::presentation::help;
use crate::state::AppState;
use coco_tui_ui::style::UiStyles;

pub(super) fn help_content(state: &AppState, styles: UiStyles<'_>) -> (String, String, Color) {
    help::help_content(state, styles)
}
