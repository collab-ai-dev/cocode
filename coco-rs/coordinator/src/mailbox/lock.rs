//! Advisory file locking for mailbox JSON files.
//!
//! Mailboxes are append-only JSON arrays read-modify-written by multiple
//! teammates. Without serialisation a "read → push → write" race drops
//! concurrent peers' messages. This module provides
//! [`with_inbox_lock`], a retry-with-jitter helper that holds an
//! exclusive `fs2` advisory lock on a sidecar `.lock` file for the
//! duration of the body. The matching no-lock read used inside the
//! critical section lives in [`read_messages_no_lock`].
//!
use super::io::TeammateMessage;

/// Run `body` while holding an exclusive advisory lock on a sidecar
/// `{path}.lock` file, retrying acquisition on contention.
pub(crate) fn with_inbox_lock<F>(path: &std::path::Path, body: F) -> crate::Result<()>
where
    F: FnOnce(&std::path::Path) -> crate::Result<()>,
{
    use fs2::FileExt;
    let lock_path = path.with_extension("json.lock");
    // `create(true)` so the lockfile can be created on first access to
    // an inbox that doesn't yet exist. We don't write anything into it;
    // it's purely the lock target.
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&lock_path)?;

    // Retry envelope: 30 attempts × exponential 5 → 100 ms with jitter.
    // Jitter scales each backoff by
    // [0.5, 1.5) to break thundering-herd wake-ups when many writers
    // contend at once (pure exponential backoff syncs all retry-waves
    // across threads).
    const MAX_RETRIES: u32 = 30;
    const MIN_DELAY_MS: u64 = 5;
    const MAX_DELAY_MS: u64 = 100;
    let mut delay_ms: u64 = MIN_DELAY_MS;
    for attempt in 0..MAX_RETRIES {
        match lock_file.try_lock_exclusive() {
            Ok(()) => {
                let result = body(path);
                let _ = lock_file.unlock();
                return result;
            }
            Err(_) if attempt + 1 < MAX_RETRIES => {
                let jitter_bits = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.subsec_nanos())
                    .unwrap_or(0);
                let jitter_pct = 50 + (jitter_bits % 100) as u64; // [50, 150)
                let jittered = delay_ms * jitter_pct / 100;
                std::thread::sleep(std::time::Duration::from_millis(jittered.max(1)));
                delay_ms = (delay_ms * 2).min(MAX_DELAY_MS);
            }
            Err(e) => {
                return Err(crate::CoordinatorError::LockFailed {
                    message: format!(
                        "failed to acquire mailbox lock at {} after {MAX_RETRIES} retries: {e}",
                        lock_path.display()
                    ),
                });
            }
        }
    }
    unreachable!("retry loop always returns")
}

/// Read the mailbox file without acquiring the lock. Callers that need
/// lock-protected read-modify-write should use this from inside
/// [`with_inbox_lock`] to avoid recursive locking. Callers that just
/// want a point-in-time read should use [`super::io::read_mailbox`]
/// which is lock-free (readers accept slight staleness).
pub(crate) fn read_messages_no_lock(path: &std::path::Path) -> crate::Result<Vec<TeammateMessage>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(path)?;
    if content.trim().is_empty() {
        return Ok(Vec::new());
    }
    Ok(serde_json::from_str(&content)?)
}
