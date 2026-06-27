use super::*;
use crate::types::XmlTag;
use pretty_assertions::assert_eq;

#[test]
fn wrap_system_reminder_matches_ts_format_exactly() {
    let wrapped = wrap_system_reminder("hello world");
    assert_eq!(
        wrapped,
        "<system-reminder>\nhello world\n</system-reminder>"
    );
}

#[test]
fn wrap_with_tag_none_returns_content_unchanged() {
    assert_eq!(wrap_with_tag("raw", XmlTag::None), "raw");
}

#[test]
fn wrap_with_tag_preserves_internal_newlines() {
    let wrapped = wrap_system_reminder("line1\nline2\nline3");
    assert_eq!(
        wrapped,
        "<system-reminder>\nline1\nline2\nline3\n</system-reminder>"
    );
}

#[test]
fn extract_system_reminder_roundtrip() {
    let wrapped = wrap_system_reminder("payload");
    assert_eq!(extract_system_reminder(&wrapped), Some("payload"));
}

#[test]
fn extract_multiline_content() {
    let wrapped = wrap_system_reminder("line1\nline2");
    assert_eq!(extract_system_reminder(&wrapped), Some("line1\nline2"));
}

#[test]
fn extract_on_unwrapped_text_returns_none() {
    assert_eq!(extract_system_reminder("not wrapped"), None);
}

#[test]
fn has_system_reminder_detects_both_tags() {
    assert!(has_system_reminder(&wrap_system_reminder("x")));
    assert!(!has_system_reminder("<system-reminder>missing close"));
    assert!(!has_system_reminder("raw"));
}

#[test]
fn has_tag_none_is_always_false() {
    assert!(!has_tag(&wrap_system_reminder("x"), XmlTag::None));
}

#[test]
fn ensure_wrapped_idempotent() {
    let once = wrap_system_reminder("x");
    let twice = ensure_wrapped(&once);
    assert_eq!(once, twice, "ensure_wrapped must not re-wrap");
}

#[test]
fn ensure_wrapped_wraps_unwrapped() {
    assert_eq!(
        ensure_wrapped("raw"),
        "<system-reminder>\nraw\n</system-reminder>"
    );
}

#[test]
fn extract_handles_empty_content() {
    let wrapped = "<system-reminder>\n\n</system-reminder>";
    assert_eq!(extract_system_reminder(wrapped), Some(""));
}
