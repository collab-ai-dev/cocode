//! Team Memory Sync data types.
//!
//! API contract: anthropic/anthropic#250711, #283027, #293258.

use std::collections::HashMap;

use serde::Deserialize;
use serde::Serialize;

/// Content portion of team memory data — flat key-value storage.
/// Keys are file paths relative to the team memory dir
/// (e.g. `"MEMORY.md"`, `"patterns.md"`). Values are UTF-8 string
/// content (typically Markdown).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TeamMemoryContent {
    pub entries: HashMap<String, String>,
    /// Per-key SHA-256 of entry content (`sha256:<hex>`). Added in
    /// anthropic/anthropic#283027. Optional for forward-compat with
    /// older server deployments.
    #[serde(default, rename = "entryChecksums")]
    pub entry_checksums: HashMap<String, String>,
}

/// Full response from `GET /api/claude_code/team_memory`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMemoryData {
    #[serde(rename = "organizationId")]
    pub organization_id: String,
    pub repo: String,
    pub version: i64,
    /// ISO 8601 timestamp.
    #[serde(rename = "lastModified")]
    pub last_modified: String,
    /// SHA256 with `sha256:` prefix.
    pub checksum: String,
    pub content: TeamMemoryContent,
}

/// A file skipped during push because it contains a detected secret.
/// Path is relative to the team memory directory. Only the matched
/// rule ID is recorded — never the secret value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkippedSecretFile {
    pub path: String,
    /// Gitleaks rule ID (e.g. `"github-pat"`, `"aws-access-token"`).
    #[serde(rename = "ruleId")]
    pub rule_id: String,
    /// Human-readable label derived from rule ID.
    pub label: String,
}

/// Result from a sync fetch (pull) operation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TeamMemorySyncFetchResult {
    pub success: bool,
    pub data: Option<TeamMemoryData>,
    /// `true` when the server returned 404 (no data exists yet).
    #[serde(default, rename = "isEmpty")]
    pub is_empty: bool,
    pub error: Option<String>,
}

/// Result from a sync push (delta upload) operation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TeamMemorySyncPushResult {
    pub success: bool,
    /// Count of entries actually uploaded (excludes unchanged keys).
    #[serde(default, rename = "uploadedCount")]
    pub uploaded_count: i32,
    /// Files skipped because they contained a detected secret.
    #[serde(default, rename = "skippedSecrets")]
    pub skipped_secrets: Vec<SkippedSecretFile>,
    pub error: Option<String>,
    /// `true` when the push terminated because of repeated 412
    /// PreconditionFailed conflicts. Distinct from a generic failure —
    /// the watcher uses it to throttle retries.
    #[serde(default)]
    pub conflict: bool,
    /// Number of 412 conflict-retries actually attempted. `0` for
    /// success-on-first-try; up to [`MAX_CONFLICT_RETRIES`] otherwise.
    #[serde(default, rename = "conflictRetries")]
    pub conflict_retries: i32,
    /// When the server returned a structured 413 with an effective
    /// cap on entry count, this carries the parsed value (also
    /// written into `state.server_max_entries`). `None` otherwise.
    #[serde(default, rename = "serverMaxEntries")]
    pub server_max_entries: Option<i32>,
    /// Local entry count dropped by client-side truncation because
    /// `state.server_max_entries` was set. `0` ⇒ no truncation
    /// happened. Drives the user-visible warning surface.
    #[serde(default, rename = "truncatedCount")]
    pub truncated_count: i32,
}

/// Result from a low-level upload chunk.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TeamMemorySyncUploadResult {
    pub success: bool,
    pub status: i32,
    pub error: Option<String>,
}

/// Hashes-only response from `GET ?view=hashes` — metadata + per-key
/// checksums without entry bodies.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMemoryHashesResult {
    #[serde(rename = "organizationId")]
    pub organization_id: String,
    pub repo: String,
    pub version: i64,
    #[serde(rename = "lastModified")]
    pub last_modified: String,
    pub checksum: String,
    #[serde(default, rename = "entryChecksums")]
    pub entry_checksums: HashMap<String, String>,
}

/// Mutable per-session state threaded through every sync call.
///
/// Created once per session by the watcher and passed to all sync
/// functions. Tests instantiate
/// fresh per-test for isolation.
#[derive(Debug, Default)]
pub struct SyncState {
    /// Last known server checksum (ETag) for conditional requests.
    pub last_known_checksum: Option<String>,
    /// Per-key content hash (`sha256:<hex>`) of what we believe the
    /// server holds. Populated from `entryChecksums` on pull and from
    /// local hashes on successful push. Drives delta computation —
    /// only keys whose local hash differs are uploaded.
    pub server_checksums: HashMap<String, String>,
    /// Server-enforced max_entries cap, learned from a structured 413
    /// response (anthropic/anthropic#293258 adds error_code +
    /// extra_details.max_entries). Stays `None` until a 413 is
    /// observed — the server's cap is per-org and there's no correct
    /// client-side default. While `None`, push sends everything and
    /// lets the server be authoritative.
    pub server_max_entries: Option<i32>,
}

/// Per-entry size cap.
pub const MAX_FILE_SIZE_BYTES: usize = 250_000;

/// Gateway body-size cap. Batches larger than this get split into
/// sequential PUTs (server upsert-merge makes that safe).
pub const MAX_PUT_BODY_BYTES: usize = 200_000;

/// Sync request timeout.
pub const SYNC_TIMEOUT_MS: u64 = 30_000;

/// Max attempts at refreshing `server_checksums` via `?view=hashes`
/// after a 412 conflict before giving up.
pub const MAX_CONFLICT_RETRIES: i32 = 2;

/// Structured 413 body (anthropic/anthropic#293258) — distinguished
/// from the gateway's unstructured 413 (HTML page) by the presence of
/// `error.details.error_code == "team_memory_too_many_entries"`.
///
/// We only care about `max_entries` (the effective server cap); the
/// other fields are parsed for telemetry parity but not load-bearing.
#[derive(Debug, Clone, Deserialize)]
pub struct TeamMemoryTooManyEntries {
    pub error: TeamMemoryTooManyEntriesInner,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TeamMemoryTooManyEntriesInner {
    pub details: TeamMemoryTooManyEntriesDetails,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TeamMemoryTooManyEntriesDetails {
    /// Always `"team_memory_too_many_entries"` for the structured
    /// rejection. Used to distinguish from gateway 413s.
    #[serde(default)]
    pub error_code: String,
    /// Effective max-entries cap enforced by the server (may be
    /// org-tuned).
    pub max_entries: i32,
    /// Count the server saw in the request that just got rejected.
    /// Optional for forward-compat.
    #[serde(default)]
    pub received_entries: Option<i32>,
}
