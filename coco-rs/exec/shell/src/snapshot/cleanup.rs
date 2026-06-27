//! Stale shell-snapshot cleanup.
//!
//! Two parts handle cleanup:
//!
//! - **`Drop` on [`ShellSnapshot`](super::ShellSnapshot)** — best-effort
//!   unlink for the current session's own file when the handle drops.
//!
//! - **`cleanup_stale_snapshots`** (this fn) — called at session start to
//!   sweep any file older than `retention` that was left behind by a
//!   previous run that crashed before `Drop` could fire.
//!
//! With the naming scheme `snapshot-<shell>-<ts>-<rand>.<ext>`, every
//! filename is unique across sessions, so we no longer need to special-case
//! "the active session's file" — mtime alone tells us what's stale.

use std::io::ErrorKind;
use std::path::Path;
use std::time::Duration;
use std::time::SystemTime;

use anyhow::Result;
use tokio::fs;

/// Remove shell-snapshot files older than `retention`.
///
/// `active_session_id` is accepted for API back-compat (older callers
/// expected per-session protection); it's currently unused because the
/// new naming scheme makes mtime alone sufficient. Returns the number
/// of files removed.
pub async fn cleanup_stale_snapshots(
    snapshot_dir: &Path,
    active_session_id: &str,
    retention: Duration,
) -> Result<i32> {
    let _ = active_session_id;

    let mut entries = match fs::read_dir(snapshot_dir).await {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(err.into()),
    };

    let now = SystemTime::now();
    let mut removed_count = 0;

    while let Some(entry) = entries.next_entry().await? {
        if !entry.file_type().await?.is_file() {
            continue;
        }
        let path = entry.path();
        let modified = match fs::metadata(&path).await.and_then(|m| m.modified()) {
            Ok(m) => m,
            Err(err) => {
                tracing::warn!(
                    "Failed to check snapshot age for {}: {err:?}",
                    path.display()
                );
                continue;
            }
        };
        if let Ok(age) = now.duration_since(modified)
            && age >= retention
        {
            remove_snapshot_file(&path).await;
            removed_count += 1;
        }
    }

    Ok(removed_count)
}

async fn remove_snapshot_file(path: &Path) {
    match fs::remove_file(path).await {
        Ok(()) => tracing::debug!("Removed stale snapshot: {}", path.display()),
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => tracing::warn!("Failed to delete shell snapshot at {:?}: {err:?}", path),
    }
}

#[cfg(test)]
#[path = "cleanup.test.rs"]
mod tests;
