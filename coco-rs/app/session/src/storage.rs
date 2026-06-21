//! Session transcript persistence via JSONL rollout format.
//!
//! JSONL append-only transcript at
//! `~/.coco/projects/{sanitized_cwd}/{session_id}.jsonl`.
//!
//! Each line is a self-contained JSON entry: transcript messages
//! (user/assistant/system), metadata entries (custom-title, tag,
//! last-prompt), and compaction markers. The file is append-only
//! during normal operation; compaction rewrites are handled separately.

use coco_paths::ProjectPaths;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashSet;
use std::io::BufRead;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use uuid::Uuid;

#[path = "storage/preview.rs"]
mod preview;
#[path = "storage/wire.rs"]
mod wire;
use preview::extract_text_content;
use preview::is_synthetic_first_prompt_candidate;
#[cfg(test)]
use preview::truncate_prompt;
pub use wire::TranscriptEntryOptions;

use wire::is_compact_boundary_message;
pub use wire::messages_from_transcript_entry;
use wire::remember_assistant_tool_calls;
use wire::source_assistant_uuid_for_tool_result;
use wire::tool_result_use_id;
pub use wire::transcript_entries_for_message;

/// Maximum transcript file size we will fully read into memory (50 MB).
const MAX_TRANSCRIPT_READ_BYTES: u64 = 50 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Entry types
// ---------------------------------------------------------------------------

/// Closed set of `entry_type` discriminators we write to the JSONL.
/// Centralised so the write side (`storage::wire::transcript_entries_for_message`),
/// the read side (`storage::wire::reconstruct_regular_message`), and
/// the lite-metadata head/tail scan (`storage::read_transcript_metadata`)
/// can't drift.
pub mod entry_kind {
    pub const USER: &str = "user";
    pub const ASSISTANT: &str = "assistant";
    pub const SYSTEM: &str = "system";
    pub const ATTACHMENT: &str = "attachment";
}

/// Token usage for a single transcript entry.
///
/// Wire shape is **snake_case JSON** — Rust-native. See module doc
/// for the cross-implementation policy.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TranscriptUsage {
    pub input_tokens: i64,
    pub output_tokens: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_tokens: Option<i64>,
}

/// A transcript message entry (user, assistant, system, attachment).
///
/// **Wire format is Rust-native snake_case JSON.** Coco-rs deliberately
/// does NOT match the original claude-code byte layout — the file content
/// is semantically equivalent (same UUIDs, same timestamps, same
/// `tool_use_id`-keyed records, same chain semantics, same metadata
/// categories) but field names follow Rust convention so we don't
/// have to maintain `camelCase` serde aliases on every new struct
/// member. TS-written transcripts must go through
/// `coco_session::import_ts` to migrate; cross-load is not supported.
///
/// `timestamp` is an ISO 8601 / RFC 3339 string — the leaf walk in
/// `recovery.rs` sorts leaves by lexicographic timestamp, which is
/// only correct for that format.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TranscriptEntry {
    #[serde(rename = "type")]
    pub entry_type: String,
    pub uuid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_uuid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logical_parent_uuid: Option<String>,
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub timestamp: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,
    #[serde(default)]
    pub is_sidechain: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// The raw message payload (role + content).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<serde_json::Value>,
    /// Token usage for this entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TranscriptUsage>,
    /// Model used for this entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Estimated cost in USD for this entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// Catch-all for fields we don't model explicitly.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Metadata entries that live alongside transcript messages in the JSONL.
///
/// `type:` discriminator is kebab-case (`custom-title`, `last-prompt`,
/// `file-history-snapshot`, …) — these drive cross-system tooling that
/// keys on them; changing them would force re-indexing. Payload field
/// NAMES are Rust snake_case; field VALUES carry the same semantic
/// content (see [`TranscriptEntry`] doc).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum MetadataEntry {
    CustomTitle {
        session_id: String,
        custom_title: String,
    },
    Tag {
        session_id: String,
        tag: String,
    },
    LastPrompt {
        session_id: String,
        last_prompt: String,
    },
    Summary {
        leaf_uuid: String,
        summary: String,
    },
    CostSummary {
        session_id: String,
        total_input_tokens: i64,
        total_output_tokens: i64,
        total_cost_usd: f64,
        #[serde(default)]
        model_usage: std::collections::HashMap<String, ModelCostEntry>,
    },
    /// File-history snapshot recorded by the rewind subsystem. Replayed
    /// on resume by [`build_file_history_snapshot_chain`] to rebuild
    /// the rewind picker and the disk-backup mapping.
    /// The `snapshot` payload is a passthrough JSON blob to keep
    /// `coco-session` free of a `coco-context` dependency —
    /// `coco-context::FileHistorySnapshot` owns the typed shape and
    /// (de)serializes through this Value.
    FileHistorySnapshot {
        message_id: String,
        snapshot: serde_json::Value,
        #[serde(default)]
        is_snapshot_update: bool,
    },
    /// Staged context-collapse commit (TS `'marble-origami-commit'`).
    ///
    /// Persists one committed range so resume can replay the splice.
    /// `payload` is a passthrough JSON blob produced by
    /// `coco_compact::staged::CommitEntry` — keeping it untyped here
    /// avoids a `coco-session → coco-compact` dependency.
    #[serde(rename = "marble-origami-commit")]
    MarbleOrigamiCommit {
        #[serde(flatten)]
        payload: serde_json::Value,
    },
    /// Staged context-collapse snapshot (TS `'marble-origami-snapshot'`).
    /// Last-wins by `sessionId` on resume.
    #[serde(rename = "marble-origami-snapshot")]
    MarbleOrigamiSnapshot {
        #[serde(flatten)]
        payload: serde_json::Value,
    },
    /// Tool-result budget replacement record. Resume replays the
    /// exact persisted-output string for a `tool_use_id` so
    /// prompt-cache stability survives a restart.
    #[serde(rename = "content-replacement")]
    ContentReplacement {
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
        replacements: Vec<ContentReplacementRecord>,
    },

    /// AI-generated session title. Lower priority than `CustomTitle`
    /// (user override wins). Unlike `CustomTitle`, this variant is
    /// **not** re-appended to the tail on cleanup; the session picker
    /// still finds it via head-window scanning.
    AiTitle {
        session_id: String,
        ai_title: String,
    },

    /// Periodic fork-generated task snapshot. Payload is opaque JSON
    /// — the summary fork (agent layer) owns the shape.
    #[serde(rename = "task-summary")]
    TaskSummary {
        #[serde(flatten)]
        payload: serde_json::Value,
    },

    /// Custom name assigned to a swarm agent.
    AgentName {
        session_id: String,
        agent_name: String,
    },

    /// UI color for a swarm agent.
    AgentColor {
        session_id: String,
        agent_color: String,
    },

    /// Agent definition that this session uses.
    AgentSetting {
        session_id: String,
        agent_setting: String,
    },

    /// GitHub PR link recorded alongside a session. Payload is opaque
    /// JSON — the PR-link emitter (commands layer) owns the shape so
    /// new fields can land without breaking the read path.
    #[serde(rename = "pr-link")]
    PrLink {
        #[serde(flatten)]
        payload: serde_json::Value,
    },

    /// Claude character contributions per file. Payload is opaque
    /// JSON — only the LATEST one matters; read-side preserves all so
    /// resume can rebuild attribution state.
    #[serde(rename = "attribution-snapshot")]
    AttributionSnapshot {
        #[serde(flatten)]
        payload: serde_json::Value,
    },

    /// Persisted worktree session state — last-wins by session_id on
    /// resume. Payload is opaque JSON.
    #[serde(rename = "worktree-state")]
    WorktreeState {
        #[serde(flatten)]
        payload: serde_json::Value,
    },

    /// Session execution mode: `coordinator` vs `normal`.
    Mode {
        session_id: String,
        mode: String,
    },
}

/// One content-replacement record.
///
/// Three fields — `kind`, `tool_use_id`, `replacement`. Records are
/// keyed by `tool_use_id`, which is globally unique within a session.
/// Wire shape is snake_case, like the rest of the JSONL envelope.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ContentReplacementRecord {
    ToolResult {
        tool_use_id: String,
        replacement: String,
    },
}

impl ContentReplacementRecord {
    pub fn tool_result(tool_use_id: impl Into<String>, replacement: impl Into<String>) -> Self {
        Self::ToolResult {
            tool_use_id: tool_use_id.into(),
            replacement: replacement.into(),
        }
    }

    pub fn tool_use_id(&self) -> &str {
        match self {
            Self::ToolResult { tool_use_id, .. } => tool_use_id,
        }
    }

    pub fn replacement(&self) -> &str {
        match self {
            Self::ToolResult { replacement, .. } => replacement,
        }
    }
}

/// Per-model cost breakdown within a session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelCostEntry {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
    pub request_count: i32,
}

/// Options for appending a batch of conversation messages as one chain.
///
/// `starting_parent_uuid` is the already-written prefix parent. Once the
/// first new message is written, later already-written messages in `messages`
/// no longer advance the parent. This keeps compact-preserved suffixes from
/// reconnecting the new branch to the old tail.
#[derive(Debug, Clone, Default)]
pub struct ChainWriteOptions {
    pub cwd: String,
    pub timestamp: String,
    pub is_sidechain: bool,
    pub agent_id: Option<String>,
    pub starting_parent_uuid: Option<String>,
    /// Git branch for `cwd` at the time of the chain write. Resolved
    /// once by the caller and stamped on every transcript line.
    /// `None` ⇒ field is omitted.
    pub git_branch: Option<String>,
}

/// Result of appending a transcript chain.
#[derive(Debug, Clone, Default)]
pub struct ChainWriteResult {
    pub appended: usize,
    pub last_written_uuid: Option<Uuid>,
}

/// Union of all entry kinds that can appear in a JSONL transcript.
/// Deserialization tries transcript message first, then metadata.
//
// `TranscriptEntry` is ~344 bytes and dominates the enum size; box it so
// the metadata / unknown variants don't drag every `Vec<Entry>` allocation
// up to that footprint.
#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    Transcript(Box<TranscriptEntry>),
    Metadata(MetadataEntry),
    /// Unparseable line — kept so we never silently drop data.
    Unknown(serde_json::Value),
}

impl Serialize for Entry {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Entry::Transcript(t) => t.serialize(serializer),
            Entry::Metadata(m) => m.serialize(serializer),
            Entry::Unknown(v) => v.serialize(serializer),
        }
    }
}

// ---------------------------------------------------------------------------
// Transcript metadata (lightweight summary)
// ---------------------------------------------------------------------------

/// Lightweight metadata extracted from a transcript file without loading
/// every message. Surfaced by the resume picker (`--resume`).
/// Snake_case JSON like the rest of the wire envelope.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TranscriptMetadata {
    pub session_id: String,
    pub first_prompt: String,
    pub message_count: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_setting: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_state: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_link: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    pub is_sidechain: bool,
    pub created_at: String,
    pub modified_at: String,
    /// File size in bytes.
    pub file_size: u64,
}

// ---------------------------------------------------------------------------
// AgentMetadata
// ---------------------------------------------------------------------------

/// Per-agent metadata sidecar — written when a background AgentTool
/// spawn registers, read when the model invokes `agent/resume` to
/// rehydrate the spawn. Snake_case wire.
///
/// Persisted as `<sessions_dir>/<session_id>/subagents/agent-<id>.meta.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentMetadata {
    /// Agent type used at original spawn (e.g. `general-purpose`,
    /// `Explore`). Resume reads this to route correctly when
    /// `subagent_type` is omitted from the resume request.
    pub agent_type: String,
    /// Worktree path if the agent was spawned with `isolation:
    /// "worktree"`. Resume restores this as the cwd_override when
    /// the directory still exists; missing directory falls back to
    /// the parent's cwd.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<String>,
    /// Original task description from the AgentTool input. Resumed
    /// agent's notification surfaces this so the panel doesn't show
    /// a placeholder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

// ---------------------------------------------------------------------------
// TranscriptStore
// ---------------------------------------------------------------------------

/// Manages reading and writing JSONL session transcripts.
///
/// Path layout (see [`ProjectPaths`]):
///
/// ```text
/// <memory_base>/projects/<sanitize(cwd)>/<session_id>.jsonl                  ← transcript
/// <memory_base>/projects/<sanitize(cwd)>/<session_id>/subagents/             ← bg agents
/// <memory_base>/projects/<sanitize(cwd)>/<session_id>/tool-results/          ← persisted blobs
/// <memory_base>/projects/<sanitize(cwd)>/<session_id>/remote-agents/         ← CCR sidecars
/// ```
///
/// Construction takes a shared [`Arc<ProjectPaths>`] so every coco-rs
/// subsystem keyed on the same project (transcript store, memory
/// store, KAIROS daily log) computes paths from one source of truth.
pub struct TranscriptStore {
    paths: Arc<ProjectPaths>,
}

impl TranscriptStore {
    /// Build a store scoped to one project. The [`ProjectPaths`] facade
    /// owns the slug + NFC + djb2-hash math so this struct just
    /// delegates.
    pub fn new(paths: Arc<ProjectPaths>) -> Self {
        Self { paths }
    }

    /// The [`ProjectPaths`] this store is scoped to. Exposed so
    /// adjacent subsystems (memory, daily log) can pull paths off
    /// the same handle without re-deriving them.
    pub fn project_paths(&self) -> &Arc<ProjectPaths> {
        &self.paths
    }

    /// `<project>/<session_id>.jsonl` — session transcript JSONL.
    pub fn transcript_path(&self, session_id: &str) -> PathBuf {
        self.paths.transcript(session_id)
    }

    /// `<project>/<session_id>/subagents/agent-<agent_id>.jsonl` —
    /// background-agent transcript.
    pub fn agent_transcript_path(&self, session_id: &str, agent_id: &str) -> PathBuf {
        self.paths.agent_transcript(session_id, agent_id)
    }

    /// `<project>/<session_id>/subagents/agent-<agent_id>.meta.json` —
    /// metadata sidecar for a background agent's spawn.
    pub fn agent_metadata_path(&self, session_id: &str, agent_id: &str) -> PathBuf {
        self.paths.agent_metadata(session_id, agent_id)
    }

    /// `<project>/<session_id>/tool-results/` — persisted tool result
    /// blob directory.
    pub fn tool_results_session_dir(&self, session_id: &str) -> PathBuf {
        self.paths.tool_results_dir(session_id)
    }

    /// Remove stale files under every session's `tool-results/` artifact dir.
    ///
    /// Scoped to **this project** (`<project_dir>/{session_id}/tool-results/`).
    /// For cross-project cleanup invoke once per project — typically
    /// the TUI on shutdown calls it for the active project's
    /// [`TranscriptStore`].
    ///
    /// Direct files under
    /// `tool-results/` and one-level nested tool directories are unlinked when
    /// their file mtime is older than the retention cutoff; then empty tool,
    /// `tool-results`, and session artifact directories are removed best-effort.
    ///
    /// Returns the number of files removed.
    pub fn cleanup_tool_results_older_than(
        &self,
        older_than: std::time::Duration,
    ) -> crate::Result<i32> {
        let cutoff = std::time::SystemTime::now()
            .checked_sub(older_than)
            .ok_or(crate::SessionError::DurationOverflow)?;
        let project_dir = self.paths.project_dir();
        if !project_dir.exists() {
            return Ok(0);
        }

        let mut removed = 0;
        for entry in std::fs::read_dir(&project_dir)? {
            let Ok(entry) = entry else { continue };
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_dir() {
                continue;
            }

            let session_dir = entry.path();
            let tool_results_dir = session_dir.join("tool-results");
            let tool_entries = match std::fs::read_dir(&tool_results_dir) {
                Ok(entries) => entries,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    try_remove_empty_dir(&session_dir);
                    continue;
                }
                Err(e) => return Err(e.into()),
            };

            for tool_entry in tool_entries {
                let Ok(tool_entry) = tool_entry else { continue };
                let Ok(tool_file_type) = tool_entry.file_type() else {
                    continue;
                };
                let tool_path = tool_entry.path();
                if tool_file_type.is_file() {
                    if unlink_if_older_than(&tool_path, cutoff)? {
                        removed += 1;
                    }
                } else if tool_file_type.is_dir() {
                    let tool_files = match std::fs::read_dir(&tool_path) {
                        Ok(files) => files,
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                        Err(e) => return Err(e.into()),
                    };
                    for tool_file in tool_files {
                        let Ok(tool_file) = tool_file else { continue };
                        let Ok(tool_file_type) = tool_file.file_type() else {
                            continue;
                        };
                        if !tool_file_type.is_file() {
                            continue;
                        }
                        if unlink_if_older_than(&tool_file.path(), cutoff)? {
                            removed += 1;
                        }
                    }
                    try_remove_empty_dir(&tool_path);
                }
            }

            try_remove_empty_dir(&tool_results_dir);
            try_remove_empty_dir(&session_dir);
        }

        Ok(removed)
    }

    /// `<project>/<session_id>/` — the per-session artifact root.
    ///
    /// Tool-result helpers receive this root and append
    /// `tool-results/` themselves.
    pub fn session_artifact_dir(&self, session_id: &str) -> PathBuf {
        self.paths.session_dir(session_id)
    }

    /// `<project>/<session_id>/usage.json` — cumulative session usage snapshot.
    pub fn usage_snapshot_path(&self, session_id: &str) -> PathBuf {
        self.paths.session_dir(session_id).join("usage.json")
    }

    /// Atomically overwrite the cumulative usage snapshot for a session.
    pub fn write_usage_snapshot(
        &self,
        session_id: &str,
        snapshot: &coco_types::SessionUsageSnapshot,
    ) -> crate::Result<()> {
        let path = self.usage_snapshot_path(session_id);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or(0);
        let tmp = path.with_file_name(format!(
            "usage.json.{}.{}.{}.tmp",
            std::process::id(),
            timestamp_ms,
            Uuid::new_v4()
        ));
        let body = serde_json::to_vec_pretty(snapshot)?;
        {
            let mut file = std::fs::File::create(&tmp)?;
            file.write_all(&body)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
        }
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// Load a cumulative usage snapshot. Returns `Ok(None)` when absent.
    pub fn load_usage_snapshot(
        &self,
        session_id: &str,
    ) -> crate::Result<Option<coco_types::SessionUsageSnapshot>> {
        let path = self.usage_snapshot_path(session_id);
        match std::fs::read_to_string(&path) {
            Ok(body) => Ok(Some(serde_json::from_str(&body)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Append typed `Arc<Message>` entries to a background agent's
    /// per-spawn transcript, one JSON line each. Used by
    /// `coco_coordinator::agent_handle_spawn` on bg-spawn
    /// completion to persist the conversation history for resume.
    ///
    /// Serialises straight from `Message` to the JSONL byte stream —
    /// no `serde_json::Value` intermediate — so the disk-write path
    /// walks the message tree exactly once. Conversation order is
    /// preserved by append order (coco-rs doesn't need TS's
    /// parent_uuid chain because `MessageHistory.messages` is in
    /// conversation order; resume just reads back the Vec).
    pub fn append_agent_messages(
        &self,
        session_id: &str,
        agent_id: &str,
        messages: &[Arc<coco_messages::Message>],
    ) -> crate::Result<()> {
        if messages.is_empty() {
            return Ok(());
        }
        let path = self.agent_transcript_path(session_id, agent_id);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        for msg in messages {
            let line = serde_json::to_string(msg.as_ref())?;
            writeln!(file, "{line}")?;
        }
        Ok(())
    }

    /// Load every line of a background agent's per-spawn transcript
    /// into typed `Arc<Message>` in conversation order. Returns
    /// `Ok(None)` when the file doesn't exist (no prior spawn).
    /// Lines that fail to parse are dropped — resume is best-effort,
    /// a corrupted entry shouldn't take the whole spawn down.
    pub fn load_agent_messages(
        &self,
        session_id: &str,
        agent_id: &str,
    ) -> crate::Result<Option<Vec<Arc<coco_messages::Message>>>> {
        let path = self.agent_transcript_path(session_id, agent_id);
        if !path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&path)?;
        let mut out: Vec<Arc<coco_messages::Message>> = Vec::new();
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(m) = serde_json::from_str::<coco_messages::Message>(line) {
                out.push(Arc::new(m));
            }
        }
        Ok(Some(out))
    }

    /// Write an agent's metadata sidecar (`agent-<id>.meta.json`).
    pub fn write_agent_metadata(
        &self,
        session_id: &str,
        agent_id: &str,
        metadata: &AgentMetadata,
    ) -> crate::Result<()> {
        let path = self.agent_metadata_path(session_id, agent_id);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = serde_json::to_string(metadata)?;
        std::fs::write(&path, body)?;
        Ok(())
    }

    /// Read an agent's metadata sidecar; returns `Ok(None)` when
    /// the file doesn't exist.
    pub fn read_agent_metadata(
        &self,
        session_id: &str,
        agent_id: &str,
    ) -> crate::Result<Option<AgentMetadata>> {
        let path = self.agent_metadata_path(session_id, agent_id);
        if !path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&path)?;
        Ok(Some(serde_json::from_str(&content)?))
    }

    /// Append a single entry to the transcript file (creates dirs if needed).
    pub fn append_entry(&self, session_id: &str, entry: &Entry) -> crate::Result<()> {
        let path = self.transcript_path(session_id);
        append_entry_to_file(&path, entry)
    }

    /// Append a transcript message, auto-filling session-level fields.
    pub fn append_message(&self, session_id: &str, entry: &TranscriptEntry) -> crate::Result<()> {
        self.append_entry(session_id, &Entry::Transcript(Box::new(entry.clone())))
    }

    /// Append a metadata entry (custom-title, tag, last-prompt, summary).
    pub fn append_metadata(&self, session_id: &str, entry: &MetadataEntry) -> crate::Result<()> {
        self.append_entry(session_id, &Entry::Metadata(entry.clone()))
    }

    /// Append messages using TS-compatible wire conversion and prefix-only
    /// dedup semantics. Thin wrapper over the backend-independent
    /// [`build_message_chain_entries`] — it builds the entry list (and
    /// mutates `seen`) then writes each line to the JSONL.
    pub fn append_message_chain<'a, I>(
        &self,
        session_id: &str,
        messages: I,
        seen: &mut HashSet<Uuid>,
        options: ChainWriteOptions,
    ) -> crate::Result<ChainWriteResult>
    where
        I: IntoIterator<Item = &'a coco_messages::Message>,
    {
        let (entries, result) = build_message_chain_entries(session_id, messages, seen, &options);
        for entry in &entries {
            self.append_entry(session_id, entry)?;
        }
        Ok(result)
    }

    /// Persist a file-history snapshot to the JSONL transcript.
    ///
    /// `snapshot_json` is the `FileHistorySnapshot`'s serialized JSON
    /// shape (see `coco-context::FileHistorySnapshot`).
    /// `is_snapshot_update == true` means we're rewriting an existing
    /// snapshot in place (a `tracked_edit` update); resume's chain builder
    /// uses last-wins on (`message_id`, `is_snapshot_update`) ordering.
    pub fn insert_file_history_snapshot(
        &self,
        session_id: &str,
        message_id: &str,
        snapshot_json: serde_json::Value,
        is_snapshot_update: bool,
    ) -> crate::Result<()> {
        self.append_metadata(
            session_id,
            &MetadataEntry::FileHistorySnapshot {
                message_id: message_id.to_string(),
                snapshot: snapshot_json,
                is_snapshot_update,
            },
        )
    }

    /// Persist a marble-origami commit entry to the transcript.
    ///
    /// `payload` is the serialized [`coco_compact::staged::CommitEntry`]
    /// (snake_case).
    pub fn append_marble_origami_commit(
        &self,
        session_id: &str,
        payload: serde_json::Value,
    ) -> crate::Result<()> {
        self.append_metadata(session_id, &MetadataEntry::MarbleOrigamiCommit { payload })
    }

    /// Persist a marble-origami snapshot entry to the transcript
    /// (last-wins on resume).
    pub fn append_marble_origami_snapshot(
        &self,
        session_id: &str,
        payload: serde_json::Value,
    ) -> crate::Result<()> {
        self.append_metadata(
            session_id,
            &MetadataEntry::MarbleOrigamiSnapshot { payload },
        )
    }

    pub fn insert_content_replacement(
        &self,
        session_id: &str,
        agent_id: Option<&str>,
        records: &[ContentReplacementRecord],
    ) -> crate::Result<()> {
        if records.is_empty() {
            return Ok(());
        }
        self.append_metadata(
            session_id,
            &MetadataEntry::ContentReplacement {
                session_id: session_id.to_string(),
                agent_id: agent_id.map(str::to_string),
                replacements: records.to_vec(),
            },
        )
    }

    pub fn load_content_replacements(
        &self,
        session_id: &str,
    ) -> crate::Result<Vec<ContentReplacementRecord>> {
        let entries = self.load_entries(session_id)?;
        Ok(entries
            .into_iter()
            .filter_map(|e| match e {
                Entry::Metadata(MetadataEntry::ContentReplacement {
                    session_id: entry_session_id,
                    replacements,
                    ..
                }) if entry_session_id == session_id => Some(replacements),
                _ => None,
            })
            .flatten()
            .collect())
    }

    /// Load all content-replacement records for this `session_id`
    /// filtered by `agent_id` presence. Records are routed purely by
    /// `agent_id`-vs-no-`agent_id` — no per-message-uuid scope, because
    /// `tool_use_id` is globally unique within a session.
    pub fn load_content_replacements_for_chain(
        &self,
        session_id: &str,
        agent_id: Option<&str>,
    ) -> crate::Result<Vec<ContentReplacementRecord>> {
        let entries = self.load_entries(session_id)?;
        Ok(content_replacements_for_chain(
            &entries, session_id, agent_id,
        ))
    }

    /// Replay marble-origami entries on resume. Returns
    /// `(commits_in_order, last_snapshot_or_none)` filtered by
    /// `session_id` (snapshot last-wins). Each `serde_json::Value` is
    /// the original payload — caller deserializes back into
    /// `coco_compact::staged::{CommitEntry, SnapshotEntry}`.
    pub fn load_marble_origami_entries(
        &self,
        session_id: &str,
    ) -> crate::Result<(Vec<serde_json::Value>, Option<serde_json::Value>)> {
        let entries = self.load_entries(session_id)?;
        Ok(marble_origami_entries(&entries, session_id))
    }

    /// Replay file-history snapshots from the transcript JSONL in
    /// conversation-chain order, with `is_snapshot_update` semantics.
    ///
    /// `chain_message_uuids` is the resolved chain of message UUIDs in
    /// conversation order (typically from
    /// `coco_session::recovery::load_conversation_for_resume`).
    /// The chain walk drives lookup — disk append order is irrelevant
    /// once the messages map is built. Caller deserializes each Value
    /// into the typed snapshot (decoupled so `coco-session` doesn't
    /// depend on `coco-context`).
    pub fn load_file_history_snapshots_for_chain(
        &self,
        session_id: &str,
        chain_message_uuids: &[String],
    ) -> crate::Result<Vec<serde_json::Value>> {
        let entries = self.load_entries(session_id)?;
        Ok(build_file_history_snapshot_chain(
            &entries,
            chain_message_uuids,
        ))
    }

    /// Load all entries from a transcript file.
    ///
    /// Skips blank and malformed lines (logged as `Unknown`). Refuses to
    /// read files larger than [`MAX_TRANSCRIPT_READ_BYTES`] to prevent OOM.
    pub fn load_entries(&self, session_id: &str) -> crate::Result<Vec<Entry>> {
        let path = self.transcript_path(session_id);
        load_entries_from_file(&path)
    }

    /// Load only transcript messages (user/assistant/system/attachment),
    /// filtering out metadata and unknown entries.
    pub fn load_transcript_messages(
        &self,
        session_id: &str,
    ) -> crate::Result<Vec<TranscriptEntry>> {
        let entries = self.load_entries(session_id)?;
        Ok(entries
            .into_iter()
            .filter_map(|e| match e {
                Entry::Transcript(t) => Some(*t),
                _ => None,
            })
            .collect())
    }

    /// Extract lightweight metadata from a transcript without loading all
    /// messages. Reads the first and last few KB of the file.
    pub fn read_metadata(&self, session_id: &str) -> crate::Result<TranscriptMetadata> {
        let path = self.transcript_path(session_id);
        read_transcript_metadata(&path, session_id)
    }

    /// List sessions in **this project** only, newest first.
    ///
    /// Walks `<memory_base>/projects/<slug>/*.jsonl`. For cross-project
    /// enumeration (the resume picker), use
    /// [`list_all_sessions`].
    pub fn list_sessions(&self) -> crate::Result<Vec<TranscriptMetadata>> {
        list_transcript_sessions(&self.paths.project_dir())
    }

    /// List sessions in **this project**, excluding sidechain transcripts.
    pub fn list_main_sessions(&self) -> crate::Result<Vec<TranscriptMetadata>> {
        let all = self.list_sessions()?;
        Ok(all.into_iter().filter(|m| !m.is_sidechain).collect())
    }

    /// Check whether a transcript file exists for the given session.
    pub fn exists(&self, session_id: &str) -> bool {
        self.transcript_path(session_id).exists()
    }

    /// Delete a transcript file.
    pub fn delete(&self, session_id: &str) -> crate::Result<()> {
        let path = self.transcript_path(session_id);
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        Ok(())
    }
}

/// Build the transcript entries for a conversation chain with prefix-only
/// dedup, mutating `seen` exactly as the append path does. Pure: no IO.
///
/// Both [`TranscriptStore::append_message_chain`] (writes the entries to
/// the JSONL) and the in-memory backend ([`crate::store::InMemoryStore`])
/// drive off this, so chain / parent-uuid / dedup semantics stay
/// identical regardless of where the bytes land.
pub fn build_message_chain_entries<'a, I>(
    session_id: &str,
    messages: I,
    seen: &mut HashSet<Uuid>,
    options: &ChainWriteOptions,
) -> (Vec<Entry>, ChainWriteResult)
where
    I: IntoIterator<Item = &'a coco_messages::Message>,
{
    let mut prev_uuid = options.starting_parent_uuid.clone();
    let mut wrote_new = false;
    let mut result = ChainWriteResult::default();
    let mut out: Vec<Entry> = Vec::new();
    let mut source_by_tool_use: std::collections::HashMap<String, Uuid> =
        std::collections::HashMap::new();

    for msg in messages {
        let Some(uuid) = msg.uuid().copied() else {
            continue;
        };
        remember_assistant_tool_calls(msg, uuid, &mut source_by_tool_use);

        if seen.contains(&uuid) {
            if !wrote_new {
                prev_uuid = Some(uuid.to_string());
            }
            continue;
        }

        let logical_parent_uuid = prev_uuid.clone();
        let source_assistant_uuid = source_assistant_uuid_for_tool_result(msg)
            .or_else(|| tool_result_use_id(msg).and_then(|id| source_by_tool_use.get(id).copied()));
        let parent_uuid = if is_compact_boundary_message(msg) {
            None
        } else if let Some(source_uuid) = source_assistant_uuid {
            Some(source_uuid.to_string())
        } else {
            prev_uuid.clone()
        };
        let entries = transcript_entries_for_message(
            msg,
            TranscriptEntryOptions {
                session_id,
                cwd: &options.cwd,
                timestamp: &options.timestamp,
                parent_uuid: parent_uuid.as_deref(),
                logical_parent_uuid: logical_parent_uuid.as_deref(),
                is_sidechain: options.is_sidechain,
                agent_id: options.agent_id.as_deref(),
                git_branch: options.git_branch.as_deref(),
            },
        );
        if entries.is_empty() {
            continue;
        }

        result.appended += entries.len();
        out.extend(entries.into_iter().map(|e| Entry::Transcript(Box::new(e))));
        seen.insert(uuid);
        wrote_new = true;
        prev_uuid = Some(uuid.to_string());
        result.last_written_uuid = Some(uuid);
    }

    (out, result)
}

fn unlink_if_older_than(path: &Path, cutoff: std::time::SystemTime) -> crate::Result<bool> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into()),
    };
    let Ok(mtime) = metadata.modified() else {
        return Ok(false);
    };
    if mtime >= cutoff {
        return Ok(false);
    }
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

fn try_remove_empty_dir(path: &Path) {
    let _ = std::fs::remove_dir(path);
}

// ---------------------------------------------------------------------------
// File-level helpers
// ---------------------------------------------------------------------------

/// Walk a conversation chain and reconstruct the file-history snapshot
/// list to replay on resume:
///
/// 1. Index the metadata entries by their OUTER `message_id` (the
///    JSONL row's field), keeping the LAST entry per outer id.
/// 2. Walk `chain_message_uuids` in conversation order. For each id,
///    look up its corresponding snapshot. If absent → continue.
/// 3. `is_snapshot_update == false` → push a new snapshot and remember
///    its position by the INNER `snapshot.messageId` (not the outer
///    one — `trackEdit` writes update entries whose outer `messageId`
///    is the **current** turn while the inner `snapshot.messageId`
///    keeps the original snapshot's id).
/// 4. `is_snapshot_update == true` → if an entry exists for the inner
///    `snapshot.messageId`, overwrite it in place; otherwise treat as
///    a new append (`existingIndex === undefined` falls through to push).
pub fn build_file_history_snapshot_chain(
    entries: &[Entry],
    chain_message_uuids: &[String],
) -> Vec<serde_json::Value> {
    use std::collections::HashMap;
    // Step 1: build outer-messageId → (inner_message_id, snapshot,
    // is_snapshot_update). Later entries overwrite earlier ones for the
    // same outer id (last-write-wins).
    let mut by_outer: HashMap<&str, (String, &serde_json::Value, bool)> = HashMap::new();
    for entry in entries {
        let Entry::Metadata(MetadataEntry::FileHistorySnapshot {
            message_id,
            snapshot,
            is_snapshot_update,
        }) = entry
        else {
            continue;
        };
        let inner_message_id = snapshot
            .get("message_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(message_id.as_str())
            .to_string();
        by_outer.insert(
            message_id.as_str(),
            (inner_message_id, snapshot, *is_snapshot_update),
        );
    }

    let mut snapshots: Vec<serde_json::Value> = Vec::new();
    let mut index_by_inner: HashMap<String, usize> = HashMap::new();
    for chain_uuid in chain_message_uuids {
        let Some((inner_id, snapshot, is_update)) = by_outer.get(chain_uuid.as_str()) else {
            continue;
        };
        if *is_update && let Some(&idx) = index_by_inner.get(inner_id) {
            snapshots[idx] = (*snapshot).clone();
            continue;
        }
        index_by_inner.insert(inner_id.clone(), snapshots.len());
        snapshots.push((*snapshot).clone());
    }
    snapshots
}

/// Collect every `content-replacement` record for `(session_id,
/// agent_id)`. Records are routed by `agent_id` presence and applied
/// all on resume — no further per-message scope, because `tool_use_id`
/// is globally unique within a session.
pub fn content_replacements_for_chain(
    entries: &[Entry],
    session_id: &str,
    agent_id: Option<&str>,
) -> Vec<ContentReplacementRecord> {
    entries
        .iter()
        .filter_map(|entry| match entry {
            Entry::Metadata(MetadataEntry::ContentReplacement {
                session_id: entry_session_id,
                agent_id: entry_agent_id,
                replacements,
            }) if entry_session_id == session_id && entry_agent_id.as_deref() == agent_id => {
                Some(replacements)
            }
            _ => None,
        })
        .flat_map(|records| records.iter())
        .cloned()
        .collect()
}

/// Replay marble-origami entries for `session_id` from in-memory entries:
/// `(commits_in_order, last_snapshot_or_none)`, snapshot last-wins. Pure;
/// shared by the disk loader and the in-memory backend.
pub fn marble_origami_entries(
    entries: &[Entry],
    session_id: &str,
) -> (Vec<serde_json::Value>, Option<serde_json::Value>) {
    let mut commits = Vec::new();
    let mut last_snapshot: Option<serde_json::Value> = None;
    for e in entries {
        let Entry::Metadata(meta) = e else { continue };
        match meta {
            MetadataEntry::MarbleOrigamiCommit { payload } => {
                if matches_session(payload, session_id) {
                    commits.push(payload.clone());
                }
            }
            MetadataEntry::MarbleOrigamiSnapshot { payload } => {
                if matches_session(payload, session_id) {
                    last_snapshot = Some(payload.clone());
                }
            }
            _ => {}
        }
    }
    (commits, last_snapshot)
}

/// Whether a marble-origami payload's session id field equals `session_id`.
/// Snake_case to match the rest of the coco-rs wire envelope.
fn matches_session(payload: &serde_json::Value, session_id: &str) -> bool {
    payload
        .get("session_id")
        .and_then(|v| v.as_str())
        .is_some_and(|s| s == session_id)
}

/// Append a single JSON entry as one JSONL line.
fn append_entry_to_file(path: &Path, entry: &Entry) -> crate::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    let line = serde_json::to_string(entry)?;
    writeln!(file, "{line}")?;
    Ok(())
}

/// Load and parse all JSONL entries from a file.
fn load_entries_from_file(path: &Path) -> crate::Result<Vec<Entry>> {
    if !path.exists() {
        return Err(crate::SessionError::TranscriptNotFound {
            path: path.to_path_buf(),
        });
    }

    let meta = std::fs::metadata(path)?;
    if meta.len() > MAX_TRANSCRIPT_READ_BYTES {
        return Err(crate::SessionError::generic(format!(
            "transcript file too large ({} bytes, max {MAX_TRANSCRIPT_READ_BYTES}): {}",
            meta.len(),
            path.display(),
        )));
    }

    let file = std::fs::File::open(path)?;
    let reader = std::io::BufReader::new(file);
    let mut entries = Vec::new();

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        entries.push(parse_entry(&line));
    }

    Ok(entries)
}

/// Parse a single JSONL line into an [`Entry`].
///
/// Dispatch order: parse once into `serde_json::Value`, then route by
/// the `type` field. Transcript types
/// (`user`/`assistant`/`system`/`attachment`) go to `TranscriptEntry`;
/// every other `type` value is attempted as a `MetadataEntry`; anything
/// that fails to deserialize lands as `Entry::Unknown` with a
/// `tracing::debug!` so the failure shows up in logs instead of being
/// silently swallowed.
fn parse_entry(line: &str) -> Entry {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        tracing::debug!(line = %line, "transcript line was not valid json");
        return Entry::Unknown(serde_json::Value::String(line.to_string()));
    };
    let entry_type = value
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let looks_like_transcript = matches!(
        entry_type,
        entry_kind::USER | entry_kind::ASSISTANT | entry_kind::SYSTEM | entry_kind::ATTACHMENT
    );
    if looks_like_transcript {
        if let Ok(transcript) = serde_json::from_value::<TranscriptEntry>(value.clone()) {
            return Entry::Transcript(Box::new(transcript));
        }
    } else if let Ok(meta) = serde_json::from_value::<MetadataEntry>(value.clone()) {
        return Entry::Metadata(meta);
    }
    tracing::debug!(
        entry_type = %entry_type,
        "transcript line did not match any known Entry shape — preserving as Unknown",
    );
    Entry::Unknown(value)
}

/// Lite-read window: when a transcript exceeds this size, scan only
/// the first and last `LITE_READ_WINDOW` bytes rather than loading
/// the whole file (64 KB on each end). Session-picker metadata
/// (`first_prompt`, `custom_title`, `tag`, `last_prompt`, `git_branch`,
/// `cwd`) lives at the top of the transcript, while re-appended-on-exit
/// values land near the tail; 64 KB at each end captures both without
/// streaming the multi-megabyte body.
const LITE_READ_WINDOW: u64 = 64 * 1024;

/// Read lightweight metadata from a transcript file without loading all
/// messages. Scans the first and last portion of the file.
/// Public alias of the per-file lite-metadata reader so
/// `SessionManager::load` can derive a `Session` from a resolved
/// transcript path without re-walking the projects tree.
pub fn read_transcript_metadata_at(
    path: &Path,
    session_id: &str,
) -> crate::Result<TranscriptMetadata> {
    read_transcript_metadata(path, session_id)
}

fn read_transcript_metadata(path: &Path, session_id: &str) -> crate::Result<TranscriptMetadata> {
    if !path.exists() {
        return Err(crate::SessionError::TranscriptNotFound {
            path: path.to_path_buf(),
        });
    }

    let file_meta = std::fs::metadata(path)?;
    let file_size = file_meta.len();

    let created_at = file_meta
        .created()
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .to_string();

    let modified_at = file_meta
        .modified()
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .to_string();

    // For small files (≤ 2× the lite window) load everything; for
    // larger transcripts scan only the head and tail. The head pass
    // captures `first_prompt`, `cwd`, `git_branch`, and the
    // sidechain/message-count signal; the tail pass picks up the
    // metadata entries (`custom-title`, `tag`, `last-prompt`) that
    // are re-appended on exit so they survive head-truncation.
    let content = if file_size > LITE_READ_WINDOW * 2 {
        read_head_and_tail(path, LITE_READ_WINDOW)?
    } else {
        std::fs::read_to_string(path)?
    };
    let entries: Vec<Entry> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(parse_entry)
        .collect();

    // Content-derived fields come from the shared fold; the fs-stat trio
    // (created/modified/file_size) is layered on top here — those are the
    // disk-specific bits a non-fs backend cannot supply from entries alone.
    let mut meta = fold_transcript_metadata(&entries, session_id);
    meta.created_at = created_at;
    meta.modified_at = modified_at;
    meta.file_size = file_size;
    Ok(meta)
}

/// Derive the **content** fields of [`TranscriptMetadata`] from in-memory
/// entries — everything except the fs-stat trio
/// (`created_at`/`modified_at`/`file_size`), which is left at its default
/// for the caller to stamp. Pure: no IO. Shared by the disk head/tail
/// reader and any non-fs backend ([`crate::store::InMemoryStore`]) that
/// already holds the entries in memory.
pub fn fold_transcript_metadata(entries: &[Entry], session_id: &str) -> TranscriptMetadata {
    let mut first_prompt = String::new();
    let mut custom_title: Option<String> = None;
    let mut ai_title: Option<String> = None;
    let mut agent_name: Option<String> = None;
    let mut agent_color: Option<String> = None;
    let mut agent_setting: Option<String> = None;
    let mut mode: Option<String> = None;
    let mut worktree_state: Option<serde_json::Value> = None;
    let mut pr_link: Option<serde_json::Value> = None;
    let mut tag: Option<String> = None;
    let mut last_prompt: Option<String> = None;
    let mut git_branch: Option<String> = None;
    let mut cwd: Option<String> = None;
    let mut is_sidechain = false;
    let mut message_count: i32 = 0;

    for entry in entries {
        match entry {
            Entry::Transcript(t) => {
                if t.entry_type == entry_kind::USER || t.entry_type == entry_kind::ASSISTANT {
                    message_count += 1;
                }
                if first_prompt.is_empty() && t.entry_type == entry_kind::USER {
                    let candidate = extract_text_content(t);
                    // Filters synthetic interrupt markers so the resume
                    // picker shows the user's real first prompt, not
                    // "[Request interrupted by user]". Uses literal
                    // equality against the interrupt markers from
                    // `coco-messages::creation`.
                    if !is_synthetic_first_prompt_candidate(&candidate) {
                        first_prompt = candidate;
                    }
                }
                if t.is_sidechain {
                    is_sidechain = true;
                }
                if cwd.is_none() && !t.cwd.is_empty() {
                    cwd = Some(t.cwd.clone());
                }
                if t.git_branch.is_some() {
                    git_branch.clone_from(&t.git_branch);
                }
            }
            Entry::Metadata(m) => match m {
                MetadataEntry::CustomTitle {
                    custom_title: ct, ..
                } => {
                    custom_title = Some(ct.clone());
                }
                MetadataEntry::Tag { tag: t, .. } => {
                    tag = Some(t.clone());
                }
                MetadataEntry::LastPrompt {
                    last_prompt: lp, ..
                } => {
                    last_prompt = Some(lp.clone());
                }
                MetadataEntry::AiTitle {
                    ai_title: title, ..
                } => {
                    ai_title = Some(title.clone());
                }
                MetadataEntry::AgentName {
                    agent_name: name, ..
                } => {
                    agent_name = Some(name.clone());
                }
                MetadataEntry::AgentColor {
                    session_id: entry_session_id,
                    agent_color: color,
                } if entry_session_id == session_id => {
                    agent_color = Some(color.clone());
                }
                MetadataEntry::AgentSetting {
                    session_id: entry_session_id,
                    agent_setting: setting,
                } if entry_session_id == session_id => {
                    agent_setting = Some(setting.clone());
                }
                MetadataEntry::Mode {
                    session_id: entry_session_id,
                    mode: m,
                } if entry_session_id == session_id => {
                    mode = Some(m.clone());
                }
                MetadataEntry::WorktreeState { payload }
                    if metadata_payload_session_id(payload).as_deref() == Some(session_id) =>
                {
                    worktree_state = Some(payload.clone());
                }
                MetadataEntry::PrLink { payload }
                    if metadata_payload_session_id(payload).as_deref() == Some(session_id) =>
                {
                    pr_link = Some(payload.clone());
                }
                MetadataEntry::Summary { .. }
                | MetadataEntry::CostSummary { .. }
                | MetadataEntry::FileHistorySnapshot { .. }
                | MetadataEntry::MarbleOrigamiCommit { .. }
                | MetadataEntry::MarbleOrigamiSnapshot { .. }
                | MetadataEntry::ContentReplacement { .. }
                | MetadataEntry::TaskSummary { .. }
                | MetadataEntry::AttributionSnapshot { .. }
                | MetadataEntry::AgentColor { .. }
                | MetadataEntry::AgentSetting { .. }
                | MetadataEntry::PrLink { .. }
                | MetadataEntry::WorktreeState { .. }
                | MetadataEntry::Mode { .. } => {}
            },
            Entry::Unknown(_) => {}
        }
    }

    TranscriptMetadata {
        session_id: session_id.to_string(),
        first_prompt,
        message_count,
        custom_title,
        ai_title,
        agent_name,
        agent_color,
        agent_setting,
        mode,
        worktree_state,
        pr_link,
        tag,
        last_prompt,
        git_branch,
        cwd,
        is_sidechain,
        created_at: String::new(),
        modified_at: String::new(),
        file_size: 0,
    }
}

fn metadata_payload_session_id(payload: &serde_json::Value) -> Option<String> {
    payload
        .get("session_id")
        .or_else(|| payload.get("sessionId"))
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string)
}

/// Read the first `window` bytes and the last `window` bytes of
/// `path`, joining them with a single newline. Drops any partial
/// JSONL lines at the seams (the byte right after the head window
/// and the byte at the start of the tail window may sit mid-record)
/// so the caller's `parse_entry` loop only sees complete lines.
fn read_head_and_tail(path: &Path, window: u64) -> crate::Result<String> {
    use std::io::Read;
    use std::io::Seek;
    use std::io::SeekFrom;

    let mut file = std::fs::File::open(path)?;
    let total = file.metadata()?.len();

    let head_len = window.min(total);
    let mut head_buf = vec![0u8; head_len as usize];
    file.read_exact(&mut head_buf)?;
    // Truncate at the last newline so we don't carry a partial
    // record into `parse_entry` (which would surface as `Unknown`).
    if let Some(idx) = find_last_newline(&head_buf) {
        head_buf.truncate(idx);
    }

    let tail_len = window.min(total.saturating_sub(head_len));
    let mut tail_buf = vec![0u8; tail_len as usize];
    if tail_len > 0 {
        file.seek(SeekFrom::End(-(tail_len as i64)))?;
        file.read_exact(&mut tail_buf)?;
        // Skip leading partial line (everything up to the first '\n').
        if let Some(idx) = tail_buf.iter().position(|b| *b == b'\n') {
            tail_buf.drain(..=idx);
        } else {
            // No newline in the tail window — every byte belongs to a
            // single oversized line; drop it.
            tail_buf.clear();
        }
    }

    let mut combined = Vec::with_capacity(head_buf.len() + 1 + tail_buf.len());
    combined.extend_from_slice(&head_buf);
    combined.push(b'\n');
    combined.extend_from_slice(&tail_buf);
    String::from_utf8(combined)
        .map_err(|e| crate::SessionError::generic(format!("transcript not utf-8: {e}")))
}

/// Index of the rightmost newline byte in `buf`, or `None` when no
/// newline is present. Used by [`read_head_and_tail`] to drop partial
/// records at the head/tail seams.
fn find_last_newline(buf: &[u8]) -> Option<usize> {
    buf.iter().rposition(|b| *b == b'\n')
}

/// List all transcript sessions from a directory, newest first.
fn list_transcript_sessions(sessions_dir: &Path) -> crate::Result<Vec<TranscriptMetadata>> {
    if !sessions_dir.exists() {
        return Ok(Vec::new());
    }

    let mut results = Vec::new();
    for entry in std::fs::read_dir(sessions_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "jsonl") {
            let session_id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            if session_id.is_empty() {
                continue;
            }
            match read_transcript_metadata(&path, &session_id) {
                Ok(meta) => results.push(meta),
                Err(_) => {
                    // Skip corrupt / unreadable files.
                    continue;
                }
            }
        }
    }

    // Newest first by modified_at (descending). Parse numerically so
    // mixed-width millisecond timestamps still compare correctly.
    results.sort_by(|a, b| {
        let a_ms = a.modified_at.parse::<u128>().unwrap_or(0);
        let b_ms = b.modified_at.parse::<u128>().unwrap_or(0);
        b_ms.cmp(&a_ms)
    });
    Ok(results)
}

// ---------------------------------------------------------------------------
// Cross-project enumeration & worktree-aware lookup
// ---------------------------------------------------------------------------

/// Result of [`resolve_session_file_path`] — the transcript file
/// found plus the project path (or worktree path) it lives under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSessionFile {
    /// Absolute path to `<session_id>.jsonl`.
    pub file_path: PathBuf,
    /// The project root associated with the file. For a direct
    /// match this is `cwd_hint`; for a worktree fallback it's the
    /// sibling worktree path; for the global scan branch
    /// (`cwd_hint == None`) this is `None`.
    pub project_path: Option<PathBuf>,
}

/// Locate the transcript file for `session_id`.
///
/// Resolution order:
/// 1. **Direct project lookup**: if `cwd_hint` is `Some`, compute
///    `ProjectPaths` from that exact cwd and check `<project_dir>/<sid>.jsonl`.
/// 2. **Worktree fallback**: if step 1 missed, shell out to
///    `git worktree list --porcelain` from `cwd_hint`, slug each
///    sibling worktree, and probe each one.
/// 3. **Global scan**: when `cwd_hint` is `None`, walk
///    `<memory_base>/projects/*/` and return the first project that
///    contains the transcript. Used by SDK callers without a cwd.
///
/// Returns `Ok(None)` when no project has the file. I/O errors at
/// the `read_dir(<projects>)` level propagate; transient stat
/// failures on individual entries are tolerated.
pub fn resolve_session_file_path(
    memory_base: &Path,
    session_id: &str,
    cwd_hint: Option<&Path>,
) -> crate::Result<Option<ResolvedSessionFile>> {
    let filename = format!("{session_id}.jsonl");

    if let Some(cwd) = cwd_hint {
        // 1. Direct lookup at the slug for this cwd.
        let paths = ProjectPaths::new(memory_base.to_path_buf(), cwd);
        let candidate = paths.project_dir().join(&filename);
        if has_nonzero_file(&candidate) {
            return Ok(Some(ResolvedSessionFile {
                file_path: candidate,
                project_path: Some(cwd.to_path_buf()),
            }));
        }

        // 2. Worktree fallback — only fires when (a) direct miss
        //    and (b) git knows about other worktrees.
        for wt in coco_git::worktree_paths(cwd) {
            if wt == cwd {
                continue;
            }
            let wt_paths = ProjectPaths::new(memory_base.to_path_buf(), &wt);
            let cand = wt_paths.project_dir().join(&filename);
            if has_nonzero_file(&cand) {
                return Ok(Some(ResolvedSessionFile {
                    file_path: cand,
                    project_path: Some(wt),
                }));
            }
        }
        return Ok(None);
    }

    // 3. Global scan — walk every project directory.
    let projects_root = coco_paths::projects_root(memory_base);
    let entries = match std::fs::read_dir(&projects_root) {
        Ok(it) => it,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    for entry in entries.flatten() {
        let candidate = entry.path().join(&filename);
        if has_nonzero_file(&candidate) {
            return Ok(Some(ResolvedSessionFile {
                file_path: candidate,
                project_path: None,
            }));
        }
    }
    Ok(None)
}

/// List every session transcript across **every** project under
/// `<memory_base>/projects/*/`, newest first.
///
/// Used by the resume picker / SDK session enumerator — callers
/// that only want this-project sessions should go through
/// [`TranscriptStore::list_sessions`] instead.
pub fn list_all_sessions(memory_base: &Path) -> crate::Result<Vec<TranscriptMetadata>> {
    let projects_root = coco_paths::projects_root(memory_base);
    let project_entries = match std::fs::read_dir(&projects_root) {
        Ok(it) => it,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };

    let mut results: Vec<TranscriptMetadata> = Vec::new();
    for project_entry in project_entries.flatten() {
        let project_dir = project_entry.path();
        if !project_dir.is_dir() {
            continue;
        }
        // Each project dir has the same internal layout — reuse
        // the same per-dir walker as `TranscriptStore::list_sessions`.
        if let Ok(mut found) = list_transcript_sessions(&project_dir) {
            results.append(&mut found);
        }
    }

    // Sort across all projects so newest wins overall.
    results.sort_by(|a, b| {
        let a_ms = a.modified_at.parse::<u128>().unwrap_or(0);
        let b_ms = b.modified_at.parse::<u128>().unwrap_or(0);
        b_ms.cmp(&a_ms)
    });
    Ok(results)
}

fn has_nonzero_file(path: &Path) -> bool {
    matches!(
        std::fs::metadata(path),
        Ok(m) if m.is_file() && m.len() > 0,
    )
}

#[cfg(test)]
#[path = "storage.test.rs"]
mod tests;
