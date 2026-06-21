use super::*;

#[test]
fn test_parse_hunk_header_basic() {
    let hdr = parse_hunk_header("@@ -10,5 +20,7 @@ fn foo()").expect("should parse");
    assert_eq!(hdr.old_start, 10);
    assert_eq!(hdr.new_start, 20);
    assert_eq!(hdr.label, "fn foo()");
}

#[test]
fn test_parse_hunk_header_no_label() {
    let hdr = parse_hunk_header("@@ -1,3 +1,4 @@").expect("should parse");
    assert_eq!(hdr.old_start, 1);
    assert_eq!(hdr.new_start, 1);
    assert_eq!(hdr.label, "");
}

#[test]
fn test_parse_hunk_header_no_count() {
    let hdr = parse_hunk_header("@@ -1 +1 @@").expect("should parse");
    assert_eq!(hdr.old_start, 1);
    assert_eq!(hdr.new_start, 1);
}

#[test]
fn test_parse_hunk_header_invalid() {
    assert!(parse_hunk_header("not a hunk").is_none());
    assert!(parse_hunk_header("@@ garbage @@").is_none());
}

#[test]
fn test_diff_line_views_builds_source_backed_rows() {
    let diff = "\
--- a/foo.rs
+++ b/foo.rs
@@ -10,2 +20,3 @@ fn foo()
 context
-old line
+new line
+added";
    let rows = diff_line_views(diff);

    assert_eq!(
        rows,
        vec![
            DiffLineView::FileHeader {
                marker: "─",
                path: "a/foo.rs".to_string(),
            },
            DiffLineView::FileHeader {
                marker: "+",
                path: "b/foo.rs".to_string(),
            },
            DiffLineView::Hunk {
                old_start: 10,
                new_start: 20,
                label: "fn foo()".to_string(),
            },
            DiffLineView::Context {
                old_line: 10,
                new_line: 20,
                content: "context".to_string(),
            },
            DiffLineView::Removed {
                old_line: 11,
                content: "old line".to_string(),
                compare_to: Some("new line".to_string()),
            },
            DiffLineView::Added {
                new_line: 21,
                content: "new line".to_string(),
                compare_to: Some("old line".to_string()),
            },
            DiffLineView::Added {
                new_line: 22,
                content: "added".to_string(),
                compare_to: None,
            },
        ]
    );
}

#[test]
fn test_diff_line_views_preserves_unbalanced_removed_lines() {
    let rows = diff_line_views("-old1\n-old2\n-old3\n+new1");
    assert_eq!(
        rows,
        vec![
            DiffLineView::Removed {
                old_line: 1,
                content: "old1".to_string(),
                compare_to: Some("new1".to_string()),
            },
            DiffLineView::Added {
                new_line: 1,
                content: "new1".to_string(),
                compare_to: Some("old1".to_string()),
            },
            DiffLineView::Removed {
                old_line: 2,
                content: "old2".to_string(),
                compare_to: None,
            },
            DiffLineView::Removed {
                old_line: 3,
                content: "old3".to_string(),
                compare_to: None,
            },
        ]
    );
}

#[test]
fn test_diff_line_views_keeps_invalid_hunk_raw() {
    assert_eq!(
        diff_line_views("@@ garbage @@"),
        vec![DiffLineView::RawHunk {
            text: "@@ garbage @@".to_string(),
        }]
    );
}

#[test]
fn test_diff_line_view_window_keeps_head_and_tail() {
    let diff = "\
+one
+two
+three
+four
+five
+six";
    let window = diff_line_view_window(diff, 4);

    assert_eq!(window.omitted, 2);
    assert_eq!(
        window.head,
        vec![
            DiffLineViewRef::Added {
                new_line: 1,
                content: "one",
                compare_to: None,
            },
            DiffLineViewRef::Added {
                new_line: 2,
                content: "two",
                compare_to: None,
            },
        ]
    );
    assert_eq!(
        window.tail,
        vec![
            DiffLineViewRef::Added {
                new_line: 5,
                content: "five",
                compare_to: None,
            },
            DiffLineViewRef::Added {
                new_line: 6,
                content: "six",
                compare_to: None,
            },
        ]
    );
}
