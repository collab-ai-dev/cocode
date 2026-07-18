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

#[test]
fn hyperlink_sidecar_tracks_a_url_across_wrapped_rows() {
    let rows = render_history_rows(
        vec![Line::from("see https://example.com/a-very-long-path now")],
        12,
    );

    assert!(
        rows.buffer()
            .content
            .iter()
            .all(|cell| !cell.symbol().contains('\x1b')),
        "OSC bytes must never enter width-bearing cell text"
    );
    assert!(rows.links().len() >= 2, "the wrapped URL should span rows");
    assert!(rows.links().iter().all(|link| {
        link.target == "https://example.com/a-very-long-path" && link.start_col < link.end_col
    }));
    assert!(
        rows.links()
            .windows(2)
            .all(|pair| pair[0].row <= pair[1].row),
        "runs must be sorted for single-pass terminal serialization"
    );
}

#[test]
fn hyperlink_sidecar_detects_relative_file_paths() {
    let rows = render_history_rows_with_base_dir(
        vec![Line::from("edit ./src/main.rs")],
        40,
        Some(std::path::Path::new("/workspace")),
    );

    let link = rows.links().first().expect("file link");
    assert!(link.target.starts_with("file://"));
    assert!(link.target.ends_with("/workspace/src/main.rs"));
}

#[test]
fn hyperlink_sidecar_detects_bare_repository_relative_file_paths() {
    let rows = render_history_rows_with_base_dir(
        vec![Line::from("edit src/lib.rs")],
        40,
        Some(std::path::Path::new("/workspace")),
    );

    let link = rows.links().first().expect("file link");
    assert_eq!(link.target, "file:///workspace/src/lib.rs");
}

#[test]
fn hyperlink_file_target_strips_line_and_column_suffix() {
    let display = "coco-rs/app/tui/src/app.rs:42:7";
    let rows = render_history_rows_with_base_dir(
        vec![Line::from(display)],
        48,
        Some(std::path::Path::new("/workspace")),
    );

    let link = rows.links().first().expect("file link");
    assert_eq!(link.target, "file:///workspace/coco-rs/app/tui/src/app.rs");
    assert_eq!(link.start_col, 0);
    assert_eq!(usize::from(link.end_col), display.len());
}

#[test]
fn relative_file_paths_require_an_explicit_base_directory() {
    let rows = render_history_rows(vec![Line::from("edit ./src/main.rs")], 40);

    assert!(rows.links().is_empty());
}

#[test]
fn ordinary_slash_tokens_are_not_inferred_as_file_links() {
    let rows = render_history_rows_with_base_dir(
        vec![Line::from(
            "and/or input/output 2026/07/18 http://example.com",
        )],
        64,
        Some(std::path::Path::new("/workspace")),
    );

    assert_eq!(rows.links().len(), 1);
    assert_eq!(rows.links()[0].target, "http://example.com");
}

#[test]
fn explicit_link_target_overrides_inferred_url_on_the_same_label() {
    let label = "https://label.example";
    let rows = render_history_rows_with_links(
        vec![Line::from(label)],
        40,
        None,
        vec![crate::engine::history_links::HistoryLinkHint {
            line: 0,
            start_byte: 0,
            end_byte: label.len(),
            target: "https://destination.example".to_string(),
        }],
    );

    assert_eq!(rows.hyperlink_runs().len(), 1);
    assert_eq!(
        rows.hyperlink_runs()[0].target,
        "https://destination.example"
    );
}

#[test]
fn explicit_link_uses_source_occurrence_when_label_repeats() {
    let text = "before before";
    let linked_start = text.rfind("before").expect("second label");
    let rows = render_history_rows_with_links(
        vec![Line::from(text)],
        40,
        None,
        vec![crate::engine::history_links::HistoryLinkHint {
            line: 0,
            start_byte: linked_start,
            end_byte: text.len(),
            target: "https://destination.example".to_string(),
        }],
    );

    assert_eq!(rows.hyperlink_runs().len(), 1);
    assert_eq!(rows.hyperlink_runs()[0].start_col, 7);
    assert_eq!(rows.hyperlink_runs()[0].end_col, 13);
}

#[test]
fn explicit_link_source_range_survives_wrap_boundary_whitespace() {
    let text = "prefix before before";
    let linked_start = text.rfind("before").expect("second label");
    let rows = render_history_rows_with_links(
        vec![Line::from(text)],
        8,
        None,
        vec![crate::engine::history_links::HistoryLinkHint {
            line: 0,
            start_byte: linked_start,
            end_byte: text.len(),
            target: "https://destination.example".to_string(),
        }],
    );

    assert_eq!(rows.hyperlink_runs().len(), 1);
    assert_eq!(rows.hyperlink_runs()[0].row, 2);
    assert_eq!(rows.hyperlink_runs()[0].start_col, 0);
    assert_eq!(rows.hyperlink_runs()[0].end_col, 6);
}

#[test]
fn copying_history_tail_translates_hyperlink_rows() {
    let rows = render_history_rows(
        vec![Line::from("plain"), Line::from("https://example.com")],
        24,
    );

    let tail = rows.tail_rows_copy(1);

    assert_eq!(tail.height(), 1);
    assert_eq!(tail.links().len(), 1);
    assert_eq!(tail.links()[0].row, 0);
}
