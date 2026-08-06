//! Atomic file writes.

use std::io::Write as _;
use std::path::Path;

/// Write `contents` to `path` atomically — never leaves a truncated or
/// half-written file on disk. Writes through a sibling
/// [`tempfile::NamedTempFile`] in the target directory (a cross-filesystem
/// rename is a no-go) and `persist`s via atomic rename, creating parent
/// directories as needed. Readers (file watchers, concurrent loads) observe
/// either the old file or the complete new one.
pub fn write_atomic(path: &Path, contents: impl AsRef<[u8]>) -> std::io::Result<()> {
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    std::fs::create_dir_all(parent)?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(contents.as_ref())?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

/// File modification time in epoch milliseconds.
///
/// Lives here rather than beside the read-state cache that consumes it:
/// `coco-types` holds types, and an async stat there would put `tokio::fs`
/// on every crate that depends on it.
pub async fn file_mtime_ms(path: &Path) -> std::io::Result<i64> {
    let meta = tokio::fs::metadata(path).await?;
    let mtime = meta
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    Ok(mtime)
}
