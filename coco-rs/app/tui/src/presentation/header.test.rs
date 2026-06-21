use super::*;
use crate::state::AppState;
use crate::theme::Theme;
use coco_tui_ui::style::UiStyles;

/// Flatten a `Line` to its plain text so assertions ignore styling.
fn line_text(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

#[test]
fn header_hides_pid_badge_when_unset() {
    let theme = Theme::default();
    let state = AppState::new(); // pid defaults to the `0` sentinel
    let view = header_bar_view(&state, UiStyles::new(&theme), 80);
    let row1 = line_text(&view.info_lines[0]);
    assert!(row1.starts_with("COCO v"), "row1 = {row1:?}");
    assert!(!row1.contains("pid"), "pid badge must be hidden at pid 0");
}

#[test]
fn header_shows_pid_badge_when_set() {
    let theme = Theme::default();
    let mut state = AppState::new();
    state.session.pid = 4242;
    let view = header_bar_view(&state, UiStyles::new(&theme), 80);
    let row1 = line_text(&view.info_lines[0]);
    assert!(row1.contains("pid 4242"), "row1 = {row1:?}");
}
