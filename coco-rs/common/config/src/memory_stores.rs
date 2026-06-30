//! Typed `COCO_MEMORY_STORES` config — a list of mounted memory stores.
//!
//! Mirrors the Claude Code `CLAUDE_MEMORY_STORES` schema. Each entry is
//! either a bare absolute-path string or an object form. The parser
//! ([`try_parse_memory_stores`]) folds the env value into
//! [`crate::sections::MemoryConfig::memory_stores`] during the
//! RuntimeConfig resolution — leaf crates never read the env directly.
//!
//! A non-empty store list enables team recall outright (the "mounted ⇒
//! enabled" precedence inversion): coco has no rollout flag, so a mounted
//! store is sufficient on its own.
//!
//! `prompt_index` is loaded by `coco-memory` during system-prompt
//! rendering. `prompt_index_max_bytes` drives post-write compact
//! reminders when a mounted prompt index approaches its read budget.

use std::collections::HashSet;

use crate::ConfigError;
use coco_utils_absolute_path::AbsolutePathBuf;
use serde::Deserialize;
use serde::Serialize;

/// Read/write mode of a mounted store. Defaults to [`StoreMode::Rw`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StoreMode {
    /// Writable — memories may be saved here.
    #[default]
    Rw,
    /// Read-only — reference only, never written.
    Ro,
}

/// Scope of a mounted store. Defaults to [`StoreScope::Team`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StoreScope {
    /// Private to the current user.
    User,
    /// Shared across the team.
    #[default]
    Team,
}

/// A single mounted memory store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryStore {
    /// Absolute path to the mounted store directory.
    pub path: AbsolutePathBuf,
    /// Read/write mode. Defaults to `rw`.
    #[serde(default)]
    pub mode: StoreMode,
    /// Scope. Defaults to `team`. At most one `user`-scoped store is
    /// permitted across all entries.
    #[serde(default)]
    pub scope: StoreScope,
    /// Mount name. Derived from the last path segment when absent;
    /// duplicates are rejected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mount: Option<String>,
    /// Safe relative path (within the store) to a prompt-index file.
    /// Each `/`-separated segment matches `[A-Za-z0-9._-]+` and is never
    /// `.` or `..`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_index: Option<String>,
    /// Optional byte cap for post-write compact reminders on the
    /// fetched prompt index.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_index_max_bytes: Option<i64>,
}

/// Wire form of a single entry: a bare absolute-path string OR the
/// object form. Normalized into [`MemoryStore`] by [`try_parse_memory_stores`].
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum RawStore {
    /// Bare absolute-path string (`"/mnt/team-mem"`).
    Bare(String),
    /// Object form with optional fields.
    Object(RawStoreObject),
}

#[derive(Debug, Clone, Deserialize)]
struct RawStoreObject {
    path: String,
    #[serde(default)]
    mode: StoreMode,
    #[serde(default)]
    scope: StoreScope,
    #[serde(default)]
    mount: Option<String>,
    #[serde(default, rename = "promptIndex", alias = "prompt_index")]
    prompt_index: Option<String>,
    #[serde(
        default,
        rename = "promptIndexMaxBytes",
        alias = "prompt_index_max_bytes"
    )]
    prompt_index_max_bytes: Option<i64>,
}

fn invalid_memory_stores(message: impl Into<String>) -> ConfigError {
    ConfigError::generic(format!("COCO_MEMORY_STORES {}", message.into()))
}

fn validate_store_path(path: &str) -> Result<(), ConfigError> {
    if !path.starts_with('/') || path.starts_with("//") {
        return Err(invalid_memory_stores(
            "failed validation: path must be path-absolute and must not override the host",
        ));
    }
    Ok(())
}

fn is_valid_mount_name(mount: &str) -> bool {
    !mount.is_empty()
        && mount
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

fn sanitize_mount_segment(segment: &str) -> String {
    segment
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-') {
                char::from(b)
            } else {
                '-'
            }
        })
        .collect()
}

/// Derive a mount name from a path's last segment.
fn derive_mount(path: &str) -> Result<String, ConfigError> {
    let trimmed = path.trim_end_matches('/');
    let segment = trimmed.rsplit('/').next().unwrap_or_default();
    if segment.is_empty() {
        return Err(invalid_memory_stores(format!(
            "cannot derive mount name from path: {path}"
        )));
    }
    let mount = sanitize_mount_segment(segment);
    if mount.is_empty() || mount == "." || mount == ".." {
        return Err(invalid_memory_stores(format!(
            "derived mount name is not a valid path segment: {segment}"
        )));
    }
    Ok(mount)
}

/// Validate a `prompt_index` as a safe relative path: each
/// `/`-separated segment matches `[A-Za-z0-9._-]+` and is never `.` or
/// `..`. Returns `false` for absolute, empty, or traversal paths.
fn is_safe_relative_prompt_index(rel: &str) -> bool {
    if rel.is_empty() || rel.starts_with('/') || rel.contains('\\') {
        return false;
    }
    let mut any_segment = false;
    for segment in rel.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return false;
        }
        if !segment
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        {
            return false;
        }
        any_segment = true;
    }
    any_segment
}

/// Parse `COCO_MEMORY_STORES` (JSON array) into validated stores.
///
/// Invariants applied:
/// - each `path` must be a host-local absolute POSIX path;
/// - `mount` is derived from the last path segment when absent;
/// - explicit `mount` must match `[A-Za-z0-9_-]+`;
/// - duplicate mounts are rejected;
/// - more than one `scope:"user"` entry is rejected;
/// - `promptIndex` must be a safe relative path;
/// - `promptIndexMaxBytes`, when present, must be positive.
///
/// Empty/blank input returns an empty vec. Invalid non-empty input returns
/// an error so runtime config can fail fast, matching Claude Code's
/// `CLAUDE_MEMORY_STORES` handling.
pub fn try_parse_memory_stores(raw: &str) -> Result<Vec<MemoryStore>, ConfigError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let entries: Vec<RawStore> = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(err) => {
            return Err(invalid_memory_stores(format!("is not valid JSON: {err}")));
        }
    };

    let mut seen_mounts: HashSet<String> = HashSet::new();
    let mut user_scope_used = false;
    let mut out: Vec<MemoryStore> = Vec::new();

    for entry in entries {
        let (path_raw, mode, scope, mount, prompt_index, prompt_index_max_bytes) = match entry {
            RawStore::Bare(path) => (
                path,
                StoreMode::default(),
                StoreScope::default(),
                None,
                None,
                None,
            ),
            RawStore::Object(o) => (
                o.path,
                o.mode,
                o.scope,
                o.mount,
                o.prompt_index,
                o.prompt_index_max_bytes,
            ),
        };

        validate_store_path(&path_raw)?;
        let path = match AbsolutePathBuf::try_from(path_raw.as_str()) {
            Ok(p) => p,
            Err(err) => {
                return Err(invalid_memory_stores(format!(
                    "failed validation for path {path_raw:?}: {err}"
                )));
            }
        };

        let mount = match mount {
            Some(m) => {
                if !is_valid_mount_name(&m) {
                    return Err(invalid_memory_stores(
                        "failed validation: mount must match /^[A-Za-z0-9_-]+$/",
                    ));
                }
                m
            }
            None => derive_mount(&path_raw)?,
        };
        if !seen_mounts.insert(mount.clone()) {
            return Err(invalid_memory_stores(format!(
                "has duplicate mount: {mount}"
            )));
        }

        if matches!(scope, StoreScope::User) {
            if user_scope_used {
                return Err(invalid_memory_stores(
                    "has more than one scope:\"user\" entry",
                ));
            }
            user_scope_used = true;
        }

        if let Some(p) = &prompt_index
            && !is_safe_relative_prompt_index(p)
        {
            return Err(invalid_memory_stores(
                "failed validation: promptIndex segments must match [A-Za-z0-9._-]+ and must not be . or ..",
            ));
        }

        if let Some(max_bytes) = prompt_index_max_bytes
            && max_bytes <= 0
        {
            return Err(invalid_memory_stores(
                "failed validation: promptIndexMaxBytes must be a positive integer",
            ));
        }

        out.push(MemoryStore {
            path,
            mode,
            scope,
            mount: Some(mount),
            prompt_index,
            prompt_index_max_bytes,
        });
    }

    Ok(out)
}

/// Backwards-compatible helper for callers that do not participate in
/// startup config failure. Runtime config resolution uses
/// [`try_parse_memory_stores`] so invalid env input is fail-fast.
pub fn parse_memory_stores(raw: &str) -> Vec<MemoryStore> {
    match try_parse_memory_stores(raw) {
        Ok(stores) => stores,
        Err(err) => {
            tracing::warn!(
                target: "coco::config",
                error = %err,
                "ignoring invalid COCO_MEMORY_STORES"
            );
            Vec::new()
        }
    }
}

#[cfg(test)]
#[path = "memory_stores.test.rs"]
mod tests;
