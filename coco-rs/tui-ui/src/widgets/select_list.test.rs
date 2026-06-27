use ratatui::text::Line;

use super::SelectItem;
use super::SelectListStyle;
use super::render_select_list;
use crate::style::UiStyles;
use crate::theme::Theme;

fn line_text(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

fn items(labels: &[&str], active: usize) -> Vec<SelectItem> {
    labels
        .iter()
        .enumerate()
        .map(|(i, l)| SelectItem::new(*l).with_active(i == active))
        .collect()
}

#[test]
fn focused_row_gets_cursor_and_others_do_not() {
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let its = items(&["Auto", "Dark mode", "Light mode"], 1);
    let lines = render_select_list(&its, 1, &SelectListStyle::default(), styles);

    assert_eq!(lines.len(), 3);
    assert!(
        line_text(&lines[1]).starts_with("❯ "),
        "focused row missing cursor"
    );
    assert!(
        line_text(&lines[0]).starts_with("  "),
        "unfocused row has cursor"
    );
    assert!(line_text(&lines[2]).starts_with("  "));
}

#[test]
fn numbers_and_active_marker_render() {
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let its = items(&["Auto", "Dark mode", "Light mode"], 1);
    let lines = render_select_list(&its, 1, &SelectListStyle::default(), styles);

    // 1-based numbering on every row.
    assert!(line_text(&lines[0]).contains("1. Auto"));
    assert!(line_text(&lines[2]).contains("3. Light mode"));
    // The applied row (index 1) carries the ✔; others do not.
    assert!(line_text(&lines[1]).contains('✔'));
    assert!(!line_text(&lines[0]).contains('✔'));
}

#[test]
fn unnumbered_style_omits_numbers() {
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let its = items(&["Auto", "Dark mode"], 0);
    let style = SelectListStyle {
        numbered: false,
        ..SelectListStyle::default()
    };
    let lines = render_select_list(&its, 0, &style, styles);
    assert!(!line_text(&lines[0]).contains("1."));
    assert!(line_text(&lines[0]).contains("Auto"));
}

#[test]
fn long_list_scrolls_to_keep_selection_visible() {
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let its: Vec<SelectItem> = (0..30)
        .map(|i| SelectItem::new(format!("theme {i}")))
        .collect();
    let style = SelectListStyle {
        numbered: true,
        visible_count: 8,
    };

    let lines = render_select_list(&its, 25, &style, styles);
    assert_eq!(lines.len(), 8, "window should cap at visible_count");
    let joined: String = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");
    assert!(
        joined.contains("theme 25"),
        "selected row not in window:\n{joined}"
    );
}

#[test]
fn empty_items_render_nothing() {
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let lines = render_select_list(&[], 0, &SelectListStyle::default(), styles);
    assert!(lines.is_empty());
}
