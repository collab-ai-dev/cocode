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
