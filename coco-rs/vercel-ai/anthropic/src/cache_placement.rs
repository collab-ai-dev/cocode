//! Auto cache-marker placement.
//!
//! When `cache_strategy.mode == Auto`, the adapter places a single
//! `cache_control` marker on the last content block of the last user
//! message. This is the right boundary for the agent loop's typical
//! shape (long stable system prompt + tool result history + new user
//! turn) — caching everything before the new turn maximizes hits.
//!
//! The other built-in markers (system → last_user → last_assistant)
//! remain available via `CacheControlValidator`. The `Manual` strategy
//! intentionally does nothing here — callers attach markers via
//! `provider_options` and `cache_control` blocks themselves.
//!
//! Design §10.3.

use serde_json::Value;
use serde_json::json;

use crate::messages::anthropic_messages_options::AdapterCacheTtl;

/// Compute the index into the post-group `messages` Vec where the auto
/// marker should be attached. Returns `None` when no marker is needed
/// (no messages, last message is not a user message, or the user
/// message has no content blocks).
///
/// "Post-group" means after `group_into_blocks` has merged adjacent
/// user/tool messages into one combined `user` message — so the
/// returned index points at that merged message.
pub fn compute_marker_index_post_group(messages: &[Value]) -> Option<usize> {
    let last_idx = messages.len().checked_sub(1)?;
    let last = messages.get(last_idx)?;
    let role = last.get("role").and_then(Value::as_str)?;
    if role != "user" {
        return None;
    }
    let content = last.get("content").and_then(Value::as_array)?;
    if content.is_empty() {
        return None;
    }
    Some(last_idx)
}

/// Build the `cache_control` block for an auto-placed marker. TTL maps
/// directly onto Anthropic's wire shape:
/// - `FiveMinutes` → `{"type": "ephemeral"}` (TTL omitted, server default).
/// - `OneHour` → `{"type": "ephemeral", "ttl": "1h"}`.
///
/// Server treats absent `ttl` as 5m; explicit `"5m"` is also accepted
/// but we omit it to keep the wire body minimal and match TS output
/// byte-for-byte (`promptCachingClient.ts:140`).
pub fn build_cache_control_value(ttl: AdapterCacheTtl) -> Value {
    match ttl {
        AdapterCacheTtl::FiveMinutes => json!({"type": "ephemeral"}),
        AdapterCacheTtl::OneHour => json!({"type": "ephemeral", "ttl": "1h"}),
    }
}

/// Attach a `cache_control` block to the last content block of the
/// message at `idx`. No-op if the message lacks a content array or the
/// content array is empty.
pub fn attach_marker_at(messages: &mut [Value], idx: usize, cache_control: Value) {
    let Some(message) = messages.get_mut(idx) else {
        return;
    };
    let Some(content) = message.get_mut("content").and_then(Value::as_array_mut) else {
        return;
    };
    let Some(last_block) = content.last_mut() else {
        return;
    };
    if let Some(obj) = last_block.as_object_mut() {
        obj.insert("cache_control".into(), cache_control);
    }
}

#[cfg(test)]
#[path = "cache_placement.test.rs"]
mod tests;
