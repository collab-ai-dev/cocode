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
use super::highlighted_label_spans;
use crate::theme::Theme;
use coco_tui_ui::style::UiStyles;
use ratatui::style::Modifier;
use ratatui::style::Style;

/// The (text, is_highlighted) runs `highlighted_label_spans` produces, using a
/// bold marker style so the two are distinguishable without asserting colors.
fn highlight_runs(label: &str, indices: &[i32]) -> Vec<(String, bool)> {
    let base = Style::default();
    let highlight = Style::default().add_modifier(Modifier::BOLD);
    highlighted_label_spans(label, indices, base, highlight)
        .into_iter()
        .map(|span| {
            (
                span.content.to_string(),
                span.style.add_modifier.contains(Modifier::BOLD),
            )
        })
        .collect()
}

#[test]
fn label_without_match_indices_stays_one_span() {
    assert_eq!(
        highlight_runs("readme.md", &[]),
        vec![("readme.md".to_string(), false)],
        "an unmatched label must not be split into runs"
    );
}

#[test]
fn matched_chars_split_into_alternating_runs() {
    // `rdm` fuzzy-matched against `readme.md`: r_e_a_d_me.md → 0, 3, 4.
    assert_eq!(
        highlight_runs("readme.md", &[0, 3, 4]),
        vec![
            ("r".to_string(), true),
            ("ea".to_string(), false),
            ("dm".to_string(), true),
            ("e.md".to_string(), false),
        ]
    );
}

#[test]
fn a_contiguous_prefix_match_is_one_highlighted_run() {
    // The slash-ranker shape: `/cle` against `/clear`.
    assert_eq!(
        highlight_runs("/clear", &[1, 2, 3]),
        vec![
            ("/".to_string(), false),
            ("cle".to_string(), true),
            ("ar".to_string(), false),
        ]
    );
}

#[test]
fn a_fully_matched_label_is_a_single_highlighted_run() {
    assert_eq!(
        highlight_runs("abc", &[0, 1, 2]),
        vec![("abc".to_string(), true)]
    );
}

#[test]
fn indices_past_the_truncated_label_are_ignored() {
    // The care point: indices are char positions into the UNTRUNCATED label, so
    // the renderer truncates first and lets the out-of-range ones fall away.
    // Shifting them onto the shorter string instead is how highlight drift gets
    // in. Here the label was cut to "read" but the matcher reported a hit at 7.
    assert_eq!(
        highlight_runs("read", &[0, 7]),
        vec![("r".to_string(), true), ("ead".to_string(), false)],
        "an index past the end must be dropped, not wrapped onto another char"
    );
}

#[test]
fn out_of_range_and_duplicate_indices_do_not_panic() {
    // Indices come from matchers this widget does not own.
    assert_eq!(
        highlight_runs("ab", &[-1, 0, 0, 99]),
        vec![("a".to_string(), true), ("b".to_string(), false)]
    );
}

#[test]
fn highlight_indices_are_char_positions_not_bytes() {
    // A multi-byte label: index 1 must mark the SECOND character, not a byte
    // inside the first one.
    assert_eq!(
        highlight_runs("中文ab", &[1]),
        vec![
            ("中".to_string(), false),
            ("文".to_string(), true),
            ("ab".to_string(), false),
        ]
    );
}

fn item(label: &str, description: Option<&str>) -> SuggestionItem {
    SuggestionItem {
        highlight_indices: Vec::new(),
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
fn overflow_reserves_bottom_row_for_scroll_indicator() {
    // 20 items into a 6-row slot: 5 item rows + 1 dim overflow indicator
    // reporting position + a scroll affordance, instead of silently dropping
    // the 6th..20th matches.
    let items: Vec<SuggestionItem> = (0..20)
        .map(|i| item(&format!("/cmd{i}"), Some("desc")))
        .collect();
    let out = render_popup(&items, 0, 40, 6);
    let lines = out.lines().collect::<Vec<_>>();
    assert!(lines[0].contains("/cmd0"), "first item row missing: {out}");
    assert!(
        lines[5].contains("1/20"),
        "expected position/total in overflow hint: {out}"
    );
    assert!(
        lines[5].contains("more"),
        "expected scroll affordance in overflow hint: {out}"
    );
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
fn slash_only_popup_drops_icon_column() {
    // A pure slash palette (all metadata None) drops the always-blank
    // kind-icon column, so the label's `/` sits at column 2 — directly
    // under the `/` the user typed after the composer's 2-col `❯ ` prefix.
    let items = vec![item("/clear", Some("Clear chat"))];
    let out = render_popup(&items, 0, 30, 1);
    let line = out.lines().next().unwrap();
    assert!(line.starts_with("▸ /clear"), "got: {line:?}");
}

#[test]
fn mixed_popup_keeps_icon_column() {
    use super::SuggestionMeta;
    // When any row carries an icon the column is reserved for every row,
    // so labels stay aligned: the agent `*` sits at col 2, label at col 4.
    let items = vec![SuggestionItem {
        highlight_indices: Vec::new(),
        label: "Plan (agent)".into(),
        description: None,
        metadata: Some(SuggestionMeta::Agent { color: None }),
    }];
    let out = render_popup(&items, 0, 40, 1);
    let line = out.lines().next().unwrap();
    assert!(line.starts_with("▸ * Plan"), "got: {line:?}");
}

#[test]
fn empty_items_renders_no_matches_placeholder() {
    // The slot stays reserved mid-session even with zero matches (fixed
    // popup slot — see viewport::popup_row_budget); the widget fills it
    // with a single dim placeholder row instead of collapsing.
    let _locale = crate::i18n::locale_test_guard("en");
    let out = render_popup(&[], 0, 30, 4);
    let lines = out.lines().collect::<Vec<_>>();
    assert!(lines[0].contains("no matches"), "got: {out}");
    assert_eq!(lines[1], " ".repeat(30));
    assert_eq!(lines[2], " ".repeat(30));
    assert_eq!(lines[3], " ".repeat(30));
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
            highlight_indices: Vec::new(),
            label: "Plan (agent)".into(),
            description: Some("Software architect agent".into()),
            metadata: Some(SuggestionMeta::Agent {
                color: Some(AgentColorName::Blue),
            }),
        },
        SuggestionItem {
            highlight_indices: Vec::new(),
            label: "Explore (agent)".into(),
            description: Some("Fast read-only search".into()),
            metadata: Some(SuggestionMeta::Agent {
                color: Some(AgentColorName::Green),
            }),
        },
        SuggestionItem {
            highlight_indices: Vec::new(),
            label: "src/lib.rs".into(),
            description: None,
            metadata: Some(SuggestionMeta::Path {
                is_directory: false,
            }),
        },
        SuggestionItem {
            highlight_indices: Vec::new(),
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
