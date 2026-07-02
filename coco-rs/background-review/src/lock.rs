//! Parameterized PID + mtime CAS lock for background consolidation passes.
//!
//! The lock file's **mtime** is the "last consolidated at" timestamp; its body
//! is the holder's PID. A stale-PID + 1h-mtime threshold lets a crashed holder
//! release the lock for a follow-up reclaim.
//!
//! ## Atomic acquisition (`O_EXCL`)
//!
//! Acquisition uses `OpenOptions::create_new` (`O_EXCL`), which fails atomically
//! if the file already exists — there is no write-then-readback race window.
//! Stale reclaim is explicit: on `AlreadyExists`, the holder is re-inspected at
//! its **current** on-disk state (age > 1h, dead PID, or same-process) and only
//! then is the file removed and re-created. A concurrent racer that wins the
//! re-create leaves us with [`LockOutcome::Held`]. The stat→unlink reclaim
//! step is not atomic — a racer acquiring in that sub-millisecond window can
//! still be unlinked — acceptable for the hour-scale cadences this lock gates;
//! callers needing hard mutual exclusion should use an OS advisory lock.
//!
//! ## RAII guard
//!
//! [`LockGuard`] holds the lock for the duration of a consolidation attempt.
//! Its sync `Drop` rolls the mtime back to the prior value on failure /
//! cancellation (so a time gate doesn't reset to "now"), unless the caller
//! marked the run committed via [`LockGuard::commit`].

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

/// Dead-PID reclaim threshold. A lock older than this is reclaimable even if
/// the holder PID is alive (defensive: prevents stuck locks).
const HOLDER_STALE_SECS: u64 = 60 * 60;

/// Lock acquisition outcome.
#[derive(Debug)]
pub enum LockOutcome {
    /// Lock acquired. The `LockGuard` releases (or rolls back) on drop — call
    /// [`LockGuard::commit`] to make the held mtime stick when the run
    /// succeeded.
    Acquired(LockGuard),
    /// Lock is held by a live PID with a fresh mtime.
    Held,
    /// Filesystem error during acquisition.
    Error(String),
}

/// A cross-process consolidation lock at a caller-chosen path.
///
/// Construct with [`ConsolidateLock::new`], passing the directory and the lock
/// filename (e.g. `.consolidate-lock` for memory, `.skill-curator-lock` for
/// the skill curator).
#[derive(Debug, Clone)]
pub struct ConsolidateLock {
    lock_path: PathBuf,
}

impl ConsolidateLock {
    /// Build a lock rooted at `dir/filename`.
    pub fn new(dir: &Path, filename: &str) -> Self {
        Self {
            lock_path: dir.join(filename),
        }
    }

    /// Try to acquire the lock atomically.
    pub fn try_acquire(&self) -> LockOutcome {
        let prior_mtime_ms = read_mtime_ms(&self.lock_path).unwrap_or(0);

        if let Some(parent) = self.lock_path.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            return LockOutcome::Error(format!("could not create lock dir: {e}"));
        }

        match self.create_new_with_pid() {
            Ok(()) => LockOutcome::Acquired(self.guard(prior_mtime_ms)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if !self.is_reclaimable() {
                    return LockOutcome::Held;
                }
                // Stale / dead / same-process — reclaim by unlink + re-create.
                let _ = std::fs::remove_file(&self.lock_path);
                match self.create_new_with_pid() {
                    Ok(()) => LockOutcome::Acquired(self.guard(prior_mtime_ms)),
                    // Lost the race to another acquirer.
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => LockOutcome::Held,
                    Err(e) => LockOutcome::Error(format!("could not write lock: {e}")),
                }
            }
            Err(e) => LockOutcome::Error(format!("could not write lock: {e}")),
        }
    }

    /// Last successful consolidation timestamp (lock file mtime in ms), or
    /// `None` if no lock has ever been written.
    pub fn last_consolidated_at(&self) -> Option<i64> {
        read_mtime_ms(&self.lock_path)
    }

    fn guard(&self, prior_mtime_ms: i64) -> LockGuard {
        LockGuard {
            lock_path: self.lock_path.clone(),
            prior_mtime_ms,
            // Rollback by default — callers must explicitly `commit()` on
            // success. Fail-safe: a cancelled future restores the prior mtime.
            rollback_on_drop: AtomicBool::new(true),
        }
    }

    fn create_new_with_pid(&self) -> std::io::Result<()> {
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&self.lock_path)?;
        f.write_all(std::process::id().to_string().as_bytes())
    }

    /// True when an existing lock may be reclaimed: stale by age, held by a
    /// dead/unparseable PID, or held by this same process. Returns `false`
    /// only when the lock is fresh AND held by a live *other* process.
    ///
    /// Age is judged from the holder's **current** mtime, re-read here — not
    /// from any value captured before `create_new` failed. Judging by a stale
    /// pre-read would let two processes both deem an old lock reclaimable and
    /// the slower one unlink the faster one's freshly re-created lock,
    /// double-acquiring.
    fn is_reclaimable(&self) -> bool {
        let Some(holder_mtime_ms) = read_mtime_ms(&self.lock_path) else {
            // Holder vanished between create_new failing and this stat — the
            // follow-up create_new resolves the race (loser gets Held).
            return true;
        };
        let now_secs = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(u64::MAX);
        let age_secs = now_secs.saturating_sub((holder_mtime_ms / 1000).max(0) as u64);
        if age_secs >= HOLDER_STALE_SECS {
            return true;
        }
        let holder_pid = std::fs::read_to_string(&self.lock_path)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok());
        match holder_pid {
            // Same-process holders are always reclaimable; within-process
            // serialization is enforced separately by the caller's in-process
            // flag. Without this, a successful pass would wedge a follow-up
            // manual run for the next hour.
            Some(pid) if pid == std::process::id() => true,
            Some(pid) => !is_process_running(pid),
            None => true,
        }
    }
}

/// RAII handle for an acquired consolidation lock.
///
/// `Drop` synchronously rolls the mtime back to `prior_mtime_ms` unless the
/// caller marked the run committed via [`Self::commit`].
pub struct LockGuard {
    lock_path: PathBuf,
    prior_mtime_ms: i64,
    rollback_on_drop: AtomicBool,
}

impl std::fmt::Debug for LockGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LockGuard")
            .field("lock_path", &self.lock_path)
            .field("prior_mtime_ms", &self.prior_mtime_ms)
            .field(
                "rollback_on_drop",
                &self.rollback_on_drop.load(Ordering::Acquire),
            )
            .finish()
    }
}

impl LockGuard {
    /// Mark the run as committed. After this, `Drop` is a no-op (the mtime
    /// stamp stays at "now", which is what the next time gate reads).
    pub fn commit(&self) {
        self.rollback_on_drop.store(false, Ordering::Release);
    }

    /// Force an explicit rollback of the mtime now. Used by manual runs so
    /// they don't perturb the automatic cadence. Subsequent `Drop` is a no-op.
    pub fn rollback_now(&self) {
        rollback_path(&self.lock_path, self.prior_mtime_ms);
        self.rollback_on_drop.store(false, Ordering::Release);
    }

    /// Prior mtime — surfaced for telemetry / log lines.
    pub fn prior_mtime_ms(&self) -> i64 {
        self.prior_mtime_ms
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        if self.rollback_on_drop.load(Ordering::Acquire) {
            rollback_path(&self.lock_path, self.prior_mtime_ms);
        }
    }
}

fn rollback_path(lock_path: &Path, prior_mtime_ms: i64) {
    if prior_mtime_ms == 0 {
        let _ = std::fs::remove_file(lock_path);
        return;
    }
    if std::fs::write(lock_path, "").is_err() {
        return;
    }
    let secs = prior_mtime_ms / 1000;
    let nanos = ((prior_mtime_ms % 1000) * 1_000_000) as u32;
    let time = filetime::FileTime::from_unix_time(secs, nanos);
    let _ = filetime::set_file_mtime(lock_path, time);
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
