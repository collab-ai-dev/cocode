use super::*;
use pretty_assertions::assert_eq;

#[test]
fn find_uuids_finds_multiple() {
    let input = "x 00112233-4455-6677-8899-aabbccddeeff-k y 12345678-90ab-cdef-0123-456789abcdef";
    assert_eq!(
        find_uuids(input),
        vec![
            "00112233-4455-6677-8899-aabbccddeeff".to_string(),
            "12345678-90ab-cdef-0123-456789abcdef".to_string(),
        ]
    );
}

#[test]
fn find_uuids_ignores_invalid() {
    let input = "not-a-uuid-1234-5678-9abc-def0-123456789abc";
    assert_eq!(find_uuids(input), Vec::<String>::new());
}

#[test]
fn find_uuids_handles_non_ascii_without_overlap() {
    let input = "\u{1f642} 55e5d6f7-8a7f-4d2a-8d88-123456789012abc";
    assert_eq!(
        find_uuids(input),
        vec!["55e5d6f7-8a7f-4d2a-8d88-123456789012".to_string()]
    );
}

#[test]
fn sanitize_metric_tag_value_trims_and_fills_unspecified() {
    let msg = "///";
    assert_eq!(sanitize_metric_tag_value(msg), "unspecified");
}

#[test]
fn sanitize_metric_tag_value_replaces_invalid_chars() {
    let msg = "bad value!";
    assert_eq!(sanitize_metric_tag_value(msg), "bad_value");
}

#[test]
fn normalize_markdown_hash_location_suffix_converts_single_location() {
    assert_eq!(
        normalize_markdown_hash_location_suffix("#L74C3"),
        Some(":74:3".to_string())
    );
}

#[test]
fn normalize_markdown_hash_location_suffix_converts_ranges() {
    assert_eq!(
        normalize_markdown_hash_location_suffix("#L74C3-L76C9"),
        Some(":74:3-76:9".to_string())
    );
}

#[test]
fn truncate_str_short_unchanged() {
    assert_eq!(truncate_str("hello", 10), "hello");
}

#[test]
fn truncate_str_exact_length_unchanged() {
    assert_eq!(truncate_str("hello", 5), "hello");
}

#[test]
fn truncate_str_long_truncated() {
    let result = truncate_str("hello world", 5);
    assert!(result.ends_with("..."));
    assert!(result.len() <= 8); // 5 + "..."
}

#[test]
fn truncate_str_multibyte_boundary() {
    // 4 emojis = 16 bytes, truncate at 5 bytes — must not split emoji
    let result = truncate_str("\u{1F600}\u{1F600}\u{1F600}\u{1F600}", 5);
    assert!(result.ends_with("..."));
}

#[test]
fn truncate_for_log_short_unchanged() {
    assert_eq!(truncate_for_log("hello", 10), "hello");
}

#[test]
fn truncate_for_log_long_shows_length() {
    let result = truncate_for_log("hello world this is long", 5);
    assert!(result.starts_with("[24 chars]"));
    assert!(result.ends_with("..."));
}

#[test]
fn truncate_for_log_exact_length_unchanged() {
    assert_eq!(truncate_for_log("hello", 5), "hello");
}

#[test]
fn truncate_utf16_units_with_ellipsis_short_unchanged() {
    assert_eq!(
        truncate_utf16_units_with_ellipsis("hello", 5, 4, "…"),
        "hello"
    );
}

#[test]
fn truncate_utf16_units_with_ellipsis_truncates_ascii() {
    assert_eq!(
        truncate_utf16_units_with_ellipsis("x".repeat(81).as_str(), 80, 79, "…"),
        format!("{}…", "x".repeat(79))
    );
}

#[test]
fn truncate_utf16_units_with_ellipsis_uses_utf16_units() {
    let exact_utf16_limit = "😀".repeat(40);
    assert_eq!(
        truncate_utf16_units_with_ellipsis(&exact_utf16_limit, 80, 79, "…"),
        exact_utf16_limit
    );

    let over_utf16_limit = format!("{}a", "😀".repeat(40));
    assert_eq!(
        truncate_utf16_units_with_ellipsis(&over_utf16_limit, 80, 79, "…"),
        format!("{}…", "😀".repeat(39))
    );
}

#[test]
fn test_format_thousands() {
    use super::format_thousands;
    assert_eq!(format_thousands(0), "0");
    assert_eq!(format_thousands(999), "999");
    assert_eq!(format_thousands(1_000), "1,000");
    assert_eq!(format_thousands(1_234_567), "1,234,567");
    assert_eq!(format_thousands(-1_234_567), "-1,234,567");
    assert_eq!(format_thousands(i64::MIN), "-9,223,372,036,854,775,808");
}

// ── strip_ansi ──────────────────────────────────────────────────────

#[test]
fn strip_ansi_clean_input_borrows() {
    let input = "plain text, no escapes — 中文也可以";
    let out = strip_ansi(input);
    assert!(matches!(out, std::borrow::Cow::Borrowed(_)));
    assert_eq!(out, input);
}

#[test]
fn strip_ansi_removes_sgr_colors() {
    let input = "\u{1b}[31merror\u{1b}[0m: something \u{1b}[1;32mbold green\u{1b}[m done";
    assert_eq!(strip_ansi(input), "error: something bold green done");
}

#[test]
fn strip_ansi_removes_csi_private_mode_and_colon_params() {
    // Cursor-hide (private-mode `?`) and colon-parameter underline styles.
    let input = "\u{1b}[?25lhidden\u{1b}[?25h \u{1b}[4:3munder\u{1b}[4:0m";
    assert_eq!(strip_ansi(input), "hidden under");
}

#[test]
fn strip_ansi_removes_osc_bel_and_st_terminated() {
    let input = "\u{1b}]0;window title\u{07}before \u{1b}]8;;https://x.test\u{1b}\\link\u{1b}]8;;\u{1b}\\ after";
    assert_eq!(strip_ansi(input), "before link after");
}

#[test]
fn strip_ansi_removes_dcs_and_apc() {
    let input = "a\u{1b}Pq#0;2;0;0;0#0~~\u{1b}\\b\u{1b}_apc payload\u{1b}\\c";
    assert_eq!(strip_ansi(input), "abc");
}

#[test]
fn strip_ansi_removes_8bit_c1_controls() {
    // U+009B is the 8-bit CSI; stray C1 controls are dropped too.
    let input = "x\u{9b}31my\u{85}z";
    assert_eq!(strip_ansi(input), "xyz");
}

#[test]
fn strip_ansi_cursor_movement_and_erase() {
    let input = "progress: \u{1b}[2K\u{1b}[1G100%\u{1b}[0K";
    assert_eq!(strip_ansi(input), "progress: 100%");
}

#[test]
fn strip_ansi_keeps_text_after_malformed_sequence() {
    // ESC [ followed by a newline: the newline is real content, not part
    // of the (malformed) sequence.
    let input = "a\u{1b}[\nb";
    assert_eq!(strip_ansi(input), "a\nb");
}

#[test]
fn strip_ansi_trailing_escape_dropped() {
    assert_eq!(strip_ansi("done\u{1b}"), "done");
    assert_eq!(strip_ansi("done\u{1b}["), "done");
}

#[test]
fn strip_ansi_simple_escapes() {
    // ESC 7 / ESC 8 (save/restore cursor), ESC ( B (charset select, nF).
    // The space after ESC ( B guards that text right after an nF final byte
    // survives (the sequence is stripped without eating the next char).
    let input = "\u{1b}7text\u{1b}8 more\u{1b}(B end";
    assert_eq!(strip_ansi(input), "text more end");
}

#[test]
fn strip_ansi_multibyte_neighbors_survive() {
    let input = "构建\u{1b}[32m成功\u{1b}[0m🎉";
    assert_eq!(strip_ansi(input), "构建成功🎉");
}
