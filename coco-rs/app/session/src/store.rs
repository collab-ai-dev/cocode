//! Backend-agnostic session-store traits.
//!
//! The boundary that lets session persistence run against something
//! other than the local disk later (a DB / HTTP / object-store
//! `RemoteStore`, or a `TeeStore` write-mirror) without touching
//! consumers or the pure domain logic — mirroring the existing
//! [`coco_permissions::PermissionStore`] / `ScheduleStore` idiom.
//!
//! This iteration ships **only** the on-disk implementation: every
//! trait here is implemented by [`crate::storage::TranscriptStore`]
//! (transcript IO) and [`DiskCatalog`] (cross-project resolve/list),
//! both wrapping today's `std::fs` logic verbatim. The method shapes
//! that only matter for a non-fs backend (the `SessionSummary` /
//! `StorageStat` metadata split, `&[Entry]`-based resume) keep their
//! current disk-shaped form and change when `RemoteStore` lands —
//! back-compat is not a constraint.
//!
//! See `docs/coco-rs/session-storage-backend-design.md`.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use coco_messages::Message;
use coco_paths::ProjectPaths;
use coco_types::SessionUsageSnapshot;
use serde_json::Value;
use uuid::Uuid;

use crate::storage::AgentMetadata;
use crate::storage::ChainWriteOptions;
use crate::storage::ChainWriteResult;
use crate::storage::ContentReplacementRecord;
use crate::storage::Entry;
use crate::storage::MetadataEntry;
use crate::storage::TranscriptEntry;
use crate::storage::TranscriptMetadata;
use crate::storage::TranscriptStore;

/// Per-project transcript IO — the hot path. Object-safe, synchronous,
/// `Send + Sync`. A remote backend bridges async internally (durable-on-
/// return appends; the rare reads `block_on` a dedicated handle).
pub trait TranscriptIo: Send + Sync {
    fn append_entry(&self, session_id: &str, entry: &Entry) -> crate::Result<()>;
    fn append_message(&self, session_id: &str, entry: &TranscriptEntry) -> crate::Result<()>;
    fn append_metadata(&self, session_id: &str, entry: &MetadataEntry) -> crate::Result<()>;

    /// Append a conversation chain with prefix-only dedup. Object-safe
    /// `&[&Message]` form of the inherent generic
    /// [`TranscriptStore::append_message_chain`]; callers crossing the
    /// `dyn` boundary collect into a slice first.
    fn append_message_chain(
        &self,
        session_id: &str,
        messages: &[&Message],
        seen: &mut HashSet<Uuid>,
        options: ChainWriteOptions,
    ) -> crate::Result<ChainWriteResult>;

    fn insert_file_history_snapshot(
        &self,
        session_id: &str,
        message_id: &str,
        snapshot_json: Value,
        is_snapshot_update: bool,
    ) -> crate::Result<()>;
    fn insert_content_replacement(
        &self,
        session_id: &str,
        agent_id: Option<&str>,
        records: &[ContentReplacementRecord],
    ) -> crate::Result<()>;
    fn append_marble_origami_commit(&self, session_id: &str, payload: Value) -> crate::Result<()>;
    fn append_marble_origami_snapshot(&self, session_id: &str, payload: Value)
    -> crate::Result<()>;

    fn load_entries(&self, session_id: &str) -> crate::Result<Vec<Entry>>;
    fn load_transcript_messages(&self, session_id: &str) -> crate::Result<Vec<TranscriptEntry>>;
    fn load_content_replacements(
        &self,
        session_id: &str,
    ) -> crate::Result<Vec<ContentReplacementRecord>>;
    fn load_content_replacements_for_chain(
        &self,
        session_id: &str,
        agent_id: Option<&str>,
    ) -> crate::Result<Vec<ContentReplacementRecord>>;
    fn load_marble_origami_entries(
        &self,
        session_id: &str,
    ) -> crate::Result<(Vec<Value>, Option<Value>)>;
    fn load_file_history_snapshots_for_chain(
        &self,
        session_id: &str,
        chain_message_uuids: &[String],
    ) -> crate::Result<Vec<Value>>;

    /// Lightweight session metadata. Disk overrides with a head/tail
    /// window; a non-fs backend will split this into a derived
    /// `SessionSummary` + backend `StorageStat` (deferred).
    fn read_metadata(&self, session_id: &str) -> crate::Result<TranscriptMetadata>;
    fn list_sessions(&self) -> crate::Result<Vec<TranscriptMetadata>>;
    fn list_main_sessions(&self) -> crate::Result<Vec<TranscriptMetadata>>;
    fn exists(&self, session_id: &str) -> bool;
    fn delete(&self, session_id: &str) -> crate::Result<()>;

    /// Local-only disk artifact dir for tool-result blobs (the §3.4
    /// decision keeps blobs unremoted). `None` on non-fs backends, which
    /// disables local blob persistence there — full-fidelity re-fetch
    /// degrades, by design.
    fn session_artifact_dir(&self, _session_id: &str) -> Option<PathBuf> {
        None
    }
    /// Resolved on-disk transcript path, if the backend is file-based;
    /// `None` otherwise. The prompt layer hands this to tools.
    fn transcript_path(&self, _session_id: &str) -> Option<PathBuf> {
        None
    }
}

/// Subagent (background / fork) transcripts — typed on `Message`, not
/// `Entry`. Subsumes the cli-side async `AgentTranscriptStore` wrapper,
/// which becomes a thin `spawn_blocking` adapter over this.
pub trait AgentTranscriptStore: Send + Sync {
    fn append_agent_messages(
        &self,
        session_id: &str,
        agent_id: &str,
        messages: &[Arc<Message>],
    ) -> crate::Result<()>;
    fn load_agent_messages(
        &self,
        session_id: &str,
        agent_id: &str,
    ) -> crate::Result<Option<Vec<Arc<Message>>>>;
    fn write_agent_metadata(
        &self,
        session_id: &str,
        agent_id: &str,
        metadata: &AgentMetadata,
    ) -> crate::Result<()>;
    fn read_agent_metadata(
        &self,
        session_id: &str,
        agent_id: &str,
    ) -> crate::Result<Option<AgentMetadata>>;
}

/// Cumulative per-session usage snapshot.
pub trait UsageSnapshotStore: Send + Sync {
    fn write_usage_snapshot(
        &self,
        session_id: &str,
        snapshot: &SessionUsageSnapshot,
    ) -> crate::Result<()>;
    fn load_usage_snapshot(&self, session_id: &str) -> crate::Result<Option<SessionUsageSnapshot>>;
}

/// Everything a consumer holding one store handle needs. Combined via a
/// blanket impl so `Arc<dyn SessionStore>` is one object while the
/// focused sub-traits stay available for narrower consumers / backends
/// (ISP without tripping over Rust's no-`dyn A + B` rule).
pub trait SessionStore: TranscriptIo + AgentTranscriptStore + UsageSnapshotStore {}
impl<T: TranscriptIo + AgentTranscriptStore + UsageSnapshotStore> SessionStore for T {}

/// A located session. Disk-only this iteration: `transcript_path` is the
/// resolved `<sid>.jsonl`. A non-fs backend will replace the path with a
/// logical handle when `RemoteStore` lands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSession {
    pub session_id: String,
    /// Project root the session was found under (worktree origin for
    /// disk); `None` for the global-scan branch / non-fs backends.
    pub project: Option<PathBuf>,
    pub transcript_path: PathBuf,
}

/// Cross-project catalog + per-project store factory. `SessionManager`
/// delegates cross-project lookups here.
pub trait SessionCatalog: Send + Sync {
    /// A store scoped to `cwd`'s project. Cheap for disk (`ProjectPaths`);
    /// a remote backend must pool connections rather than reconnect.
    fn store_for(&self, cwd: &Path) -> Arc<dyn SessionStore>;
    /// Locate a session by id (worktree-aware for disk; PK lookup for a DB).
    fn resolve(
        &self,
        session_id: &str,
        cwd_hint: Option<&Path>,
    ) -> crate::Result<Option<ResolvedSession>>;
    /// Every session across every project, newest first.
    fn list_all(&self) -> crate::Result<Vec<TranscriptMetadata>>;

    /// Lightweight metadata for a session located by id, or `None` when
    /// it doesn't exist. The backend-agnostic replacement for consumers
    /// reaching into [`ResolvedSession::transcript_path`] + the free
    /// `read_transcript_metadata_at` — disk resolves then head/tail-scans;
    /// a non-fs backend reads its own store.
    fn read_metadata(
        &self,
        session_id: &str,
        cwd_hint: Option<&Path>,
    ) -> crate::Result<Option<TranscriptMetadata>>;

    /// Delete a session's transcript. Idempotent — deleting an absent
    /// session is `Ok(())`.
    fn delete(&self, session_id: &str, cwd_hint: Option<&Path>) -> crate::Result<()>;
}

// ---------------------------------------------------------------------------
// Disk implementations — delegate to today's `std::fs` logic verbatim.
// ---------------------------------------------------------------------------

impl TranscriptIo for TranscriptStore {
    fn append_entry(&self, session_id: &str, entry: &Entry) -> crate::Result<()> {
        TranscriptStore::append_entry(self, session_id, entry)
    }
    fn append_message(&self, session_id: &str, entry: &TranscriptEntry) -> crate::Result<()> {
        TranscriptStore::append_message(self, session_id, entry)
    }
    fn append_metadata(&self, session_id: &str, entry: &MetadataEntry) -> crate::Result<()> {
        TranscriptStore::append_metadata(self, session_id, entry)
    }
    fn append_message_chain(
        &self,
        session_id: &str,
        messages: &[&Message],
        seen: &mut HashSet<Uuid>,
        options: ChainWriteOptions,
    ) -> crate::Result<ChainWriteResult> {
        // Fully-qualified call hits the inherent generic method (not this
        // trait method) — no recursion.
        TranscriptStore::append_message_chain(
            self,
            session_id,
            messages.iter().copied(),
            seen,
            options,
        )
    }
    fn insert_file_history_snapshot(
        &self,
        session_id: &str,
        message_id: &str,
        snapshot_json: Value,
        is_snapshot_update: bool,
    ) -> crate::Result<()> {
        TranscriptStore::insert_file_history_snapshot(
            self,
            session_id,
            message_id,
            snapshot_json,
            is_snapshot_update,
        )
    }
    fn insert_content_replacement(
        &self,
        session_id: &str,
        agent_id: Option<&str>,
        records: &[ContentReplacementRecord],
    ) -> crate::Result<()> {
        TranscriptStore::insert_content_replacement(self, session_id, agent_id, records)
    }
    fn append_marble_origami_commit(&self, session_id: &str, payload: Value) -> crate::Result<()> {
        TranscriptStore::append_marble_origami_commit(self, session_id, payload)
    }
    fn append_marble_origami_snapshot(
        &self,
        session_id: &str,
        payload: Value,
    ) -> crate::Result<()> {
        TranscriptStore::append_marble_origami_snapshot(self, session_id, payload)
    }
    fn load_entries(&self, session_id: &str) -> crate::Result<Vec<Entry>> {
        TranscriptStore::load_entries(self, session_id)
    }
    fn load_transcript_messages(&self, session_id: &str) -> crate::Result<Vec<TranscriptEntry>> {
        TranscriptStore::load_transcript_messages(self, session_id)
    }
    fn load_content_replacements(
        &self,
        session_id: &str,
    ) -> crate::Result<Vec<ContentReplacementRecord>> {
        TranscriptStore::load_content_replacements(self, session_id)
    }
    fn load_content_replacements_for_chain(
        &self,
        session_id: &str,
        agent_id: Option<&str>,
    ) -> crate::Result<Vec<ContentReplacementRecord>> {
        TranscriptStore::load_content_replacements_for_chain(self, session_id, agent_id)
    }
    fn load_marble_origami_entries(
        &self,
        session_id: &str,
    ) -> crate::Result<(Vec<Value>, Option<Value>)> {
        TranscriptStore::load_marble_origami_entries(self, session_id)
    }
    fn load_file_history_snapshots_for_chain(
        &self,
        session_id: &str,
        chain_message_uuids: &[String],
    ) -> crate::Result<Vec<Value>> {
        TranscriptStore::load_file_history_snapshots_for_chain(
            self,
            session_id,
            chain_message_uuids,
        )
    }
    fn read_metadata(&self, session_id: &str) -> crate::Result<TranscriptMetadata> {
        TranscriptStore::read_metadata(self, session_id)
    }
    fn list_sessions(&self) -> crate::Result<Vec<TranscriptMetadata>> {
        TranscriptStore::list_sessions(self)
    }
    fn list_main_sessions(&self) -> crate::Result<Vec<TranscriptMetadata>> {
        TranscriptStore::list_main_sessions(self)
    }
    fn exists(&self, session_id: &str) -> bool {
        TranscriptStore::exists(self, session_id)
    }
    fn delete(&self, session_id: &str) -> crate::Result<()> {
        TranscriptStore::delete(self, session_id)
    }
    fn session_artifact_dir(&self, session_id: &str) -> Option<PathBuf> {
        Some(TranscriptStore::session_artifact_dir(self, session_id))
    }
    fn transcript_path(&self, session_id: &str) -> Option<PathBuf> {
        Some(TranscriptStore::transcript_path(self, session_id))
    }
}

impl AgentTranscriptStore for TranscriptStore {
    fn append_agent_messages(
        &self,
        session_id: &str,
        agent_id: &str,
        messages: &[Arc<Message>],
    ) -> crate::Result<()> {
        TranscriptStore::append_agent_messages(self, session_id, agent_id, messages)
    }
    fn load_agent_messages(
        &self,
        session_id: &str,
        agent_id: &str,
    ) -> crate::Result<Option<Vec<Arc<Message>>>> {
        TranscriptStore::load_agent_messages(self, session_id, agent_id)
    }
    fn write_agent_metadata(
        &self,
        session_id: &str,
        agent_id: &str,
        metadata: &AgentMetadata,
    ) -> crate::Result<()> {
        TranscriptStore::write_agent_metadata(self, session_id, agent_id, metadata)
    }
    fn read_agent_metadata(
        &self,
        session_id: &str,
        agent_id: &str,
    ) -> crate::Result<Option<AgentMetadata>> {
        TranscriptStore::read_agent_metadata(self, session_id, agent_id)
    }
}

impl UsageSnapshotStore for TranscriptStore {
    fn write_usage_snapshot(
        &self,
        session_id: &str,
        snapshot: &SessionUsageSnapshot,
    ) -> crate::Result<()> {
        TranscriptStore::write_usage_snapshot(self, session_id, snapshot)
    }
    fn load_usage_snapshot(&self, session_id: &str) -> crate::Result<Option<SessionUsageSnapshot>> {
        TranscriptStore::load_usage_snapshot(self, session_id)
    }
}

/// Disk-backed [`SessionCatalog`]. Spans `<memory_base>/projects/*/`,
/// resolving the same way [`SessionManager`](crate::SessionManager) does.
pub struct DiskCatalog {
    memory_base: PathBuf,
}

impl DiskCatalog {
    pub fn new(memory_base: PathBuf) -> Self {
        Self { memory_base }
    }
}

impl SessionCatalog for DiskCatalog {
    fn store_for(&self, cwd: &Path) -> Arc<dyn SessionStore> {
        Arc::new(TranscriptStore::new(Arc::new(ProjectPaths::new(
            self.memory_base.clone(),
            cwd,
        ))))
    }

    fn resolve(
        &self,
        session_id: &str,
        cwd_hint: Option<&Path>,
    ) -> crate::Result<Option<ResolvedSession>> {
        Ok(
            crate::storage::resolve_session_file_path(&self.memory_base, session_id, cwd_hint)?
                .map(|r| ResolvedSession {
                    session_id: session_id.to_string(),
                    project: r.project_path,
                    transcript_path: r.file_path,
                }),
        )
    }

    fn list_all(&self) -> crate::Result<Vec<TranscriptMetadata>> {
        crate::storage::list_all_sessions(&self.memory_base)
    }

    fn read_metadata(
        &self,
        session_id: &str,
        cwd_hint: Option<&Path>,
    ) -> crate::Result<Option<TranscriptMetadata>> {
        match self.resolve(session_id, cwd_hint)? {
            Some(resolved) => Ok(Some(crate::storage::read_transcript_metadata_at(
                &resolved.transcript_path,
                session_id,
            )?)),
            None => Ok(None),
        }
    }

    fn delete(&self, session_id: &str, cwd_hint: Option<&Path>) -> crate::Result<()> {
        let Some(resolved) = self.resolve(session_id, cwd_hint)? else {
            return Ok(());
        };
        match std::fs::remove_file(&resolved.transcript_path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

/// Select the session-store backend from the resolved config. The single
/// site the `SessionBackend` enum is matched — both construction points
/// (`SessionManager` and each per-turn engine's transcript store) route
/// here so they always agree on the live backend.
pub fn catalog_for_backend(
    backend: coco_config::SessionBackend,
    memory_base: PathBuf,
) -> Arc<dyn SessionCatalog> {
    match backend {
        coco_config::SessionBackend::Disk => Arc::new(DiskCatalog::new(memory_base)),
        coco_config::SessionBackend::Memory => Arc::new(InMemoryCatalog::new()),
    }
}

// ---------------------------------------------------------------------------
// In-memory backend — pure RAM, no disk, no async.
// ---------------------------------------------------------------------------

/// One session's in-memory transcript plus the timestamps the disk
/// backend reads off `std::fs::metadata`.
#[derive(Default)]
struct MemorySession {
    entries: Vec<Entry>,
    created_at_ms: u128,
    modified_at_ms: u128,
}

#[derive(Default)]
struct InMemoryState {
    sessions: HashMap<String, MemorySession>,
    /// `(session_id, agent_id)` → subagent transcript.
    agent_messages: HashMap<(String, String), Vec<Arc<Message>>>,
    agent_metadata: HashMap<(String, String), AgentMetadata>,
    usage: HashMap<String, SessionUsageSnapshot>,
}

/// Process-lifetime, non-persistent [`SessionStore`] — every trait method
/// is a `Mutex`-guarded map op, no disk and no async. Backs
/// [`coco_config::SessionBackend::Memory`] (ephemeral sessions) and is the
/// heterogeneous backend `store.test.rs` swaps in to prove the store
/// traits aren't carved to the on-disk JSONL shape.
///
/// Chain dedup and metadata derivation reuse the exact same pure helpers
/// as the disk store ([`crate::storage::build_message_chain_entries`] /
/// [`crate::storage::fold_transcript_metadata`]), so the content-derived
/// semantics match — only the fs-stat trio (`created`/`modified`/byte
/// size) differs, since RAM has no file metadata.
#[derive(Default)]
pub struct InMemoryStore {
    state: Mutex<InMemoryState>,
}

impl InMemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, InMemoryState> {
        // Recover the guard on poison: our ops never leave the maps in a
        // broken invariant, so a panic elsewhere shouldn't wedge the store.
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn now_ms() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    }

    /// Push pre-built entries onto a session, creating it (and stamping
    /// `created_at`) on first touch and bumping `modified_at`.
    fn push_entries(&self, session_id: &str, entries: impl IntoIterator<Item = Entry>) {
        let now = Self::now_ms();
        let mut state = self.lock();
        let session = state
            .sessions
            .entry(session_id.to_string())
            .or_insert_with(|| MemorySession {
                entries: Vec::new(),
                created_at_ms: now,
                modified_at_ms: now,
            });
        session.entries.extend(entries);
        session.modified_at_ms = now;
    }

    /// Derive [`TranscriptMetadata`] for one in-memory session, stamping
    /// the tracked timestamps over the content-derived fold (byte size is
    /// always 0 — RAM has none).
    fn metadata_for(session_id: &str, session: &MemorySession) -> TranscriptMetadata {
        let mut meta = crate::storage::fold_transcript_metadata(&session.entries, session_id);
        meta.created_at = session.created_at_ms.to_string();
        meta.modified_at = session.modified_at_ms.to_string();
        meta.file_size = 0;
        meta
    }
}

impl TranscriptIo for InMemoryStore {
    fn append_entry(&self, session_id: &str, entry: &Entry) -> crate::Result<()> {
        self.push_entries(session_id, [entry.clone()]);
        Ok(())
    }

    fn append_message(&self, session_id: &str, entry: &TranscriptEntry) -> crate::Result<()> {
        self.append_entry(session_id, &Entry::Transcript(Box::new(entry.clone())))
    }

    fn append_metadata(&self, session_id: &str, entry: &MetadataEntry) -> crate::Result<()> {
        self.append_entry(session_id, &Entry::Metadata(entry.clone()))
    }

    fn append_message_chain(
        &self,
        session_id: &str,
        messages: &[&Message],
        seen: &mut HashSet<Uuid>,
        options: ChainWriteOptions,
    ) -> crate::Result<ChainWriteResult> {
        let (entries, result) = crate::storage::build_message_chain_entries(
            session_id,
            messages.iter().copied(),
            seen,
            &options,
        );
        if !entries.is_empty() {
            self.push_entries(session_id, entries);
        }
        Ok(result)
    }

    fn insert_file_history_snapshot(
        &self,
        session_id: &str,
        message_id: &str,
        snapshot_json: Value,
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

    fn insert_content_replacement(
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

    fn append_marble_origami_commit(&self, session_id: &str, payload: Value) -> crate::Result<()> {
        self.append_metadata(session_id, &MetadataEntry::MarbleOrigamiCommit { payload })
    }

    fn append_marble_origami_snapshot(
        &self,
        session_id: &str,
        payload: Value,
    ) -> crate::Result<()> {
        self.append_metadata(
            session_id,
            &MetadataEntry::MarbleOrigamiSnapshot { payload },
        )
    }

    fn load_entries(&self, session_id: &str) -> crate::Result<Vec<Entry>> {
        Ok(self
            .lock()
            .sessions
            .get(session_id)
            .map(|s| s.entries.clone())
            .unwrap_or_default())
    }

    fn load_transcript_messages(&self, session_id: &str) -> crate::Result<Vec<TranscriptEntry>> {
        Ok(self
            .load_entries(session_id)?
            .into_iter()
            .filter_map(|e| match e {
                Entry::Transcript(t) => Some(*t),
                _ => None,
            })
            .collect())
    }

    fn load_content_replacements(
        &self,
        session_id: &str,
    ) -> crate::Result<Vec<ContentReplacementRecord>> {
        Ok(self
            .load_entries(session_id)?
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

    fn load_content_replacements_for_chain(
        &self,
        session_id: &str,
        agent_id: Option<&str>,
    ) -> crate::Result<Vec<ContentReplacementRecord>> {
        let entries = self.load_entries(session_id)?;
        Ok(crate::storage::content_replacements_for_chain(
            &entries, session_id, agent_id,
        ))
    }

    fn load_marble_origami_entries(
        &self,
        session_id: &str,
    ) -> crate::Result<(Vec<Value>, Option<Value>)> {
        let entries = self.load_entries(session_id)?;
        Ok(crate::storage::marble_origami_entries(&entries, session_id))
    }

    fn load_file_history_snapshots_for_chain(
        &self,
        session_id: &str,
        chain_message_uuids: &[String],
    ) -> crate::Result<Vec<Value>> {
        let entries = self.load_entries(session_id)?;
        Ok(crate::storage::build_file_history_snapshot_chain(
            &entries,
            chain_message_uuids,
        ))
    }

    fn read_metadata(&self, session_id: &str) -> crate::Result<TranscriptMetadata> {
        let state = self.lock();
        let Some(session) = state.sessions.get(session_id) else {
            return Err(crate::SessionError::TranscriptNotFound {
                path: PathBuf::from(format!("memory://{session_id}")),
            });
        };
        Ok(Self::metadata_for(session_id, session))
    }

    fn list_sessions(&self) -> crate::Result<Vec<TranscriptMetadata>> {
        let state = self.lock();
        let mut out: Vec<TranscriptMetadata> = state
            .sessions
            .iter()
            .map(|(id, session)| Self::metadata_for(id, session))
            .collect();
        // Newest first by modified_at, mirroring the disk enumerator.
        out.sort_by(|a, b| {
            let a_ms = a.modified_at.parse::<u128>().unwrap_or(0);
            let b_ms = b.modified_at.parse::<u128>().unwrap_or(0);
            b_ms.cmp(&a_ms)
        });
        Ok(out)
    }

    fn list_main_sessions(&self) -> crate::Result<Vec<TranscriptMetadata>> {
        Ok(self
            .list_sessions()?
            .into_iter()
            .filter(|m| !m.is_sidechain)
            .collect())
    }

    fn exists(&self, session_id: &str) -> bool {
        self.lock().sessions.contains_key(session_id)
    }

    fn delete(&self, session_id: &str) -> crate::Result<()> {
        self.lock().sessions.remove(session_id);
        Ok(())
    }
}

impl AgentTranscriptStore for InMemoryStore {
    fn append_agent_messages(
        &self,
        session_id: &str,
        agent_id: &str,
        messages: &[Arc<Message>],
    ) -> crate::Result<()> {
        if messages.is_empty() {
            return Ok(());
        }
        self.lock()
            .agent_messages
            .entry((session_id.to_string(), agent_id.to_string()))
            .or_default()
            .extend(messages.iter().cloned());
        Ok(())
    }

    fn load_agent_messages(
        &self,
        session_id: &str,
        agent_id: &str,
    ) -> crate::Result<Option<Vec<Arc<Message>>>> {
        Ok(self
            .lock()
            .agent_messages
            .get(&(session_id.to_string(), agent_id.to_string()))
            .cloned())
    }

    fn write_agent_metadata(
        &self,
        session_id: &str,
        agent_id: &str,
        metadata: &AgentMetadata,
    ) -> crate::Result<()> {
        self.lock().agent_metadata.insert(
            (session_id.to_string(), agent_id.to_string()),
            metadata.clone(),
        );
        Ok(())
    }

    fn read_agent_metadata(
        &self,
        session_id: &str,
        agent_id: &str,
    ) -> crate::Result<Option<AgentMetadata>> {
        Ok(self
            .lock()
            .agent_metadata
            .get(&(session_id.to_string(), agent_id.to_string()))
            .cloned())
    }
}

impl UsageSnapshotStore for InMemoryStore {
    fn write_usage_snapshot(
        &self,
        session_id: &str,
        snapshot: &SessionUsageSnapshot,
    ) -> crate::Result<()> {
        self.lock()
            .usage
            .insert(session_id.to_string(), snapshot.clone());
        Ok(())
    }

    fn load_usage_snapshot(&self, session_id: &str) -> crate::Result<Option<SessionUsageSnapshot>> {
        Ok(self.lock().usage.get(session_id).cloned())
    }
}

/// In-memory [`SessionCatalog`]. Holds one shared [`InMemoryStore`] for
/// every cwd (the RAM backend has no project scoping), so the per-turn
/// engine store and the cross-project `SessionManager` see the same
/// state within a process.
pub struct InMemoryCatalog {
    store: Arc<InMemoryStore>,
}

impl InMemoryCatalog {
    pub fn new() -> Self {
        Self {
            store: Arc::new(InMemoryStore::new()),
        }
    }

    /// Wrap an existing store so a caller can hold the same handle the
    /// catalog hands out (used by the swap test to assert shared state).
    pub fn with_store(store: Arc<InMemoryStore>) -> Self {
        Self { store }
    }
}

impl Default for InMemoryCatalog {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionCatalog for InMemoryCatalog {
    fn store_for(&self, _cwd: &Path) -> Arc<dyn SessionStore> {
        self.store.clone()
    }

    fn resolve(
        &self,
        session_id: &str,
        _cwd_hint: Option<&Path>,
    ) -> crate::Result<Option<ResolvedSession>> {
        if TranscriptIo::exists(&*self.store, session_id) {
            Ok(Some(ResolvedSession {
                session_id: session_id.to_string(),
                project: None,
                transcript_path: PathBuf::from(format!("memory://{session_id}")),
            }))
        } else {
            Ok(None)
        }
    }

    fn list_all(&self) -> crate::Result<Vec<TranscriptMetadata>> {
        self.store.list_sessions()
    }

    fn read_metadata(
        &self,
        session_id: &str,
        _cwd_hint: Option<&Path>,
    ) -> crate::Result<Option<TranscriptMetadata>> {
        if TranscriptIo::exists(&*self.store, session_id) {
            Ok(Some(self.store.read_metadata(session_id)?))
        } else {
            Ok(None)
        }
    }

    fn delete(&self, session_id: &str, _cwd_hint: Option<&Path>) -> crate::Result<()> {
        TranscriptIo::delete(&*self.store, session_id)
    }
}

#[cfg(test)]
#[path = "store.test.rs"]
mod tests;
