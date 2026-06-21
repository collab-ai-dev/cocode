//! Tests for the pure helpers in `coco-mcp-skills`.
//!
//! End-to-end paths ([`sync_one`], [`sync_all`]) need a live
//! [`McpConnectionManager`] so they live in the integration test suite
//! (`tests/live/`); here we only cover the URI / extraction logic.

use super::*;
use coco_mcp::discovery::DiscoveredResource;

fn make_resource(uri: &str) -> DiscoveredResource {
    DiscoveredResource {
        server_name: "test".to_string(),
        uri: uri.to_string(),
        name: "example".to_string(),
        description: None,
        mime_type: Some("text/markdown".to_string()),
    }
}

#[test]
fn is_skill_resource_matches_skill_scheme() {
    assert!(is_skill_resource(&make_resource("skill://srv/lint")));
    assert!(is_skill_resource(&make_resource(
        "skill://srv/path/to/lint"
    )));
}

#[test]
fn is_skill_resource_rejects_other_schemes() {
    assert!(!is_skill_resource(&make_resource("file:///etc/passwd")));
    assert!(!is_skill_resource(&make_resource("https://example.com/x")));
    assert!(!is_skill_resource(&make_resource("")));
}

#[test]
fn derive_skill_name_uses_last_uri_segment() {
    assert_eq!(
        derive_skill_name("skill://srv/lint-fix", "fallback"),
        "lint-fix"
    );
    assert_eq!(
        derive_skill_name("skill://srv/path/to/lint-fix", "fallback"),
        "lint-fix"
    );
    assert_eq!(
        derive_skill_name("skill://srv/lint-fix/", "fallback"),
        "lint-fix"
    );
}

#[test]
fn derive_skill_name_falls_back_when_uri_is_empty() {
    assert_eq!(
        derive_skill_name("skill://", "fallback-name"),
        "fallback-name"
    );
    // Non-skill URI: falls back unconditionally (caller has already
    // filtered to skill resources, but the function is permissive).
    assert_eq!(
        derive_skill_name("https://example.com/x", "fallback-name"),
        "fallback-name"
    );
}

#[test]
fn extract_text_content_pulls_text_field() {
    let raw = serde_json::json!({
        "contents": [
            { "uri": "skill://srv/x", "mimeType": "text/markdown", "text": "hello body" }
        ]
    });
    let result: coco_mcp::ReadResourceResult = serde_json::from_value(raw).expect("parse");
    let text = extract_text_content(&result);
    assert_eq!(text.as_deref(), Some("hello body"));
}

#[test]
fn extract_text_content_returns_none_for_blob_only_content() {
    let raw = serde_json::json!({
        "contents": [
            { "uri": "skill://srv/x", "mimeType": "application/octet-stream", "blob": "AAAA" }
        ]
    });
    let result: coco_mcp::ReadResourceResult = serde_json::from_value(raw).expect("parse");
    let text = extract_text_content(&result);
    assert!(text.is_none());
}
