//! Standalone theme picker view.
//!
//! Renders the styled body — title, subtitle, the reusable select list, a live
//! diff preview in the focused theme, the syntax-highlight status line, and the
//! footer — as owned `Line`s for the styled modal path in `surface/modal.rs`.
//! The list itself is the domain-free `coco_tui_ui::widgets::render_select_list`
//! so other commands can reuse the same component.

use ratatui::prelude::*;
use ratatui::style::Modifier;

use crate::i18n::t;
use crate::state::ThemePickerState;
use coco_tui_ui::display::SyntaxHighlighting;
use coco_tui_ui::style::UiStyles;
use coco_tui_ui::widgets::SelectItem;
use coco_tui_ui::widgets::SelectListStyle;
use coco_tui_ui::widgets::diff_display::render_diff_lines;
use coco_tui_ui::widgets::render_select_list;

/// The demo diff shown in the preview box.
const DEMO_DIFF: &str = " function greet() {\n\
-  console.log(\"Hello, World!\");\n\
+  console.log(\"Hello, Claude!\");\n\
 }\n";

/// Build the full styled body for the theme picker. `width` is the inner modal
/// width (excluding borders/padding); `list_visible` caps how many theme rows
/// show at once so the box always fits the terminal (footer included).
pub(crate) fn theme_picker_lines(
    picker: &ThemePickerState,
    syntax: SyntaxHighlighting,
    styles: UiStyles<'_>,
    width: u16,
    list_visible: usize,
) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();

    // Title — bold accent.
    lines.push(Line::from(Span::styled(
        t!("dialog.theme_title").to_string(),
        Style::default()
            .fg(styles.accent())
            .add_modifier(Modifier::BOLD),
    )));
    // Blank line between title and subtitle.
    lines.push(Line::default());
    // Subtitle — bold, default color.
    lines.push(Line::from(Span::styled(
        t!("dialog.theme_subtitle").to_string(),
        Style::default()
            .fg(styles.text())
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::default());

    // The reusable select list. The `✔` marks the saved theme
    // (`original_setting`); focus follows navigation independently.
    let items: Vec<SelectItem> = picker
        .choices
        .iter()
        .map(|c| SelectItem::new(c.label.clone()).with_active(c.setting == picker.original_setting))
        .collect();
    let selected = picker.selected.max(0) as usize;
    let list_style = SelectListStyle {
        visible_count: list_visible.max(1),
        ..SelectListStyle::default()
    };
    lines.extend(render_select_list(&items, selected, &list_style, styles));

    lines.push(Line::default());

    // Diff preview box — dashed top/bottom rules. Rendered with the live theme
    // so it previews the focused palette's diff + syntax colors.
    let rule = "╌".repeat(width.max(1) as usize);
    let rule_style = Style::default().fg(styles.dim());
    lines.push(Line::from(Span::styled(rule.clone(), rule_style)));
    lines.extend(render_diff_lines(DEMO_DIFF, styles, width));
    lines.push(Line::from(Span::styled(rule, rule_style)));

    // Syntax-highlight status line (ctrl+t toggles).
    let syntax_text = match syntax {
        SyntaxHighlighting::Enabled => t!("dialog.theme_syntax_on"),
        SyntaxHighlighting::Disabled => t!("dialog.theme_syntax_off"),
    };
    lines.push(Line::from(Span::styled(
        format!(" {syntax_text}"),
        Style::default().fg(styles.dim()),
    )));

    lines.push(Line::default());

    // Footer.
    lines.push(Line::from(Span::styled(
        t!("dialog.theme_hint").to_string(),
        Style::default()
            .fg(styles.dim())
            .add_modifier(Modifier::ITALIC),
    )));

    lines
}

#[cfg(test)]
#[path = "theme_picker.test.rs"]
mod tests;
