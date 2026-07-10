//! Tests for the shared shell-render helpers. The `<persisted-output>`
//! envelope is now produced by the offload seam and covered by
//! `coco_tool_runtime::tool_result_offload` tests.

use super::strip_leading_blank_lines;

/// Drop a contiguous run of blank-only lines at the head, preserve
/// the first non-blank line.
#[test]
fn strip_leading_blank_lines_drops_full_blank_prefix() {
    assert_eq!(
        strip_leading_blank_lines("\n\n  \nhello\nworld"),
        "hello\nworld"
    );
    assert_eq!(strip_leading_blank_lines("hello"), "hello");
    assert_eq!(strip_leading_blank_lines(""), "");
    // A blank trailing line (no newline) is preserved.
    assert_eq!(strip_leading_blank_lines("\n   "), "   ");
}
