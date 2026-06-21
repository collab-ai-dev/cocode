//! Symlink-aware path resolution for write-validation.
//!
//! Walks up the ancestry until canonicalize succeeds, then
//! re-appends the dropped components. Lets us validate paths whose
//! tail doesn't exist yet (we're about to create them) without
//! choking on partial trees.
//!
//! **Fails closed** on three error classes that the original
//! implementation silently swallowed:
//!
//! - **Dangling symlink** — `path` is a symlink pointing nowhere.
//!   Distinguishes via `lstat` after `realpath` returns ENOENT; we use
//!   `symlink_metadata` for the same effect. A planted
//!   `<team>/x.md -> /etc/passwd` would otherwise pass through.
//! - **ELOOP** — symlink loop. Rejected; we map any
//!   non-`NotFound`/`NotADirectory`/`InvalidFilename` IO error to
//!   `None` so the caller's containment check fails closed.
//! - **EACCES / EIO** — can't verify. Same fail-closed posture as
//!   ELOOP; a security boundary that silently passes on unknown error
//!   is worse than one that occasionally rejects a recoverable case.

use std::io;
use std::path::Path;
use std::path::PathBuf;

/// Resolve symlinks for the deepest existing ancestor of `path`.
///
/// If `path` itself exists, returns its canonical form. Otherwise walks
/// `parent()` until canonicalize succeeds and re-appends the leaf
/// components.
///
/// Returns `None` when:
/// - no ancestor is canonicalizable (effectively never on a normal
///   filesystem), OR
/// - any ancestor's canonicalize hits a non-recoverable error
///   (`PermissionDenied`, `Other`, anything ELOOP-shaped), OR
/// - `path` itself is a **dangling symlink** (detected via
///   `symlink_metadata` after `canonicalize` returns `NotFound`).
pub fn realpath_deepest_existing(path: &Path) -> Option<PathBuf> {
    match path.canonicalize() {
        Ok(real) => return Some(real),
        Err(err) => match err.kind() {
            io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => {
                // Possibly the leaf doesn't exist yet AND/OR a tail
                // component is a dangling symlink. Probe the latter
                // before falling back to the walk-up path.
                if let Ok(meta) = path.symlink_metadata()
                    && meta.file_type().is_symlink()
                {
                    // Dangling symlink at the leaf — fail closed. A
                    // planted symlink whose target was deleted would
                    // otherwise slip past the walk-up reassembly.
                    return None;
                }
            }
            // ELOOP, EACCES, EIO, etc. — fail closed.
            _ => return None,
        },
    }
    let mut remaining: Vec<&std::ffi::OsStr> = Vec::new();
    let mut cursor = path;
    loop {
        let parent = cursor.parent()?;
        if let Some(name) = cursor.file_name() {
            remaining.push(name);
        }
        match parent.canonicalize() {
            Ok(real) => {
                let mut out = real;
                for component in remaining.iter().rev() {
                    out.push(component);
                }
                return Some(out);
            }
            Err(err) => match err.kind() {
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => {
                    // Keep walking up — the parent's parent may exist.
                }
                _ => return None,
            },
        }
        cursor = parent;
    }
}

#[cfg(test)]
#[path = "symlink.test.rs"]
mod tests;
