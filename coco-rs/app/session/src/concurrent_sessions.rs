//! Per-process PID-file registry under `<config_home>/sessions/pids/{pid}.json`.
//!
//! Every top-level coco session (interactive CLI, SDK, bg/daemon spawn)
//! writes a single small JSON file keyed by its OS pid so a cross-cutting
//! `coco ps` view can enumerate live sessions, surface live activity
//! (`status` / `waitingFor`), and de-duplicate sessions reachable over
//! multiple transports (`bridgeSessionId`). Subagent / teammate sessions
//! intentionally do *not* register (`agent_id` set ⇒ skip) — counting
//! them would conflate swarm activity with real concurrency.
//!
//! ## Layout
//!
//! ```text
//! <config_home>/sessions/pids/
//! ├── 12345.json   # one file per live session
//! ├── 67890.json
//! └── …
//! ```
//!
//! Directory mode `0o700`. Each file contains the serialized
//! [`SessionRegistration`] record (camelCase JSON, matching TS on the
//! wire). The dedicated `pids/` subdir means coco's own session JSONL
//! transcripts under `<memory_base>/projects/<slug>/<sid>.jsonl` —
//! which can share a parent with `<config_home>/sessions/` in certain
//! `COCO_REMOTE_MEMORY_DIR` configs — never share a namespace with
//! the PID registry, eliminating the class of bugs where a UUID-named
//! transcript could falsely deserialize as a `SessionRegistration`.
//!
//! ## Lifecycle
//!
//! 1. [`SessionRegistry::register`] writes the initial file and returns
//!    a guard. Drop the guard (process exit or explicit
//!    [`SessionRegistry::unregister`]) to delete the file.
//! 2. Live updates (`update_session_*`) merge into the existing file
//!    via read-modify-write; concurrent writes from the same process
//!    are not a concern because each process owns exactly one pid file.
//! 3. [`count_concurrent_sessions`] sweeps stale files (PID not running)
//!    on every call so a crashed session doesn't inflate the count
//!    forever. WSL is excluded from the sweep — if `~/.coco/sessions/`
//!    is shared with Windows-native coco the WSL probe falsely reports
//!    "not running" for Windows PIDs.

use coco_config::env::EnvKey;
use serde::Deserialize;
use serde::Serialize;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

/// Kind of session, written to the PID registry. Drives `coco ps`
/// grouping and exit-path behavior (bg sessions detach instead of
/// killing the process).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionKind {
    Interactive,
    Bg,
    Daemon,
    DaemonWorker,
}

impl SessionKind {
    /// Parse the env-var value used by `COCO_SESSION_KIND`. Only `bg` /
    /// `daemon` / `daemon-worker` are honored; anything else falls back
    /// to the caller's default.
    fn from_env_value(s: &str) -> Option<Self> {
        match s {
            "bg" => Some(Self::Bg),
            "daemon" => Some(Self::Daemon),
            "daemon-worker" => Some(Self::DaemonWorker),
            _ => None,
        }
    }
}

/// Live activity state for the `coco ps` sparkline. Written by the
/// REPL's status-change effect; absence means "no live status reported
/// yet — fall back to transcript-tail heuristic".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Busy,
    Idle,
    Waiting,
}

/// Wire shape of a single `<pid>.json` file. Snake_case wire,
/// `Option`s skipped when missing. The file is coco-rs's own
/// registry; no cross-implementation consumer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRegistration {
    pub pid: u32,
    pub session_id: String,
    pub cwd: PathBuf,
    /// Unix-ms timestamp.
    pub started_at: i64,
    pub kind: SessionKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entrypoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bridge_session_id: Option<String>,
    /// Unix-ms timestamp of the last `update_session_activity` call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<SessionStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_for: Option<String>,
}

/// Per-process registry guard. Holding one means a PID file exists at
/// `<sessions_dir>/<pid>.json`; dropping it deletes the file (best
/// effort — `ENOENT` is fine).
///
/// Constructed via [`SessionRegistry::register`]. The guard is the
/// only safe handle for the live-update calls because they need to
/// know the pid file path; subagent contexts (which never register)
/// simply never hold a guard.
///
/// The `write_lock` serializes concurrent same-process patches
/// (`update_session_*`). Without it, a teleport rename racing with
/// the per-turn `update_session_activity` would interleave their
/// read-modify-write passes and one side's field would silently
/// overwrite the other's. Single-threaded JS doesn't see this; tokio
/// does.
pub struct SessionRegistry {
    pid_file: PathBuf,
    write_lock: Mutex<()>,
}

impl SessionRegistry {
    /// Write the initial PID file under `<config_home>/sessions/`.
    ///
    /// Returns `Ok(Some(guard))` when registered. Returns `Ok(None)`
    /// when the caller is a subagent — counted as success because
    /// callers treat this as fire-and-forget. Errors (mkdir / write /
    /// chmod) bubble up so the caller can `tracing::warn` and proceed.
    pub fn register(
        config_home: &Path,
        session_id: &str,
        cwd: &Path,
        agent_id: Option<&str>,
    ) -> std::io::Result<Option<Self>> {
        if agent_id.is_some() {
            return Ok(None);
        }
        let dir = sessions_dir(config_home);
        let pid = std::process::id();
        let pid_file = dir.join(format!("{pid}.json"));
        create_sessions_dir(&dir)?;

        let kind = env_session_kind().unwrap_or(SessionKind::Interactive);
        let record = SessionRegistration {
            pid,
            session_id: session_id.to_string(),
            cwd: cwd.to_path_buf(),
            started_at: now_ms(),
            kind,
            entrypoint: coco_config::env::var(EnvKey::CocoEntrypoint).ok(),
            name: None,
            bridge_session_id: None,
            updated_at: None,
            status: None,
            waiting_for: None,
        };
        write_record(&pid_file, &record)?;

        Ok(Some(Self {
            pid_file,
            write_lock: Mutex::new(()),
        }))
    }

    /// Explicitly delete the PID file. Equivalent to dropping the
    /// guard but propagates the error to the caller.
    pub fn unregister(self) -> std::io::Result<()> {
        let path = self.pid_file.clone();
        // Forget self to avoid the Drop-impl double-unlink; we already
        // consumed it here.
        std::mem::forget(self);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Apply a JSON-merge patch to the PID file under the registry's
    /// write lock so concurrent updaters serialize cleanly.
    fn apply_patch(&self, patch: serde_json::Map<String, serde_json::Value>) {
        // PoisonError just means a prior holder panicked mid-update;
        // we still want to apply the patch (and another panic is
        // unlikely from a serde_json round-trip).
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = update_pid_file(&self.pid_file, patch);
    }

    /// Update this session's human-readable name (shown by `coco ps`
    /// and `/resume`). Silent no-op when `name` is empty or the file
    /// has gone away (session not registered, or already cleaned up).
    pub fn update_session_name(&self, name: &str) {
        if name.is_empty() {
            return;
        }
        let mut patch = serde_json::Map::new();
        patch.insert("name".into(), serde_json::Value::String(name.into()));
        self.apply_patch(patch);
    }

    /// Record the Remote Control / bridge session ID so peer
    /// enumeration can de-duplicate UDS + bridge entries. Passing
    /// `None` clears the field (used on bridge teardown so a stale ID
    /// doesn't suppress a legitimately-remote session after reconnect).
    pub fn update_session_bridge_id(&self, bridge_session_id: Option<&str>) {
        let mut patch = serde_json::Map::new();
        patch.insert(
            "bridge_session_id".into(),
            match bridge_session_id {
                Some(s) => serde_json::Value::String(s.to_string()),
                None => serde_json::Value::Null,
            },
        );
        self.apply_patch(patch);
    }

    /// Push live activity state for the `coco ps` sparkline. Both
    /// fields are optional patches — pass `None` to leave the existing
    /// value untouched. Always stamps `updated_at` so the reader can
    /// tell stale rows apart.
    pub fn update_session_activity(
        &self,
        status: Option<SessionStatus>,
        waiting_for: Option<&str>,
    ) {
        let mut patch = serde_json::Map::new();
        if let Some(status) = status {
            patch.insert(
                "status".into(),
                serde_json::to_value(status).unwrap_or(serde_json::Value::Null),
            );
        }
        if let Some(s) = waiting_for {
            patch.insert(
                "waiting_for".into(),
                serde_json::Value::String(s.to_string()),
            );
        }
        patch.insert(
            "updated_at".into(),
            serde_json::Value::Number(serde_json::Number::from(now_ms())),
        );
        self.apply_patch(patch);
    }

    /// Path to the PID file backing this registration. Exposed for
    /// tests + `coco ps` reading the same file directly.
    pub fn pid_file(&self) -> &Path {
        &self.pid_file
    }
}

impl Drop for SessionRegistry {
    fn drop(&mut self) {
        // Best effort — process is going down, swallow IO errors.
        let _ = std::fs::remove_file(&self.pid_file);
    }
}

/// True when the current process is the child of a `coco --bg` (tmux)
/// spawn. Exit paths (e.g. /exit, Ctrl-C) detach the attached client
/// instead of killing the process.
pub fn is_bg_session() -> bool {
    env_session_kind() == Some(SessionKind::Bg)
}

/// Count live sessions under `<config_home>/sessions/`.
///
/// Sweeps PID files whose processes are no longer running, except on
/// WSL where the `kill -0` probe can lie about Windows PIDs (silent
/// data loss in a shared `~/.coco/sessions/` mount). The current
/// process is always counted, even if its file hasn't been written yet.
///
/// Returns `0` on any directory-read error (conservative).
pub fn count_concurrent_sessions(config_home: &Path) -> i64 {
    let dir = sessions_dir(config_home);
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return 0,
    };

    let our_pid = std::process::id();
    let is_wsl = detect_wsl();
    let mut count: i64 = 0;
    for entry in entries.flatten() {
        let name_os = entry.file_name();
        let name = name_os.to_string_lossy();
        // Strict guard: only `<digits>.json` is a candidate.
        // `str::parse` rejects non-numeric prefixes, but the explicit
        // all-digits check makes the invariant clear.
        let Some(stem) = name.strip_suffix(".json") else {
            continue;
        };
        if stem.is_empty() || !stem.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let Ok(pid) = stem.parse::<u32>() else {
            continue;
        };
        if pid == our_pid {
            count += 1;
            continue;
        }
        if is_process_running(pid) {
            count += 1;
        } else if !is_wsl {
            // Stale file — sweep. Best effort; ignore unlink errors.
            let _ = std::fs::remove_file(entry.path());
        }
    }
    count
}

/// Read a single PID file (without sweeping). Useful for `coco ps`
/// formatters and tests that want to assert the wire shape after a
/// patch. `Ok(None)` when the file doesn't exist.
pub fn read_registration(
    config_home: &Path,
    pid: u32,
) -> std::io::Result<Option<SessionRegistration>> {
    let path = sessions_dir(config_home).join(format!("{pid}.json"));
    match std::fs::read_to_string(&path) {
        Ok(body) => match serde_json::from_str::<SessionRegistration>(&body) {
            Ok(rec) => Ok(Some(rec)),
            Err(e) => Err(std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

// ── private helpers ───────────────────────────────────────────────

fn sessions_dir(config_home: &Path) -> PathBuf {
    // Dedicated `pids/` subdir keeps the PID registry namespaced away
    // from any other JSON the user (or another coco subsystem) might
    // drop under `<config_home>/sessions/`. The sweep regex
    // `^\d+\.json$` already guards against accidental deletion, but
    // the subdir provides defense-in-depth.
    config_home.join("sessions").join("pids")
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn env_session_kind() -> Option<SessionKind> {
    coco_config::env::var(EnvKey::CocoSessionKind)
        .ok()
        .and_then(|s| SessionKind::from_env_value(&s))
}

fn create_sessions_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn write_record(path: &Path, record: &SessionRegistration) -> std::io::Result<()> {
    let body = serde_json::to_string(record)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, body)
}

fn update_pid_file(
    path: &Path,
    patch: serde_json::Map<String, serde_json::Value>,
) -> std::io::Result<()> {
    let body = match std::fs::read_to_string(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // File gone — session deregistered or never registered.
            return Ok(());
        }
        Err(e) => return Err(e),
    };
    let mut current: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let obj = current.as_object_mut().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "pid file is not an object")
    })?;
    for (k, v) in patch {
        obj.insert(k, v);
    }
    let next = serde_json::to_string(&current)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, next)
}

fn is_process_running(pid: u32) -> bool {
    if pid <= 1 {
        return false;
    }
    #[cfg(unix)]
    {
        // Signal 0 probes existence without delivering a signal.
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

/// Best-effort WSL detection. WSL exposes itself via `/proc/version`
/// containing "Microsoft" or the `WSL_DISTRO_NAME` env. Used only to
/// skip the stale-file sweep — false negatives are safe (we just keep
/// the conservative TS behavior).
fn detect_wsl() -> bool {
    if std::env::var("WSL_DISTRO_NAME").is_ok() {
        return true;
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(v) = std::fs::read_to_string("/proc/version") {
            return v.contains("Microsoft") || v.contains("microsoft");
        }
    }
    false
}

#[cfg(test)]
#[path = "concurrent_sessions.test.rs"]
mod tests;
