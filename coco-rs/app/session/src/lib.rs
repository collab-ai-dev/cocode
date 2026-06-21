//! Session persistence, history, and state aggregation.

pub mod concurrent_sessions;
pub mod error;
pub mod history;
pub mod recovery;
pub mod storage;
pub mod store;
pub mod title_generator;

pub use concurrent_sessions::SessionKind;
pub use concurrent_sessions::SessionRegistration;
pub use concurrent_sessions::SessionRegistry;
pub use concurrent_sessions::SessionStatus;
pub use concurrent_sessions::count_concurrent_sessions;
pub use concurrent_sessions::is_bg_session;
pub use concurrent_sessions::read_registration as read_session_registration;
pub use error::SessionError;
pub use history::HistoryEntry;
pub use history::PromptHistory;
pub use storage::AgentMetadata;
pub use storage::ContentReplacementRecord;
pub use storage::Entry;
pub use storage::MetadataEntry;
pub use storage::ModelCostEntry;
pub use storage::TranscriptEntry;
pub use storage::TranscriptMetadata;
pub use storage::TranscriptStore;
pub use storage::TranscriptUsage;
pub use storage::build_file_history_snapshot_chain;
pub use store::AgentTranscriptStore;
pub use store::DiskCatalog;
pub use store::InMemoryCatalog;
pub use store::InMemoryStore;
pub use store::ResolvedSession;
pub use store::SessionCatalog;
pub use store::SessionStore;
pub use store::TranscriptIo;
pub use store::UsageSnapshotStore;
pub use store::catalog_for_backend;

use serde::Deserialize;
use serde::Serialize;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

/// Crate-local Result alias. Default error type is `SessionError` but the
/// generic stays open so `Result::ok` / 2-arg `Result<T, E>` callsites
/// still resolve against `std::result::Result`.
pub type Result<T, E = SessionError> = std::result::Result<T, E>;

/// A session record.
///
/// **Derived value** in the JSONL-canonical model: this struct is
/// reconstructed from the on-disk transcript (first/last lines + tag
/// / custom-title metadata entries), not persisted as its own file.
/// There is no `{session_id}.json` sidecar — every session-level fact
/// (title, tags, model, created/updated_at, message counts) is
/// derivable from the transcript's first entry + trailing metadata block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    pub model: String,
    pub working_dir: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Number of messages in the session.
    #[serde(default)]
    pub message_count: i32,
    /// Total tokens used.
    #[serde(default)]
    pub total_tokens: i64,
    /// Searchable tags applied via `/tag`. Persisted alongside title for
    /// session browsing/filtering.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

impl Session {
    /// Build a `Session` from a [`storage::TranscriptMetadata`] —
    /// the lite metadata view the session picker uses.
    fn from_transcript_metadata(meta: storage::TranscriptMetadata) -> Self {
        let working_dir = meta.cwd.as_deref().map(PathBuf::from).unwrap_or_default();
        let title = meta.custom_title.clone().or(meta.ai_title.clone());
        Session {
            id: meta.session_id,
            created_at: meta.created_at,
            updated_at: Some(meta.modified_at),
            // model is filled by a deeper scan if needed; the lite
            // metadata view doesn't carry it. The session picker
            // displays the title / first_prompt instead.
            model: String::new(),
            working_dir,
            title,
            message_count: meta.message_count,
            total_tokens: 0,
            tags: meta.tag.map(|t| vec![t]).unwrap_or_default(),
        }
    }
}

/// Session manager — every operation reads/writes the
/// JSONL transcript and its metadata entries. There is no
/// `{session_id}.json` sidecar (pre-fix coco-rs had one, and the
/// duplication produced silent state drift between the sidecar and
/// the source-of-truth transcript on every `/rename`, `/tag`, or
/// interrupted save).
///
/// `memory_base` is `coco_config::global_config::config_home()`
/// unless overridden by `COCO_REMOTE_MEMORY_DIR`. The manager spans
/// every project under `<memory_base>/projects/*/` — operations
/// keyed by session id walk projects to locate the transcript.
pub struct SessionManager {
    memory_base: PathBuf,
    catalog: Arc<dyn SessionCatalog>,
}

impl SessionManager {
    /// Build a session manager rooted at `memory_base` (typically
    /// `coco_config::global_config::config_home()`). Always disk-backed —
    /// use [`with_backend`](Self::with_backend) to honor a configured
    /// `session.backend`.
    pub fn new(memory_base: PathBuf) -> Self {
        let catalog = Arc::new(DiskCatalog::new(memory_base.clone()));
        Self {
            memory_base,
            catalog,
        }
    }

    /// Build a session manager for the configured backend. The selection
    /// is the resolved `RuntimeConfig` field `session.backend` — callers
    /// pass `runtime_config.settings.merged.session.backend`.
    pub fn with_backend(backend: coco_config::SessionBackend, memory_base: PathBuf) -> Self {
        let catalog = store::catalog_for_backend(backend, memory_base.clone());
        Self {
            memory_base,
            catalog,
        }
    }

    /// Inject a custom backend catalog (e.g. a future remote / tee store).
    /// `memory_base` is retained only for the disk-specific maintenance
    /// helper (`cleanup_older_than`) that has no backend-agnostic form yet.
    pub fn with_catalog(memory_base: PathBuf, catalog: Arc<dyn SessionCatalog>) -> Self {
        Self {
            memory_base,
            catalog,
        }
    }

    /// A store scoped to `cwd`'s project, from the configured backend.
    /// The per-turn engine sources its transcript store here so it shares
    /// the manager's backend (and, for non-disk backends, its state).
    pub fn store_for(&self, cwd: &Path) -> Arc<dyn SessionStore> {
        self.catalog.store_for(cwd)
    }

    /// Create a new in-memory session. **Does not write to disk** —
    /// the transcript JSONL is created lazily by the first
    /// `append_message` call from the runtime.
    pub fn create(&self, model: &str, cwd: &Path) -> crate::Result<Session> {
        Ok(Session {
            id: uuid::Uuid::new_v4().to_string(),
            created_at: timestamp_now(),
            updated_at: None,
            model: model.to_string(),
            working_dir: cwd.to_path_buf(),
            title: None,
            message_count: 0,
            total_tokens: 0,
            tags: Vec::new(),
        })
    }

    /// Set the session title (`/rename <name>`). Appends **two**
    /// metadata entries — `CustomTitle` (the picker's preferred
    /// field) and `AgentName` (the prompt-bar agent banner +
    /// `coco ps`-visible name) — in that order.
    ///
    /// Errors when no transcript exists for `id` under any project.
    pub fn set_title(&self, id: &str, title: &str) -> crate::Result<Session> {
        let mut session = self.load(id)?;
        let store = self.store_for(&session.working_dir);
        store.append_metadata(
            id,
            &storage::MetadataEntry::CustomTitle {
                session_id: id.to_string(),
                custom_title: title.to_string(),
            },
        )?;
        // `AgentName` is appended immediately after `CustomTitle`. The
        // reader side (storage::read_transcript_metadata) has the
        // AgentName arm wired; both are always written together.
        store.append_metadata(
            id,
            &storage::MetadataEntry::AgentName {
                session_id: id.to_string(),
                agent_name: title.to_string(),
            },
        )?;
        session.title = Some(title.to_string());
        session.updated_at = Some(timestamp_now());
        Ok(session)
    }

    /// Persist an AI-generated title as a distinct `AiTitle` metadata
    /// entry. Reader precedence in
    /// [`storage::read_transcript_metadata`] is `CustomTitle >
    /// AiTitle`, so a subsequent user [`set_title`] always wins on
    /// the next read regardless of write order.
    ///
    /// Unlike [`set_title`], this method does NOT touch the
    /// `AgentName` slot — AI-suggested titles are advisory and
    /// shouldn't claim the prompt-bar agent banner from the user.
    pub fn set_ai_title(&self, id: &str, title: &str) -> crate::Result<Session> {
        let mut session = self.load(id)?;
        let store = self.store_for(&session.working_dir);
        let meta = store.read_metadata(id)?;
        store.append_metadata(
            id,
            &storage::MetadataEntry::AiTitle {
                session_id: id.to_string(),
                ai_title: title.to_string(),
            },
        )?;
        if meta.custom_title.is_none() {
            session.title = Some(title.to_string());
        }
        session.updated_at = Some(timestamp_now());
        Ok(session)
    }

    /// Toggle a tag on/off (`/tag <name>`). Tag presence is decided
    /// from the current Session derive; the new state is appended
    /// as a `Tag` metadata entry. Re-running `/tag X` adds and then
    /// removes X — the new desired state is appended and the picker's
    /// tail-window scan picks up the latest.
    pub fn toggle_tag(&self, id: &str, tag: &str) -> crate::Result<(Session, bool)> {
        let mut session = self.load(id)?;
        let store = self.store_for(&session.working_dir);
        let added = if let Some(idx) = session.tags.iter().position(|t| t == tag) {
            session.tags.remove(idx);
            false
        } else {
            session.tags.push(tag.to_string());
            true
        };
        // Build the tag set serialised as a single `Tag` metadata
        // entry — TS reads only the most recent so we collapse on
        // write. Empty set still writes (an empty Tag effectively
        // clears the picker).
        store.append_metadata(
            id,
            &storage::MetadataEntry::Tag {
                session_id: id.to_string(),
                tag: session.tags.join(","),
            },
        )?;
        session.updated_at = Some(timestamp_now());
        Ok((session, added))
    }

    /// Backwards-compatibility shim. The JSONL-canonical model has
    /// no separate "save" step — title/tag mutations are persisted
    /// inline via [`set_title`] / [`toggle_tag`], and the JSONL
    /// owns every other field. This method now no-ops so existing
    /// callers don't need rewriting.
    pub fn save(&self, _session: &Session) -> crate::Result<()> {
        Ok(())
    }

    /// Load a session by id by locating its transcript under
    /// `<memory_base>/projects/*/{id}.jsonl` (via global scan) and
    /// deriving the lite metadata view.
    pub fn load(&self, id: &str) -> crate::Result<Session> {
        let Some(meta) = self.catalog.read_metadata(id, None)? else {
            return Err(SessionError::TranscriptNotFound {
                path: coco_paths::projects_root(&self.memory_base).join(format!("*/{id}.jsonl")),
            });
        };
        Ok(Session::from_transcript_metadata(meta))
    }

    /// Resume a session — equivalent to [`load`] in the
    /// JSONL-canonical model. Pre-fix this also bumped a sidecar
    /// `updated_at` field; the JSONL's own mtime now serves the
    /// same purpose.
    pub fn resume(&self, id: &str) -> crate::Result<Session> {
        self.load(id)
    }

    /// List every session across every project, newest first.
    pub fn list(&self) -> crate::Result<Vec<Session>> {
        let metas = self.catalog.list_all()?;
        Ok(metas
            .into_iter()
            .map(Session::from_transcript_metadata)
            .collect())
    }

    /// Snapshot the session's coordinator-mode state into the transcript
    /// (`"coordinator"` / `"normal"`) so a later `--resume` can re-derive
    /// it. The reader keeps the last-wins `Mode` entry
    /// ([`storage::read_transcript_metadata`]), which drives
    /// `coco_cli::coordinator_mode_resume::reconcile_on_resume`.
    ///
    /// Errors when no transcript exists for `id` (a zero-turn session has
    /// nothing to record); callers treat the write as best-effort.
    pub fn save_mode(&self, id: &str, mode: &str) -> crate::Result<()> {
        let session = self.load(id)?;
        let store = self.store_for(&session.working_dir);
        store.append_metadata(
            id,
            &storage::MetadataEntry::Mode {
                session_id: id.to_string(),
                mode: mode.to_string(),
            },
        )
    }

    /// Re-append session metadata to EOF so the lite-metadata tail scan in
    /// [`storage::read_transcript_metadata`] keeps finding it after
    /// large post-compaction content.
    ///
    /// **Algorithm**:
    /// 1. Read the current lite metadata (head+tail scan).
    /// 2. For each non-empty slot, append a fresh metadata entry at EOF.
    ///
    /// Re-append is **unconditional** even when the value is already
    /// near EOF: a title 40KB from EOF sits inside the current tail
    /// window but the next compaction round will push it out. Skipping
    /// would defeat the purpose.
    ///
    /// Silent no-op when no transcript exists (e.g. session created
    /// in-memory but no message appended yet) — the empty case has
    /// nothing to preserve. Other IO failures bubble up.
    ///
    /// **Discipline**: write `CustomTitle` separately from `AgentName`
    /// (paired, like [`set_title`]). `AiTitle` is deliberately not
    /// re-appended — AI titles are lower-priority and are not refreshed
    /// to EOF.
    pub fn re_append_session_metadata(&self, id: &str) -> crate::Result<()> {
        let Some(meta) = self.catalog.read_metadata(id, None)? else {
            return Ok(());
        };
        let working_dir = meta.cwd.as_deref().map(PathBuf::from).unwrap_or_default();
        let store = self.store_for(&working_dir);

        if let Some(last_prompt) = meta
            .last_prompt
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            store.append_metadata(
                id,
                &storage::MetadataEntry::LastPrompt {
                    session_id: id.to_string(),
                    last_prompt: last_prompt.to_string(),
                },
            )?;
        }
        if let Some(title) = meta
            .custom_title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            store.append_metadata(
                id,
                &storage::MetadataEntry::CustomTitle {
                    session_id: id.to_string(),
                    custom_title: title.to_string(),
                },
            )?;
        }
        if let Some(tag) = meta.tag.as_deref().filter(|t| !t.is_empty()) {
            store.append_metadata(
                id,
                &storage::MetadataEntry::Tag {
                    session_id: id.to_string(),
                    tag: tag.to_string(),
                },
            )?;
        }
        if let Some(name) = meta
            .agent_name
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            store.append_metadata(
                id,
                &storage::MetadataEntry::AgentName {
                    session_id: id.to_string(),
                    agent_name: name.to_string(),
                },
            )?;
        }
        if let Some(color) = meta
            .agent_color
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            store.append_metadata(
                id,
                &storage::MetadataEntry::AgentColor {
                    session_id: id.to_string(),
                    agent_color: color.to_string(),
                },
            )?;
        }
        if let Some(setting) = meta
            .agent_setting
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            store.append_metadata(
                id,
                &storage::MetadataEntry::AgentSetting {
                    session_id: id.to_string(),
                    agent_setting: setting.to_string(),
                },
            )?;
        }
        if let Some(mode) = meta
            .mode
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            store.append_metadata(
                id,
                &storage::MetadataEntry::Mode {
                    session_id: id.to_string(),
                    mode: mode.to_string(),
                },
            )?;
        }
        if let Some(payload) = meta.worktree_state {
            store.append_metadata(id, &storage::MetadataEntry::WorktreeState { payload })?;
        }
        if let Some(payload) = meta.pr_link {
            store.append_metadata(id, &storage::MetadataEntry::PrLink { payload })?;
        }
        Ok(())
    }

    /// Find every session whose title matches `query`, newest first.
    ///
    /// Match semantics: case-insensitive substring (`exact = false`)
    /// or case-insensitive equality (`exact = true`). Empty / `None`
    /// titles never match.
    ///
    /// Caller is responsible for any project-scope filtering
    /// (`tui_runner`'s resume path rejects cross-project matches);
    /// here we walk every project for worktree-aware search. Newest-first
    /// sort comes from [`list`] which is mtime sorted.
    pub fn find_by_title(&self, query: &str, exact: bool) -> crate::Result<Vec<Session>> {
        let needle = query.to_lowercase();
        let needle = needle.trim();
        if needle.is_empty() {
            return Ok(Vec::new());
        }
        Ok(self
            .catalog
            .list_all()?
            .into_iter()
            .filter(|meta| match meta.custom_title.as_deref() {
                Some(t) => {
                    let t = t.to_lowercase();
                    let t = t.trim();
                    if exact {
                        t == needle
                    } else {
                        t.contains(needle)
                    }
                }
                None => false,
            })
            .map(Session::from_transcript_metadata)
            .collect())
    }

    /// Delete a session — removes its transcript JSONL. The
    /// session subdirectory (`<project>/<id>/`) is left intact;
    /// the retention sweep handles its eventual collection.
    pub fn delete(&self, id: &str) -> crate::Result<()> {
        self.catalog.delete(id, None)
    }

    /// Most-recent session across every project (= the first entry
    /// of [`list`]).
    pub fn most_recent(&self) -> crate::Result<Option<Session>> {
        Ok(self.list()?.into_iter().next())
    }

    /// Keep the `keep_count` most-recent sessions, delete the rest.
    pub fn cleanup(&self, keep_count: usize) -> crate::Result<i32> {
        let sessions = self.list()?;
        let mut removed = 0;
        for session in sessions.iter().skip(keep_count) {
            self.delete(&session.id)?;
            removed += 1;
        }
        Ok(removed)
    }

    /// Mtime-based retention: delete every transcript
    /// `.jsonl` whose on-disk mtime is older than `older_than`
    /// (`DEFAULT_CLEANUP_PERIOD_DAYS = 30`).
    ///
    /// Walks `<memory_base>/projects/*/*.jsonl` stat-only — a
    /// corrupt transcript doesn't prevent cleanup.
    pub fn cleanup_older_than(&self, older_than: std::time::Duration) -> crate::Result<i32> {
        let cutoff = std::time::SystemTime::now()
            .checked_sub(older_than)
            .ok_or(SessionError::DurationOverflow)?;
        let projects_root = coco_paths::projects_root(&self.memory_base);
        let project_entries = match std::fs::read_dir(&projects_root) {
            Ok(it) => it,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(e.into()),
        };
        let mut removed = 0;
        for project in project_entries.flatten() {
            let project_dir = project.path();
            if !project_dir.is_dir() {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&project_dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_none_or(|e| e != "jsonl") {
                    continue;
                }
                let Ok(meta) = entry.metadata() else { continue };
                let Ok(mtime) = meta.modified() else { continue };
                if mtime >= cutoff {
                    continue;
                }
                match std::fs::remove_file(&path) {
                    Ok(()) => removed += 1,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.into()),
                }
            }
        }
        Ok(removed)
    }
}

/// Default cleanup retention period (30 days).
pub const DEFAULT_CLEANUP_PERIOD_DAYS: u64 = 30;

/// [`DEFAULT_CLEANUP_PERIOD_DAYS`] as a `Duration` (convenience).
pub const fn default_cleanup_period() -> std::time::Duration {
    std::time::Duration::from_secs(DEFAULT_CLEANUP_PERIOD_DAYS * 24 * 60 * 60)
}

/// Wall-clock now as a unix-millisecond string.
///
/// Must match the unit `storage::read_transcript_metadata` emits for
/// `created_at` / `modified_at` (`as_millis().to_string()`). Mixed
/// units silently corrupt `list_all_sessions`'s newest-first sort
/// since `parse::<u128>()` compares scaled-different numbers as if
/// they were the same scale.
pub fn timestamp_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}", now.as_millis())
}

#[cfg(test)]
#[path = "lib.test.rs"]
mod tests;
