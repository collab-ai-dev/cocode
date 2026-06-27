use pretty_assertions::assert_eq;
use ratatui::style::Style;

use super::CellGrid;
use super::truncate_to_width;

fn row_text(lines: &[ratatui::text::Line<'_>], row: usize) -> String {
    lines[row]
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect()
}

#[test]
fn rect_draws_rounded_border() {
    let mut g = CellGrid::new(5, 3);
    g.rect(0, 0, 5, 3, Style::default());
    let lines = g.into_lines();
    assert_eq!(row_text(&lines, 0), "╭───╮");
    assert_eq!(row_text(&lines, 1), "│   │"); // interior blank, right border kept
    assert_eq!(row_text(&lines, 2), "╰───╯");
}

#[test]
fn crossing_strokes_merge_into_junction() {
    let mut g = CellGrid::new(3, 3);
    g.hline(0, 2, 1, Style::default());
    g.vline(1, 0, 2, Style::default());
    let lines = g.into_lines();
    // The crossing cell becomes a ┼ junction, not an overwrite.
    assert_eq!(row_text(&lines, 1), "─┼─");
}

#[test]
fn text_centered_centers_and_truncates() {
    let mut g = CellGrid::new(7, 1);
    g.text_centered(0, 0, 7, "hi", Style::default());
    assert_eq!(row_text(&g.into_lines(), 0), "  hi"); // 2-space lead, trailing trimmed

    let mut g2 = CellGrid::new(5, 1);
    g2.text_centered(0, 0, 5, "abcdefgh", Style::default());
    assert_eq!(row_text(&g2.into_lines(), 0), "abcd…");
}

#[test]
fn truncate_to_width_handles_exact_and_overflow() {
    assert_eq!(truncate_to_width("abc", 3), "abc");
    assert_eq!(truncate_to_width("abcd", 3), "ab…");
    assert_eq!(truncate_to_width("x", 0), "");
}

#[test]
fn into_lines_trims_trailing_blanks() {
    let mut g = CellGrid::new(10, 1);
    g.text(0, 0, "ab", Style::default());
    assert_eq!(row_text(&g.into_lines(), 0), "ab");
}

#[test]
fn into_lines_trims_leading_and_trailing_blank_rows() {
    // Content only on the middle row; the layout's boundary-pad rows above and
    // below must be dropped so the diagram emits no surrounding whitespace.
    let mut g = CellGrid::new(3, 3);
    g.text(0, 1, "x", Style::default());
    let lines = g.into_lines();
    assert_eq!(lines.len(), 1, "leading/trailing blank rows not trimmed");
    assert_eq!(row_text(&lines, 0), "x");
}

#[test]
fn all_blank_grid_emits_no_lines() {
    let g = CellGrid::new(4, 3);
    assert!(g.into_lines().is_empty());
}

#[test]
fn stroke_merges_corner_into_line_as_junction() {
    // F10: a turn merged onto an existing straight stroke must connect (┬), not
    // overwrite it with a lone corner — this is what keeps a shared bend wired.
    let mut g = CellGrid::new(3, 2);
    g.hline(0, 2, 0, Style::default());
    g.stroke(1, 0, '╮', Style::default()); // ╮ (down+left) | ─ (left+right) → ┬
    assert_eq!(row_text(&g.into_lines(), 0), "─┬─");
}

#[test]
fn wide_glyph_at_final_column_is_dropped_not_overflowed() {
    // F12: a width-2 glyph whose shadow would fall off the right edge is skipped
    // so the emitted row's display width never exceeds the grid width.
    let mut g = CellGrid::new(2, 1);
    g.text(0, 0, "a你", Style::default()); // 'a' fits at col 0; 你 needs cols 1+2 → dropped
    assert_eq!(row_text(&g.into_lines(), 0), "a");
}

#[test]
fn wide_glyph_within_bounds_keeps_display_width() {
    let mut g = CellGrid::new(3, 1);
    g.text(0, 0, "a你", Style::default()); // 你 at col 1, shadow at col 2 → fits
    assert_eq!(row_text(&g.into_lines(), 0), "a你");
}

#[test]
fn run_is_clear_detects_occupancy_and_bounds() {
    let mut g = CellGrid::new(5, 1);
    g.text(0, 0, "ab", Style::default());
    assert!(!g.run_is_clear(0, 0, 2), "occupied run reported clear");
    assert!(g.run_is_clear(2, 0, 3), "blank run reported occupied");
    assert!(
        !g.run_is_clear(3, 0, 5),
        "out-of-bounds run must be not-clear"
    );
    assert!(!g.run_is_clear(0, 9, 1), "out-of-row run must be not-clear");
}
