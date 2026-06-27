//! Typed `COCO_MEMORY_STORES` config — a list of mounted memory stores.
//!
//! Mirrors the Claude Code `CLAUDE_MEMORY_STORES` schema. Each entry is
//! either a bare absolute-path string or an object form. The parser
//! ([`parse_memory_stores`]) folds the env value into
//! [`crate::sections::MemoryConfig::memory_stores`] during the
//! RuntimeConfig resolution — leaf crates never read the env directly.
//!
//! A non-empty store list enables team recall outright (the "mounted ⇒
//! enabled" precedence inversion): coco has no rollout flag, so a mounted
//! store is sufficient on its own.
//!
//! NOTE: `prompt_index` / `prompt_index_max_bytes` are parsed and
//! validated here but currently UNUSED — the network promptIndex fetch +
//! `<memory path=...>` injection is a deferred (phase 3) change.

use std::collections::HashSet;

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
    /// `.` or `..`. DEFERRED (phase 3): parsed-but-unused today.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_index: Option<String>,
    /// Optional byte cap for the fetched prompt index. DEFERRED (phase
    /// 3): parsed-but-unused today.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_index_max_bytes: Option<i64>,
}

/// Wire form of a single entry: a bare absolute-path string OR the
/// object form. Normalized into [`MemoryStore`] by [`parse_memory_stores`].
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
    #[serde(default)]
    prompt_index: Option<String>,
    #[serde(default)]
    prompt_index_max_bytes: Option<i64>,
}

/// Derive a mount name from a path's last segment.
fn derive_mount(path: &AbsolutePathBuf) -> Option<String> {
    path.as_path()
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
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
/// - each `path` must be absolute (entries with relative paths are skipped);
/// - `mount` is derived from the last path segment when absent;
/// - duplicate mounts are skipped (first wins);
/// - at most one `scope:"user"` entry is kept (extras skipped);
/// - `prompt_index` must be a safe relative path, else dropped to `None`.
///
/// Returns an empty vec on empty/blank input or invalid JSON (fail-open;
/// the failure is logged). Never panics.
pub fn parse_memory_stores(raw: &str) -> Vec<MemoryStore> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let entries: Vec<RawStore> = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(
                target: "coco::config",
                error = %err,
                "ignoring invalid COCO_MEMORY_STORES (not a valid JSON array of stores)"
            );
            return Vec::new();
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

        // Reject non-absolute paths explicitly: `AbsolutePathBuf` would
        // otherwise silently resolve a relative path against the process
        // CWD. A mounted store path must be absolute (CC `tgi()` parity).
        if !std::path::Path::new(&path_raw).is_absolute() {
            tracing::warn!(
                target: "coco::config",
                path = %path_raw,
                "skipping COCO_MEMORY_STORES entry with non-absolute path"
            );
            continue;
        }
        let path = match AbsolutePathBuf::try_from(path_raw.as_str()) {
            Ok(p) => p,
            Err(err) => {
                tracing::warn!(
                    target: "coco::config",
                    path = %path_raw,
                    error = %err,
                    "skipping COCO_MEMORY_STORES entry with invalid path"
                );
                continue;
            }
        };

        let mount = match mount
            .filter(|m| !m.trim().is_empty())
            .or_else(|| derive_mount(&path))
        {
            Some(m) => m,
            None => {
                tracing::warn!(
                    target: "coco::config",
                    path = %path.display(),
                    "skipping COCO_MEMORY_STORES entry: cannot derive mount name"
                );
                continue;
            }
        };
        if !seen_mounts.insert(mount.clone()) {
            tracing::warn!(
                target: "coco::config",
                mount = %mount,
                "skipping COCO_MEMORY_STORES entry: duplicate mount"
            );
            continue;
        }

        if matches!(scope, StoreScope::User) {
            if user_scope_used {
                tracing::warn!(
                    target: "coco::config",
                    mount = %mount,
                    "skipping COCO_MEMORY_STORES entry: more than one scope:\"user\" store"
                );
                continue;
            }
            user_scope_used = true;
        }

        let prompt_index = prompt_index.filter(|p| {
            let safe = is_safe_relative_prompt_index(p);
            if !safe {
                tracing::warn!(
                    target: "coco::config",
                    mount = %mount,
                    prompt_index = %p,
                    "dropping unsafe prompt_index (not a safe relative path)"
                );
            }
            safe
        });

        out.push(MemoryStore {
            path,
            mode,
            scope,
            mount: Some(mount),
            prompt_index,
            prompt_index_max_bytes,
        });
    }

    out
}

#[cfg(test)]
#[path = "memory_stores.test.rs"]
mod tests;
