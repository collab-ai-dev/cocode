//! Widget-isolated snapshot tests for `SuggestionPopup`.
//!
//! These render the popup into a small dedicated buffer (no surrounding
//! chat / input chrome), so chrome-layout changes can't break them and
//! the snapshots stay byte-stable across unrelated UI refactors. The
//! full-screen test in `mod.test.rs::test_snapshot_autocomplete_popup`
//! still covers positioning + Z-order; this one covers pure popup
//! rendering.

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;

use super::SuggestionItem;
use super::SuggestionPopup;
use crate::theme::Theme;
use coco_tui_ui::style::UiStyles;

fn item(label: &str, description: Option<&str>) -> SuggestionItem {
    SuggestionItem {
        label: label.to_string(),
        description: description.map(ToString::to_string),
        metadata: None,
    }
}

/// Render the popup into a fixed `w × h` slot, matching the viewport layout.
fn render_popup(items: &[SuggestionItem], selected: usize, w: u16, h: u16) -> String {
    let theme = Theme::default();
    let popup = SuggestionPopup::new(items, UiStyles::new(&theme))
        .selected(selected)
        .max_visible(h as usize);

    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| frame.render_widget(popup, Rect::new(0, 0, w, h)))
        .unwrap();
    let buf = terminal.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..h {
        for x in 0..w {
            out.push(buf[(x, y)].symbol().chars().next().unwrap_or(' '));
        }
        out.push('\n');
    }
    out
}

#[test]
fn fixed_slot_keeps_reserved_rows_clear() {
    let items = vec![item("/clear", Some("Clear chat"))];
    let out = render_popup(&items, 0, 30, 4);
    let lines = out.lines().collect::<Vec<_>>();

    assert!(lines[0].contains("/clear"));
    assert_eq!(lines[1], " ".repeat(30));
    assert_eq!(lines[2], " ".repeat(30));
    assert_eq!(lines[3], " ".repeat(30));
}

#[test]
fn snapshot_short_descriptions() {
    let items = vec![
        item("/clear", Some("Clear chat")),
        item("/config", Some("Settings")),
    ];
    insta::assert_snapshot!("suggestion_popup_short", render_popup(&items, 0, 50, 6));
}

#[test]
fn snapshot_long_description_truncates_within_width() {
    // Verifies that a long description gets truncated with an ellipsis
    // so the row still fits on a single line inside the popup width.
    let items = vec![item(
        "/add-dir",
        Some("<path>  Mount an extra working directory"),
    )];
    insta::assert_snapshot!("suggestion_popup_long_desc", render_popup(&items, 0, 60, 4));
}

#[test]
fn snapshot_cjk_description_reserves_correct_width() {
    // Verifies UnicodeWidthStr is used for sizing so CJK (each char =
    // 2 columns) doesn't underestimate width and clip the right edge.
    let items = vec![item("/帮助", Some("显示帮助信息"))];
    insta::assert_snapshot!("suggestion_popup_cjk", render_popup(&items, 0, 60, 4));
}

#[test]
fn snapshot_selected_row_marker_changes() {
    let items = vec![
        item("/help", Some("Show help")),
        item("/clear", Some("Clear chat")),
        item("/config", Some("Settings")),
    ];
    insta::assert_snapshot!(
        "suggestion_popup_selected_middle",
        render_popup(&items, 1, 50, 6)
    );
}

#[test]
fn snapshot_uniform_name_column_padding() {
    // Verifies all rows share a single padded name column so the
    // descriptions line up vertically.
    let items = vec![
        item("/m", Some("model")),
        item("/clear", Some("clear chat")),
        item("/commit-push-pr", Some("commit + push + PR")),
    ];
    insta::assert_snapshot!(
        "suggestion_popup_column_alignment",
        render_popup(&items, 0, 60, 6)
    );
}

#[test]
fn empty_items_renders_nothing() {
    let out = render_popup(&[], 0, 30, 4);
    // Widget early-returns; the buffer stays as default cells (spaces).
    assert!(out.chars().all(|c| c == ' ' || c == '\n'));
}

#[test]
fn snapshot_unified_mixed_icons() {
    use super::SuggestionMeta;
    use coco_types::AgentColorName;
    // Verifies the unified `@` popup: agents (`*`) listed before files
    // (`+`), each row prefixed by its kind icon. Verifies icon dispatch
    // off `SuggestionMeta` and that agent + file rows share the column
    // grid.
    let items = vec![
        SuggestionItem {
            label: "Plan (agent)".into(),
            description: Some("Software architect agent".into()),
            metadata: Some(SuggestionMeta::Agent {
                color: Some(AgentColorName::Blue),
            }),
        },
        SuggestionItem {
            label: "Explore (agent)".into(),
            description: Some("Fast read-only search".into()),
            metadata: Some(SuggestionMeta::Agent {
                color: Some(AgentColorName::Green),
            }),
        },
        SuggestionItem {
            label: "src/lib.rs".into(),
            description: None,
            metadata: Some(SuggestionMeta::Path {
                is_directory: false,
            }),
        },
        SuggestionItem {
            label: "docs/".into(),
            description: None,
            metadata: Some(SuggestionMeta::Path { is_directory: true }),
        },
    ];
    insta::assert_snapshot!(
        "suggestion_popup_unified_mixed",
        render_popup(&items, 0, 60, 6)
    );
}
