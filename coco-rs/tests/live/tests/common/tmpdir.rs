//! Test tempdir anchored at `/tmp` (Linux/macOS) so prompt text that
//! mentions absolute paths reads obviously as scratch space — not "in
//! the project directory".
//!
//! macOS's default `tempfile::tempdir()` lands at
//! `/var/folders/<rand>/T/.tmpXXXX`, which is functionally a tempdir
//! but visually opaque. Linux defaults to `/tmp` already; either way
//! pinning to `/tmp` makes review unambiguous.

use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use tempfile::TempDir;

const ROOT: &str = "/tmp";

/// Create a tempdir under `/tmp/<prefix><rand>`. Equivalent to
/// `tempfile::Builder::new().prefix(prefix).tempdir_in("/tmp")`.
///
/// The directory is removed when the returned `TempDir` is dropped.
pub fn make(prefix: &str) -> Result<TempDir> {
    // Best-effort `mkdir -p /tmp` (every supported platform has this
    // path; the call is harmless when it already exists).
    let _ = std::fs::create_dir_all(ROOT);
    tempfile::Builder::new()
        .prefix(prefix)
        .tempdir_in(ROOT)
        .with_context(|| format!("create tempdir under {ROOT} (prefix={prefix})"))
}

/// Convenience: return the resolved path as `PathBuf` for callers that
/// want to embed it in a prompt without holding `TempDir` directly.
/// **Caller is responsible for cleanup** — use `make()` unless you
/// know what you're doing.
#[allow(dead_code)]
pub fn make_persistent(prefix: &str) -> Result<PathBuf> {
    let dir = make(prefix)?;
    Ok(dir.keep())
}
