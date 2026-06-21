use super::*;
use serde_json::json;

#[test]
fn returns_none_for_empty_messages() {
    let messages: Vec<Value> = vec![];
    assert_eq!(compute_marker_index_post_group(&messages), None);
}

#[test]
fn returns_none_when_last_role_is_assistant() {
    let messages = vec![
        json!({"role": "user", "content": [{"type": "text", "text": "hi"}]}),
        json!({"role": "assistant", "content": [{"type": "text", "text": "hello"}]}),
    ];
    assert_eq!(compute_marker_index_post_group(&messages), None);
}

#[test]
fn returns_index_when_last_is_user_with_content() {
    let messages = vec![
        json!({"role": "assistant", "content": [{"type": "text", "text": "hello"}]}),
        json!({"role": "user", "content": [{"type": "text", "text": "next"}]}),
    ];
    assert_eq!(compute_marker_index_post_group(&messages), Some(1));
}

#[test]
fn returns_none_when_last_user_has_empty_content() {
    let messages = vec![json!({"role": "user", "content": []})];
    assert_eq!(compute_marker_index_post_group(&messages), None);
}

#[test]
fn five_minute_ttl_omits_explicit_ttl() {
    assert_eq!(
        build_cache_control_value(AdapterCacheTtl::FiveMinutes),
        json!({"type": "ephemeral"})
    );
}

#[test]
fn one_hour_ttl_includes_one_hour_suffix() {
    assert_eq!(
        build_cache_control_value(AdapterCacheTtl::OneHour),
        json!({"type": "ephemeral", "ttl": "1h"})
    );
}

#[test]
fn attach_marker_writes_to_last_content_block() {
    let mut messages = vec![json!({
        "role": "user",
        "content": [
            {"type": "text", "text": "first"},
            {"type": "text", "text": "second"},
        ],
    })];
    attach_marker_at(&mut messages, 0, json!({"type": "ephemeral"}));
    let blocks = messages[0]["content"].as_array().unwrap();
    assert!(blocks[0].get("cache_control").is_none());
    assert_eq!(blocks[1]["cache_control"], json!({"type": "ephemeral"}));
}

#[test]
fn attach_marker_no_op_on_invalid_index() {
    let mut messages = vec![json!({"role": "user", "content": [{"type": "text", "text": "x"}]})];
    attach_marker_at(&mut messages, 99, json!({"type": "ephemeral"}));
    assert!(messages[0]["content"][0].get("cache_control").is_none());
}
