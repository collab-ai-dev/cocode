//! Cross-process session write lease.
//!
//! Every *writable* materialization of a session id must hold one exclusive
//! lease (§9.1 of `docs/coco-rs/goal-architecture-redesign.md`). Two coco
//! processes may share a workspace, but they must not materialize the same
//! session for mutation at once. This protects transcript appends, metadata,
//! resume recovery, usage snapshots, plan binding, goal state — every session
//! mutation, not only goals.
//!
//! The lease is a capability enforced by the API: mutating flows require the
//! matching [`SessionWriteLease`] (or a writable store handle that encapsulates
//! it). Read-only listing/inspection does not.
//!
//! Two layers guard acquisition:
//!
//! * an OS-backed exclusive advisory lock on a stable `.session-locks/<id>.lock`
//!   file (authoritative across processes; released automatically on process
//!   death); and
//! * a process-local registry keyed by the normalized lock path, so two runtimes
//!   in one process never depend on platform-specific same-process lock
//!   semantics.

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, MutexGuard};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Stable error code: another owner holds the writable lease.
pub const SESSION_IN_USE: &str = "session_in_use";
/// Stable error code: the filesystem cannot provide exclusive-lock semantics.
pub const SESSION_LOCK_UNSUPPORTED: &str = "session_lock_unsupported";
/// Stable error code: an unexpected I/O failure while acquiring the lease.
pub const SESSION_LEASE_IO: &str = "session_lease_io";

/// Why a writable lease could not be granted.
#[derive(Debug, Error)]
pub enum SessionLeaseError {
    /// The session is already open for writing (this process or another).
    #[error("session `{session_id}` is already open for writing")]
    InUse {
        session_id: String,
        /// Best-effort holder diagnostics; the OS lock is authoritative.
        owner: Option<LeaseOwner>,
    },
    /// The filesystem does not support the required exclusive-lock semantics.
    /// Writable materialization fails closed rather than silently degrading.
    #[error("session write lease unsupported on this filesystem: {reason}")]
    Unsupported { reason: String },
    /// An unexpected I/O error while acquiring the lease.
    #[error("session lease I/O error: {source}")]
    Io { source: std::io::Error },
}

impl SessionLeaseError {
    /// Stable machine-readable code for protocol/diagnostic surfaces.
    pub fn code(&self) -> &'static str {
        match self {
            Self::InUse { .. } => SESSION_IN_USE,
            Self::Unsupported { .. } => SESSION_LOCK_UNSUPPORTED,
            Self::Io { .. } => SESSION_LEASE_IO,
        }
    }

    fn in_use(session_id: &str, owner: Option<LeaseOwner>) -> Self {
        Self::InUse {
            session_id: session_id.to_string(),
            owner,
        }
    }
}

/// Best-effort diagnostics about a lease holder. Persisted in the lock file's
/// body; the kernel lock — not these bytes — is authoritative.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseOwner {
    pub pid: u32,
    pub instance_id: String,
    pub acquired_at_ms: i64,
}

impl LeaseOwner {
    fn current() -> Self {
        Self {
            pid: std::process::id(),
            instance_id: process_instance_id().to_string(),
            acquired_at_ms: now_ms(),
        }
    }
}

/// Capability trait: acquire the exclusive writable lease for a session.
/// Folded into [`crate::SessionStore`] so any store handle can gate writes.
pub trait SessionLeaseStore: Send + Sync {
    /// Acquire the writable lease for `session_id`, non-blocking. Returns
    /// [`SessionLeaseError::InUse`] with owner diagnostics when held elsewhere,
    /// or [`SessionLeaseError::Unsupported`] when the backend cannot lock.
    fn require_write_lease(&self, session_id: &str)
    -> Result<SessionWriteLease, SessionLeaseError>;
}

/// RAII proof of exclusive writable ownership of one session's storage.
/// Dropping it releases the OS lock (by closing the fd) and the process-local
/// registry entry. The lock *file* is intentionally not deleted.
pub struct SessionWriteLease {
    session_id: String,
    owner: LeaseOwner,
    /// Held to keep the OS advisory lock; `None` for the in-memory backend.
    _file: Option<File>,
    /// Held to keep the process-local exclusive slot.
    _process_local: ProcessLocalGuard,
}

impl SessionWriteLease {
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn owner(&self) -> &LeaseOwner {
        &self.owner
    }
}

impl std::fmt::Debug for SessionWriteLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionWriteLease")
            .field("session_id", &self.session_id)
            .field("owner", &self.owner)
            .finish_non_exhaustive()
    }
}

// ── Process-local registry ─────────────────────────────────────────────────

static LEASE_REGISTRY: LazyLock<Mutex<HashSet<PathBuf>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

fn lock_registry() -> MutexGuard<'static, HashSet<PathBuf>> {
    LEASE_REGISTRY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Guards one process-local lease slot keyed by normalized lock path.
struct ProcessLocalGuard {
    key: PathBuf,
}

impl Drop for ProcessLocalGuard {
    fn drop(&mut self) {
        lock_registry().remove(&self.key);
    }
}

fn try_process_local(key: &Path) -> Option<ProcessLocalGuard> {
    let mut registry = lock_registry();
    if registry.contains(key) {
        return None;
    }
    registry.insert(key.to_path_buf());
    Some(ProcessLocalGuard {
        key: key.to_path_buf(),
    })
}

static PROCESS_INSTANCE_ID: LazyLock<String> = LazyLock::new(|| uuid::Uuid::new_v4().to_string());

fn process_instance_id() -> &'static str {
    &PROCESS_INSTANCE_ID
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

// ── Acquisition ────────────────────────────────────────────────────────────

/// Acquire the file-backed lease at `lock_path`. Process-local slot first, then
/// the OS advisory lock.
pub(crate) fn acquire_file_lease(
    lock_path: &Path,
    session_id: &str,
) -> Result<SessionWriteLease, SessionLeaseError> {
    let process_local =
        try_process_local(lock_path).ok_or_else(|| SessionLeaseError::in_use(session_id, None))?;

    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| SessionLeaseError::Io { source })?;
    }
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(lock_path)
        .map_err(|source| SessionLeaseError::Io { source })?;

    match file.try_lock_exclusive() {
        Ok(()) => {
            let owner = LeaseOwner::current();
            write_owner_diagnostics(&file, &owner);
            Ok(SessionWriteLease {
                session_id: session_id.to_string(),
                owner,
                _file: Some(file),
                _process_local: process_local,
            })
        }
        Err(err) if is_would_block(&err) => Err(SessionLeaseError::in_use(
            session_id,
            read_owner_diagnostics(lock_path),
        )),
        Err(err) if is_unsupported(&err) => Err(SessionLeaseError::Unsupported {
            reason: err.to_string(),
        }),
        Err(source) => Err(SessionLeaseError::Io { source }),
    }
}

/// Acquire a process-local-only lease for the in-memory backend.
pub(crate) fn acquire_memory_lease(
    session_id: &str,
) -> Result<SessionWriteLease, SessionLeaseError> {
    let key = PathBuf::from(format!("mem-lease://{session_id}"));
    let process_local =
        try_process_local(&key).ok_or_else(|| SessionLeaseError::in_use(session_id, None))?;
    Ok(SessionWriteLease {
        session_id: session_id.to_string(),
        owner: LeaseOwner::current(),
        _file: None,
        _process_local: process_local,
    })
}

fn is_would_block(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::WouldBlock
}

fn is_unsupported(err: &std::io::Error) -> bool {
    #[cfg(unix)]
    {
        let code = err.raw_os_error();
        code == Some(libc::ENOLCK) || code == Some(libc::ENOTSUP) || code == Some(libc::EOPNOTSUPP)
    }
    #[cfg(not(unix))]
    {
        let _ = err;
        false
    }
}

fn write_owner_diagnostics(mut file: &File, owner: &LeaseOwner) {
    let Ok(json) = serde_json::to_vec(owner) else {
        return;
    };
    // Best-effort: the kernel lock is authoritative, the body is diagnostics.
    let _ = file.set_len(0);
    let _ = file.seek(SeekFrom::Start(0));
    let _ = file.write_all(&json);
    let _ = file.flush();
}

fn read_owner_diagnostics(lock_path: &Path) -> Option<LeaseOwner> {
    let bytes = std::fs::read(lock_path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[cfg(test)]
#[path = "lease.test.rs"]
mod tests;
