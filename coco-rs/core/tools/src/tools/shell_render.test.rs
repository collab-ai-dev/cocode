//! Tests for the shared shell-render helpers.

use super::build_persisted_output_message;
use super::format_byte_size;
use super::strip_leading_blank_lines;

/// Locks in the no-space, 1-decimal, strip-trailing-`.0` rules so any
/// drift surfaces as a test failure rather than divergent
/// `<persisted-output>` envelope text.
#[test]
fn format_byte_size_matches_ts_format_file_size() {
    assert_eq!(format_byte_size(0), "0 bytes");
    assert_eq!(format_byte_size(512), "512 bytes");
    assert_eq!(format_byte_size(1023), "1023 bytes");
    // 1024 == 1.0KB → "1KB" (trailing .0 stripped)
    assert_eq!(format_byte_size(1024), "1KB");
    // 50_000 / 1024 = 48.828… → "48.8KB"
    assert_eq!(format_byte_size(50_000), "48.8KB");
    // exactly 1MB → "1MB"
    assert_eq!(format_byte_size(1024 * 1024), "1MB");
    // 1.5MB
    assert_eq!(format_byte_size(1024 * 1024 + 512 * 1024), "1.5MB");
    // 2GB → "2GB"
    assert_eq!(format_byte_size(2 * 1024 * 1024 * 1024), "2GB");
}

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

#[test]
fn build_persisted_output_envelope_short_preview() {
    let envelope = build_persisted_output_message("/tmp/out.txt", 50_000, "small");
    assert!(envelope.starts_with("<persisted-output>\n"));
    assert!(envelope.contains("Output too large (48.8KB). Full output saved to: /tmp/out.txt"));
    assert!(envelope.contains("Preview (first 2KB):"));
    assert!(envelope.contains("\nsmall\n"));
    assert!(envelope.ends_with("</persisted-output>"));
    assert!(
        !envelope.contains("\n...\n"),
        "short preview must not append ellipsis"
    );
}

#[test]
fn build_persisted_output_envelope_long_preview_appends_ellipsis() {
    let big = "x".repeat(3000);
    let envelope = build_persisted_output_message("/tmp/big.txt", big.len(), &big);
    assert!(
        envelope.contains("\n...\n"),
        "long preview must end with ellipsis line"
    );
    // Preview slice must respect 2KB cap.
    let preview_section = envelope
        .split_once("Preview (first 2KB):\n")
        .expect("preview section")
        .1;
    let preview = preview_section
        .split_once("\n...\n")
        .expect("ellipsis terminator")
        .0;
    assert!(preview.len() <= 2000, "preview slice must fit budget");
}

#[test]
fn build_persisted_output_envelope_respects_utf8_boundaries() {
    // Cut at PREVIEW_SIZE_BYTES (2000) must walk back to a char
    // boundary so the slice stays valid UTF-8 even when a multi-byte
    // codepoint straddles the budget.
    let mut s = String::with_capacity(2010);
    s.push_str(&"a".repeat(1998));
    s.push('é'); // 2 bytes — straddles 2000 boundary
    s.push('!');
    let envelope = build_persisted_output_message("/tmp/utf.txt", s.len(), &s);
    // Just need to confirm we built a valid String — bad slice would
    // have panicked inside `build_persisted_output_message`.
    assert!(envelope.contains("<persisted-output>"));
}
