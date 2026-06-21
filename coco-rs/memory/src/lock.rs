//! PID + mtime CAS lock for auto-dream consolidation.
//!
//! The lock file's **mtime** is the `lastConsolidatedAt` timestamp;
//! its body is the holder's PID. A stale-PID + 1h-mtime threshold
//! lets a crashed parent
//! release the lock for a follow-up reclaim.
//!
//! ## RAII guard
//!
//! [`LockGuard`] holds the lock for the duration of a consolidation
//! attempt. Its sync `Drop` runs [`rollback`] (or [`release`] when the
//! caller marked the run as committed via [`LockGuard::commit`]),
//! restoring the prior mtime on failure / cancellation so the
//! 24h auto-dream gate doesn't reset to "now". Rust async-drop
//! semantics mean a cancelled future can't leak the lock file with
//! our live PID for the next hour.

use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::SystemTime;

/// Lock file basename inside the memory directory.
pub const LOCK_FILENAME: &str = ".consolidate-lock";

/// Dead-PID reclaim threshold. A lock older than this is reclaimable
/// even if the holder PID is alive (defensive: prevents stuck locks).
pub const HOLDER_STALE_SECS: u64 = 60 * 60;

/// Lock acquisition outcome.
#[derive(Debug)]
pub enum LockOutcome {
    /// Lock acquired. The `LockGuard` releases (or rolls back) on
    /// drop — call [`LockGuard::commit`] to make the held mtime stick
    /// when a consolidation run succeeded.
    Acquired(LockGuard),
    /// Lock is held by a live PID with a fresh mtime.
    Held,
    /// Filesystem error during acquisition.
    Error(String),
}

/// RAII handle for an acquired consolidation lock.
///
/// `Drop` synchronously rolls the mtime back to `prior_mtime_ms` (the
/// value before this guard's `try_acquire` overwrote it), unless the
/// caller marked the run committed via [`Self::commit`]. The rollback
/// covers both the explicit failure path (when the agent spawn
/// returns `Err`) and async cancellation — a dropped consolidation
/// future leaves the prior auto-dream cadence intact rather than
/// silently bumping the 24h gate to "now".
pub struct LockGuard {
    memory_dir: PathBuf,
    prior_mtime_ms: i64,
    /// Set when the caller wants the mtime to stick — successful run
    /// or explicit "rollback so manual /dream doesn't perturb auto
    /// cadence" decision.
    rollback_on_drop: AtomicBool,
}

impl std::fmt::Debug for LockGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LockGuard")
            .field("memory_dir", &self.memory_dir)
            .field("prior_mtime_ms", &self.prior_mtime_ms)
            .field(
                "rollback_on_drop",
                &self.rollback_on_drop.load(Ordering::Acquire),
            )
            .finish()
    }
}

impl LockGuard {
    /// Mark the consolidation run as committed. After this, `Drop` is
    /// a no-op (the mtime stamp stays at "now", which is what the
    /// next 24h gate reads).
    pub fn commit(&self) {
        self.rollback_on_drop.store(false, Ordering::Release);
    }

    /// Force an explicit rollback of the mtime now. Used by manual
    /// `/dream` so the run doesn't perturb the auto cadence — the
    /// previous successful run's mtime stays the reference point.
    /// Subsequent `Drop` is a no-op.
    pub fn rollback_now(&self) {
        rollback(&self.memory_dir, self.prior_mtime_ms);
        self.rollback_on_drop.store(false, Ordering::Release);
    }

    /// Prior mtime — surfaced for telemetry / log lines so callers
    /// don't need to thread a separate `prior_mtime_ms` field.
    pub fn prior_mtime_ms(&self) -> i64 {
        self.prior_mtime_ms
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        if self.rollback_on_drop.load(Ordering::Acquire) {
            rollback(&self.memory_dir, self.prior_mtime_ms);
        }
    }
}

/// Try to acquire the consolidation lock at `<memory_dir>/.consolidate-lock`.
pub fn try_acquire(memory_dir: &Path) -> LockOutcome {
    let lock_path = memory_dir.join(LOCK_FILENAME);

    let prior_mtime_ms = read_mtime_ms(&lock_path).unwrap_or(0);

    if lock_path.exists() {
        let holder_pid = std::fs::read_to_string(&lock_path)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok());
        let age_secs = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs())
            .map(|now| now.saturating_sub((prior_mtime_ms / 1000) as u64))
            .unwrap_or(u64::MAX);

        let our_pid = std::process::id();
        let fresh = age_secs < HOLDER_STALE_SECS;
        // Same-process holders are always reclaimable. Within-process
        // serialization is enforced separately by `DreamService`'s
        // `consolidating` atomic flag; the lock file's primary purpose
        // is cross-process exclusion + the `lastConsolidatedAt`
        // mtime stamp. Without this carve-out, a successful
        // auto-dream leaves the lock with our_pid + fresh mtime, and
        // a follow-up user-initiated `/dream` within 1h would see
        // `Held` (alive PID == us) and silently no-op.
        let alive_other = holder_pid
            .filter(|pid| *pid != our_pid)
            .is_some_and(is_process_running);

        if fresh && alive_other {
            return LockOutcome::Held;
        }
        // Stale, dead, or same-process — fall through and reclaim.
        // NB: do NOT `remove_file` here. A single `writeFile`
        // (POSIX `O_TRUNC | O_CREAT`) atomically overwrites without
        // an unlink window.
    }

    if let Some(parent) = lock_path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        return LockOutcome::Error(format!("could not create memory dir: {e}"));
    }
    let our_pid = std::process::id();
    if let Err(e) = std::fs::write(&lock_path, our_pid.to_string()) {
        return LockOutcome::Error(format!("could not write lock: {e}"));
    }

    // CAS verify: the file we just wrote should still hold our PID.
    let read_back = std::fs::read_to_string(&lock_path)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok());
    if read_back == Some(our_pid) {
        LockOutcome::Acquired(LockGuard {
            memory_dir: memory_dir.to_path_buf(),
            prior_mtime_ms,
            // Rollback by default — callers must explicitly `commit()`
            // on success. Fail-safe: a cancelled future restores the
            // prior mtime rather than bumping the 24h cadence.
            rollback_on_drop: AtomicBool::new(true),
        })
    } else {
        LockOutcome::Held
    }
}

/// Release the lock unconditionally — call on successful consolidation
/// completion. Idempotent.
pub fn release(memory_dir: &Path) {
    let _ = std::fs::remove_file(memory_dir.join(LOCK_FILENAME));
}

/// Roll the lock mtime back to `prior_mtime_ms` after a failed run, so
/// the time-gate resets to the previous successful consolidation
/// rather than "now." If `prior_mtime_ms == 0`, the lock didn't exist
/// before — remove it.
pub fn rollback(memory_dir: &Path, prior_mtime_ms: i64) {
    let lock_path = memory_dir.join(LOCK_FILENAME);
    if prior_mtime_ms == 0 {
        let _ = std::fs::remove_file(&lock_path);
        return;
    }
    if std::fs::write(&lock_path, "").is_err() {
        return;
    }
    let secs = prior_mtime_ms / 1000;
    let nanos = ((prior_mtime_ms % 1000) * 1_000_000) as u32;
    let time = filetime::FileTime::from_unix_time(secs, nanos);
    let _ = filetime::set_file_mtime(&lock_path, time);
}

/// Last successful consolidation timestamp (lock file mtime in ms),
/// or `None` if no lock has ever been written.
pub fn last_consolidated_at(memory_dir: &Path) -> Option<i64> {
    read_mtime_ms(&memory_dir.join(LOCK_FILENAME))
}

/// Stamp a successful consolidation — used when /dream runs manually
/// outside the normal lock cycle.
pub fn record_consolidation(memory_dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(memory_dir)?;
    std::fs::write(
        memory_dir.join(LOCK_FILENAME),
        std::process::id().to_string(),
    )
}

fn read_mtime_ms(path: &Path) -> Option<i64> {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
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
        // Conservative on non-Unix.
        true
    }
}

#[cfg(test)]
#[path = "lock.test.rs"]
mod tests;
