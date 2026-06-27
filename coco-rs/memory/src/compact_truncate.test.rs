use super::*;
use pretty_assertions::assert_eq;

#[test]
fn passes_through_short_sections() {
    let content = "# A\nshort body\n# B\nanother short body\n";
    let out = truncate_session_memory_for_compact(content, 2_000);
    assert_eq!(out.trim(), content.trim());
}

#[test]
fn truncates_oversized_section_and_tags_it() {
    let big = "x".repeat(20_000);
    let content = format!("# A\nintro\n{big}\n# B\nshort\n");
    // 100 tokens ≈ 400 bytes — section A will overflow.
    let out = truncate_session_memory_for_compact(&content, 100);
    assert!(out.contains("section truncated"));
    // Section B should still appear intact.
    assert!(out.contains("# B"));
    assert!(out.contains("short"));
}

#[test]
fn handles_content_without_section_headers() {
    let content = "no headers here\njust prose\n";
    let out = truncate_session_memory_for_compact(content, 1_000);
    assert_eq!(out, content);
}
