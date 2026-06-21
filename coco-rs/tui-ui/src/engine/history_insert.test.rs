use pretty_assertions::assert_eq;
use ratatui::buffer::Buffer;
use ratatui::text::Line;

use super::*;

#[test]
fn render_history_rows_preserves_width_and_row_count() {
    let rows = render_history_rows(vec![Line::from("one"), Line::from("two")], 6);
    let buffer = rows.buffer();

    assert_eq!(buffer.area.width, 6);
    assert_eq!(buffer.area.height, 2);
    assert_eq!(*buffer, Buffer::with_lines(["one   ", "two   "]));
}

#[test]
fn render_history_rows_handles_empty_input() {
    let rows = render_history_rows(Vec::new(), 6);
    let buffer = rows.buffer();

    assert_eq!(buffer.area.width, 6);
    assert_eq!(buffer.area.height, 0);
}

#[test]
fn render_history_rows_wraps_long_line_instead_of_clipping() {
    // A logical line longer than `width` must occupy multiple rows (matching the
    // live tail's Paragraph::wrap), NOT be clipped to a single row — otherwise
    // committed scrollback silently drops the wrapped continuation.
    let rows = render_history_rows(vec![Line::from("alpha bravo cobra eagle")], 6);
    let buffer = rows.buffer();

    assert_eq!(buffer.area.width, 6);
    assert!(
        buffer.area.height >= 2,
        "expected wrapped rows, got height {}",
        buffer.area.height
    );
    let text: String = (0..buffer.area.height)
        .flat_map(|y| (0..buffer.area.width).map(move |x| (x, y)))
        .map(|(x, y)| buffer[(x, y)].symbol().to_string())
        .collect();
    assert!(
        text.contains("bravo"),
        "wrapped continuation lost: {text:?}"
    );
    assert!(
        text.contains("eagle"),
        "wrapped continuation lost: {text:?}"
    );
}

#[test]
fn history_rows_tail_slice_borrows_suffix_rows() {
    let rows = render_history_rows(
        vec![
            Line::from("one"),
            Line::from("two"),
            Line::from("three"),
            Line::from("four"),
        ],
        6,
    );

    let tail = rows.tail_slice(2);

    assert_eq!(tail.width(), 6);
    assert_eq!(tail.height(), 2);
    assert_eq!(tail.source_start_row(), 2);
    assert_eq!(tail.buffer()[(0, 2)].symbol(), "t");
    assert_eq!(tail.buffer()[(0, 3)].symbol(), "f");
}

#[test]
fn history_rows_copy_tail_from_slices_keeps_last_rows() {
    let left = render_history_rows(vec![Line::from("one"), Line::from("two")], 6);
    let right = render_history_rows(vec![Line::from("three"), Line::from("four")], 6);

    let copied =
        HistoryRows::copy_tail_from_slices(6, &[left.tail_slice(2), right.tail_slice(2)], 3);

    assert_eq!(copied.height(), 3);
    assert_eq!(
        *copied.buffer(),
        Buffer::with_lines(["two   ", "three ", "four  "])
    );
}

#[test]
fn history_rows_checked_copy_rejects_width_mismatch() {
    let left = render_history_rows(vec![Line::from("one")], 6);
    let right = render_history_rows(vec![Line::from("two")], 8);

    let copied =
        HistoryRows::try_copy_tail_from_slices(6, &[left.tail_slice(1), right.tail_slice(1)], 2);

    assert!(copied.is_none());
}
