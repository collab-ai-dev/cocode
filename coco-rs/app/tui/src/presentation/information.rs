//! Presentation for read-only information surfaces.

use ratatui::prelude::Color;

use crate::i18n::t;
use crate::presentation::pager;
use crate::state::DiffViewState;
use coco_tui_ui::style::UiStyles;

pub(crate) fn diff_view_content(
    d: &DiffViewState,
    styles: UiStyles<'_>,
) -> (String, String, Color) {
    let all_lines: Vec<&str> = d.diff.lines().collect();
    let window = pager::pager_window(all_lines.len(), d.scroll, 30);
    let visible: String = all_lines
        .get(window.range())
        .unwrap_or_default()
        .iter()
        .map(|line| {
            if line.starts_with('+') && !line.starts_with("+++") {
                format!("  + {}", line.strip_prefix('+').unwrap_or(line))
            } else if line.starts_with('-') && !line.starts_with("---") {
                format!("  - {}", line.strip_prefix('-').unwrap_or(line))
            } else if line.starts_with("@@") {
                format!("  {line}")
            } else {
                format!("    {}", line.strip_prefix(' ').unwrap_or(line))
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    let position = window.position_suffix();
    (
        t!(
            "dialog.title_diff",
            path = d.path.as_str(),
            position = position.as_str()
        )
        .to_string(),
        format!("{visible}\n\n{}", t!("dialog.scroll_close_hints")),
        styles.primary(),
    )
}

#[cfg(test)]
#[path = "information.test.rs"]
mod tests;
