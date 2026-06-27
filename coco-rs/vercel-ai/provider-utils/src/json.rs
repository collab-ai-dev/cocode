//! JSON parsing utilities.

use serde::de::DeserializeOwned;
use serde_json::Value;
use std::io::BufRead;
use std::io::BufReader;

/// Parse JSON from bytes.
pub fn parse_json<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, serde_json::Error> {
    serde_json::from_slice(bytes)
}

/// Parse JSON from a string.
pub fn parse_json_str<T: DeserializeOwned>(s: &str) -> Result<T, serde_json::Error> {
    serde_json::from_str(s)
}

/// Parse JSON from a reader.
pub fn parse_json_reader<R: std::io::Read, T: DeserializeOwned>(
    reader: R,
) -> Result<T, serde_json::Error> {
    serde_json::from_reader(reader)
}

/// Parse a JSON event stream (Server-Sent Events format).
///
/// Each line starting with "data: " is parsed as JSON.
pub fn parse_json_event_stream<R: std::io::Read>(
    reader: R,
) -> impl Iterator<Item = Result<Value, JsonEventStreamError>> {
    let reader = BufReader::new(reader);
    reader.lines().filter_map(move |line| match line {
        Ok(line) => {
            let line = line.trim();
            if line.is_empty() || line == "data: [DONE]" {
                return None;
            }
            if let Some(data) = line.strip_prefix("data: ") {
                match serde_json::from_str(data) {
                    Ok(value) => Some(Ok(value)),
                    Err(e) => Some(Err(JsonEventStreamError::Parse(e))),
                }
            } else {
                None
            }
        }
        Err(e) => Some(Err(JsonEventStreamError::Io(e))),
    })
}

/// Error type for JSON event stream parsing.
#[derive(Debug, thiserror::Error)]
pub enum JsonEventStreamError {
    #[error("JSON parse error: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Securely parse JSON, limiting recursion depth to prevent stack overflow.
pub fn parse_json_secure<T: DeserializeOwned>(
    bytes: &[u8],
    max_depth: usize,
) -> Result<T, SecureJsonError> {
    let value: Value = serde_json::from_slice(bytes)?;
    check_depth(&value, max_depth)?;
    serde_json::from_value(value).map_err(SecureJsonError::from)
}

fn check_depth(value: &Value, max_depth: usize) -> Result<(), SecureJsonError> {
    if max_depth == 0 {
        return Err(SecureJsonError::DepthExceeded);
    }
    match value {
        Value::Object(map) => {
            for v in map.values() {
                check_depth(v, max_depth - 1)?;
            }
        }
        Value::Array(arr) => {
            for v in arr {
                check_depth(v, max_depth - 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Error type for secure JSON parsing.
#[derive(Debug, thiserror::Error)]
pub enum SecureJsonError {
    #[error("JSON depth exceeded maximum allowed")]
    DepthExceeded,
    #[error("JSON parse error: {0}")]
    Parse(#[from] serde_json::Error),
}

/// Returns `true` if `input` is a valid JSON string.
pub fn is_parsable_json(input: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(input).is_ok()
}

/// Deep-merge two JSON values.
///
/// Semantics:
/// - **Objects** merge recursively, per-key.
/// - **Arrays and primitives** in `overrides` replace `base`.
/// - **`Value::Null` in `overrides`** is treated as "no override" and
///   the corresponding `base` value (or key absence) is preserved.
///   To unset a key, omit it from `overrides` — do NOT pass `null`.
/// - **Prototype-polluting keys** (`__proto__`, `constructor`,
///   `prototype`) are dropped from `overrides`.
///
/// Null-skip rationale: callers (provider-options extras path, the
/// `thinking_convert` / `cache_convert` emit pipelines, user-written
/// `extra_body`) routinely produce JSON via `serde_json::to_value` on
/// `Option<T>` fields. Without `skip_serializing_if`, `None` would
/// serialize to `null` and a subsequent deep-merge would replace a
/// typed default with `null` — which the wire APIs (Gemini, OpenAI)
/// then reject. Null-skip makes the merge resilient to that pattern
/// without requiring every producer to add `skip_serializing_if`.
///
/// Mirrors `@ai-sdk/ai`'s `mergeObjects` and is the single deep-merge
/// implementation used across the workspace — `vercel-ai` builds
/// `merge_provider_options` on top of it for per-step overrides,
/// `coco-inference::build_call_options` uses it directly for nested
/// `extra_body` merges, and every provider adapter's `get_args`
/// deep-merges extras onto the wire body with it. One canonical
/// helper, identical semantics everywhere.
pub fn merge_json_value(base: &Value, overrides: &Value) -> Value {
    // Null in overrides → "no override, preserve base".
    if overrides.is_null() {
        return base.clone();
    }
    match (base, overrides) {
        (Value::Object(base_map), Value::Object(override_map)) => {
            let mut result = base_map.clone();
            for (key, override_value) in override_map {
                if is_prototype_polluting_key(key) {
                    continue;
                }
                // Null at any leaf key also means "no override" — skip.
                if override_value.is_null() {
                    continue;
                }
                let merged = match result.get(key) {
                    Some(base_value) => merge_json_value(base_value, override_value),
                    None => override_value.clone(),
                };
                result.insert(key.clone(), merged);
            }
            Value::Object(result)
        }
        _ => overrides.clone(),
    }
}

/// Whether `key` is one of the JS prototype-pollution keys
/// (`__proto__`, `constructor`, `prototype`). Used by deep-merge
/// helpers to drop untrusted nested keys without changing the merge
/// shape.
pub fn is_prototype_polluting_key(key: &str) -> bool {
    matches!(key, "__proto__" | "constructor" | "prototype")
}

#[cfg(test)]
#[path = "json.test.rs"]
mod tests;
