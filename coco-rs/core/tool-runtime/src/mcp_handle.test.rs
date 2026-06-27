use super::*;
use serde_json::json;

#[test]
fn from_meta_lifts_anthropic_search_hint() {
    let schema = json!({
        "type": "object",
        "_meta": { "anthropic/searchHint": "query the issue tracker" }
    });
    let ann = McpToolAnnotations::from_input_schema_meta(&schema);
    assert_eq!(ann.search_hint.as_deref(), Some("query the issue tracker"));
}

#[test]
fn from_meta_lifts_generic_search_hint_when_anthropic_absent() {
    // Provider-neutral key for non-Anthropic MCP servers.
    let schema = json!({
        "type": "object",
        "_meta": { "searchHint": "query the issue tracker" }
    });
    let ann = McpToolAnnotations::from_input_schema_meta(&schema);
    assert_eq!(ann.search_hint.as_deref(), Some("query the issue tracker"));
}

#[test]
fn from_meta_prefers_anthropic_key_over_generic() {
    let schema = json!({
        "type": "object",
        "_meta": {
            "anthropic/searchHint": "namespaced hint",
            "searchHint": "generic hint",
        }
    });
    let ann = McpToolAnnotations::from_input_schema_meta(&schema);
    assert_eq!(ann.search_hint.as_deref(), Some("namespaced hint"));
}

#[test]
fn from_meta_normalizes_whitespace_and_drops_empty() {
    let collapsed = McpToolAnnotations::from_input_schema_meta(&json!({
        "_meta": { "searchHint": "find   things\n here" }
    }));
    assert_eq!(collapsed.search_hint.as_deref(), Some("find things here"));

    let empty = McpToolAnnotations::from_input_schema_meta(&json!({
        "_meta": { "searchHint": "   " }
    }));
    assert_eq!(empty.search_hint, None);
}

#[test]
fn from_meta_no_hint_is_none_and_always_load_independent() {
    let none = McpToolAnnotations::from_input_schema_meta(&json!({ "type": "object" }));
    assert_eq!(none.search_hint, None);
    assert!(!none.always_load);

    // search_hint and always_load are lifted independently.
    let both = McpToolAnnotations::from_input_schema_meta(&json!({
        "_meta": { "searchHint": "do a thing", "anthropic/alwaysLoad": true }
    }));
    assert_eq!(both.search_hint.as_deref(), Some("do a thing"));
    assert!(both.always_load);
}

#[test]
fn from_meta_always_load_accepts_both_keys() {
    let anthropic = McpToolAnnotations::from_input_schema_meta(&json!({
        "_meta": { "anthropic/alwaysLoad": true }
    }));
    assert!(anthropic.always_load);

    // Provider-neutral fallback for non-Anthropic servers.
    let generic = McpToolAnnotations::from_input_schema_meta(&json!({
        "_meta": { "alwaysLoad": true }
    }));
    assert!(generic.always_load);
}
