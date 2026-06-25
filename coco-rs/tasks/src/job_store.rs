//! Durable terminal-record store for background jobs under
//! `<config_home>/bg-jobs/<id>.json`.
//!
//! The PID registry ([`coco_session::SessionRegistry`]) deletes a
//! session's `<pid>.json` file on process exit, so it carries no
//! terminal state — a completed or failed background job leaves no
//! trace there. `coco ps` therefore cannot report `done` / `failed` /
//! `stopped` from the PID registry alone.
//!
//! This store fills that gap: a background job writes a [`JobState`]
//! record keyed by a stable job id that **survives process exit**, so
//! the `ps` lifecycle mapper can surface real terminal outcomes by
//! merging these records with the live PID sweep (keyed on
//! `session_id`).
//!
//! ## Scope (first cut)
//!
//! This is intentionally just the typed record + file IO. The daemon
//! supervisor process, RPC roster, worker fork/retire/respawn, and the
//! spawn/exit wiring that *writes* these records are deliberately
//! deferred — a follow-up wires bg-agent spawn/exit to call
//! [`JobStore::write`].
//!
//! ## Layout
//!
//! ```text
//! <config_home>/bg-jobs/
//! ├── job-abc.json   # one file per job record
//! └── …
//! ```
//!
//! Directory mode `0o700`. Snake_case wire, `Ok(None)` on `ENOENT` —
//! mirrors the `coco-session` PID-registry IO idioms.

use coco_session::SessionKind;
use coco_types::TaskStatus;
use serde::Deserialize;
use serde::Serialize;
use std::path::Path;
use std::path::PathBuf;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

/// Durable record of a background job. Reuses [`SessionKind`] and
/// [`TaskStatus`] rather than inventing parallel enums — `TaskStatus`
/// is the one that can represent terminal `Completed` / `Failed` /
/// `Killed`, which is exactly what the `ps` lifecycle mapper needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobState {
    /// Stable job id, independent of any OS pid. Names the on-disk file.
    pub id: String,
    /// Session this job belongs to — the merge key against the PID
    /// registry's `SessionRegistration.session_id`.
    pub session_id: String,
    pub cwd: PathBuf,
    pub kind: SessionKind,
    /// Unix-ms timestamp.
    pub created_at: i64,
    /// Unix-ms timestamp of the last [`JobStore::write`].
    pub updated_at: i64,
    pub status: TaskStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Original prompt / intent that launched the job.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,
    /// Error text from a `Failed` terminal transition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// File-backed store for [`JobState`] records under
/// `<config_home>/bg-jobs/`.
pub struct JobStore {
    dir: PathBuf,
}

impl JobStore {
    /// Open the store rooted at `<config_home>/bg-jobs`. Does not touch
    /// the filesystem — the directory is created lazily on first
    /// [`write`](Self::write).
    pub fn new(config_home: &Path) -> Self {
        Self {
            dir: config_home.join("bg-jobs"),
        }
    }

    /// Persist a job record to `bg-jobs/<id>.json` (creating the dir
    /// `0o700` if needed). Mirrors the PID-registry write idiom.
    pub fn write(&self, job: &JobState) -> crate::Result<()> {
        create_jobs_dir(&self.dir)?;
        let path = self.job_path(&job.id);
        let body = serde_json::to_string_pretty(job)?;
        std::fs::write(&path, body)?;
        Ok(())
    }

    /// Read a single job record. `Ok(None)` when the file doesn't exist.
    pub fn read(&self, id: &str) -> crate::Result<Option<JobState>> {
        let path = self.job_path(id);
        match std::fs::read_to_string(&path) {
            Ok(body) => Ok(Some(serde_json::from_str::<JobState>(&body)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// List every job record. Unreadable / malformed files are skipped
    /// (best effort) rather than failing the whole listing. Returns an
    /// empty vec when the directory doesn't exist.
    pub fn list(&self) -> crate::Result<Vec<JobState>> {
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let name_os = entry.file_name();
            let name = name_os.to_string_lossy();
            let Some(id) = name.strip_suffix(".json") else {
                continue;
            };
            if id.is_empty() || name.starts_with('.') {
                continue;
            }
            if let Ok(Some(job)) = self.read(id) {
                out.push(job);
            }
        }
        Ok(out)
    }

    /// Delete a job record. `ENOENT` is treated as success (idempotent).
    pub fn remove(&self, id: &str) -> crate::Result<()> {
        match std::fs::remove_file(self.job_path(id)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    fn job_path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }
}

/// Wall-clock now as a unix-millisecond timestamp.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn create_jobs_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "job_store.test.rs"]
mod tests;
