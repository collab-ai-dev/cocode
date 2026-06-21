use pretty_assertions::assert_eq;

use super::*;

#[test]
fn test_truncate_description_short() {
    let short = "A short description";
    assert_eq!(truncate_description(short), short);
}

#[test]
fn test_truncate_description_at_limit() {
    let exact = "x".repeat(MAX_DESCRIPTION_LENGTH);
    assert_eq!(truncate_description(&exact), exact);
}

#[test]
fn test_truncate_description_over_limit() {
    let long = "x".repeat(3000);
    let truncated = truncate_description(&long);
    assert!(truncated.len() < 3000);
    // Truncation suffix uses a single U+2026 ellipsis, not three ASCII dots.
    assert!(truncated.ends_with("… [truncated]"));
    assert!(!truncated.contains("..."));
    assert!(truncated.starts_with(&"x".repeat(MAX_DESCRIPTION_LENGTH)));
}

#[test]
fn test_mcp_tool_content_serialization() {
    let text = McpToolContent::Text {
        text: "result".to_string(),
    };
    let json = serde_json::to_string(&text).unwrap();
    assert!(json.contains("\"type\":\"text\""));

    let image = McpToolContent::Image {
        data: "abc".to_string(),
        mime_type: "image/png".to_string(),
    };
    let json = serde_json::to_string(&image).unwrap();
    assert!(json.contains("\"type\":\"image\""));
}

#[test]
fn test_truncate_description_multibyte_boundary_no_panic() {
    // Byte index MAX_DESCRIPTION_LENGTH (2048) lands inside a 3-byte '€'
    // (starts at byte 2047), which would panic with a raw `&s[..2048]`.
    let s = format!("{}{}{}", "a".repeat(2047), "€", "z".repeat(20));
    let out = super::truncate_description(&s);
    assert!(out.ends_with("… [truncated]"));
    // The '€' straddling the cut was dropped; head is the 2047 'a's.
    assert!(out.starts_with(&"a".repeat(2047)));
    assert!(
        !out.contains('€'),
        "partial multibyte char must not survive"
    );
}
