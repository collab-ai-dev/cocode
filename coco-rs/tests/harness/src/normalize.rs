//! Normalize volatile fields out of values before golden snapshotting.
//!
//! Snapshots break when they capture data that changes run-to-run or
//! machine-to-machine: wall-clock timestamps, generated UUIDs, and OS temp-dir
//! paths (which embed a random component and differ per platform —
//! `/tmp/.tmpXXXX` on Linux, `/var/folders/…` on macOS). `coco-secret-redact`
//! handles *credentials*; this module handles the non-secret-but-nondeterministic
//! remainder so session-replay and request goldens stay portable.
//!
//! Each helper replaces matches with a stable `<PLACEHOLDER>` token. Apply
//! [`normalize_str`] for the common bundle, or [`normalize_json_value`] to walk
//! a structured payload.

use regex::Regex;
use std::sync::OnceLock;

/// Stand-in for an ISO-8601 / RFC-3339 timestamp.
pub const TIMESTAMP_PLACEHOLDER: &str = "<TIMESTAMP>";
/// Stand-in for a UUID.
pub const UUID_PLACEHOLDER: &str = "<UUID>";
/// Stand-in for an OS temp directory (base + random tempfile component).
pub const TEMP_PATH_PLACEHOLDER: &str = "<TMP>";

fn timestamp_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // 2025-01-15T10:00:00Z, with optional fractional seconds and ±HH:MM offset.
    RE.get_or_init(|| {
        Regex::new(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})")
            .expect("timestamp regex is valid")
    })
}

fn uuid_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // 8-4-4-4-12 lowercase/uppercase hex.
    RE.get_or_init(|| {
        Regex::new(
            r"\b[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\b",
        )
        .expect("uuid regex is valid")
    })
}

fn tempfile_component_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // The random component `tempfile` appends, e.g. `.tmpAb3Xz9`.
    RE.get_or_init(|| Regex::new(r"\.tmp[A-Za-z0-9]+").expect("tempfile regex is valid"))
}

/// Replace ISO-8601 / RFC-3339 timestamps with [`TIMESTAMP_PLACEHOLDER`].
pub fn normalize_timestamps(input: &str) -> String {
    timestamp_re()
        .replace_all(input, TIMESTAMP_PLACEHOLDER)
        .into_owned()
}

/// Replace UUIDs with [`UUID_PLACEHOLDER`].
pub fn normalize_uuids(input: &str) -> String {
    uuid_re().replace_all(input, UUID_PLACEHOLDER).into_owned()
}

/// Replace OS temp-dir paths with [`TEMP_PATH_PLACEHOLDER`]. Collapses both the
/// system temp-dir prefix (`std::env::temp_dir()`) and the random tempfile
/// component, so `/tmp/.tmpAb3/x` and `/var/folders/zz/T/.tmpQ9/x` both reduce
/// to `<TMP>/x`.
pub fn normalize_temp_paths(input: &str) -> String {
    let mut out = input.to_string();
    // Replace the canonical temp-dir prefix first (longest, most specific).
    if let Some(tmp) = std::env::temp_dir().to_str() {
        let trimmed = tmp.trim_end_matches('/');
        if !trimmed.is_empty() {
            out = out.replace(trimmed, TEMP_PATH_PLACEHOLDER);
        }
    }
    // Then collapse the random `.tmpXXXX` component tempfile appends.
    tempfile_component_re()
        .replace_all(&out, TEMP_PATH_PLACEHOLDER)
        .into_owned()
}

/// Apply every string normalization (timestamps, UUIDs, temp paths) in order.
pub fn normalize_str(input: &str) -> String {
    normalize_temp_paths(&normalize_uuids(&normalize_timestamps(input)))
}

/// Recursively normalize every string *value* in a JSON document with
/// [`normalize_str`]. Object keys are left intact (keys are part of the schema,
/// not volatile data).
pub fn normalize_json_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(s) => {
            let normalized = normalize_str(s);
            if normalized != *s {
                *s = normalized;
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                normalize_json_value(item);
            }
        }
        serde_json::Value::Object(map) => {
            for (_key, v) in map.iter_mut() {
                normalize_json_value(v);
            }
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
}

#[cfg(test)]
#[path = "normalize.test.rs"]
mod tests;
