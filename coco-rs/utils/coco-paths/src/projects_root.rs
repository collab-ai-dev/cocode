//! Project-root level operations independent of any specific project:
//! `getProjectsDir`, `getProjectDir`, and `findProjectDir` (with the
//! long-path prefix-fallback that handles Bun/Node hash divergence).
//!
//! Most callers should use [`ProjectPaths`](crate::ProjectPaths) for
//! per-project paths. These functions exist for the cross-project
//! discovery case (e.g. `listSessions` walking every project dir).

use std::path::{Path, PathBuf};

use crate::nfc::normalize_nfc;
use crate::sanitize::{MAX_SANITIZED_LENGTH, sanitize_path};

/// `<memory_base>/projects/` — matches TS `getProjectsDir`.
pub fn projects_root(memory_base: &Path) -> PathBuf {
    memory_base.join("projects")
}

/// `<memory_base>/projects/<sanitize_path(normalize_nfc(project_path))>/`.
///
/// Pure path computation — no filesystem access. Use
/// [`find_project_dir`] when you need to handle long-path hash
/// mismatches.
pub fn project_dir(memory_base: &Path, project_path: &Path) -> PathBuf {
    let raw = project_path.to_string_lossy();
    let nfc = normalize_nfc(&raw);
    let slug = sanitize_path(&nfc);
    projects_root(memory_base).join(slug)
}

/// Locate the on-disk project directory for `project_path`.
///
/// Returns:
/// - `Ok(Some(path))` when an exact-slug directory exists.
/// - `Ok(Some(path))` when the sanitized slug overflows
///   [`MAX_SANITIZED_LENGTH`] (i.e. its real disk name has a djb2
///   suffix) and a directory whose name starts with
///   `<prefix>-` exists. This is the long-path prefix fallback that
///   handles Bun-vs-Node hash divergence — different runtimes can
///   produce different suffixes for the same input, but they share
///   the truncated prefix.
/// - `Ok(None)` when neither exact match nor prefix-match is found.
/// - `Err` on I/O errors that aren't "not found".
pub fn find_project_dir(
    memory_base: &Path,
    project_path: &Path,
) -> std::io::Result<Option<PathBuf>> {
    let exact = project_dir(memory_base, project_path);
    match std::fs::metadata(&exact) {
        Ok(m) if m.is_dir() => return Ok(Some(exact)),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }

    let raw = project_path.to_string_lossy();
    let nfc = normalize_nfc(&raw);
    let sanitized = sanitize_path(&nfc);
    if sanitized.len() <= MAX_SANITIZED_LENGTH {
        // Short slug: there is no hash suffix to disagree about; the
        // exact lookup above is authoritative.
        return Ok(None);
    }

    // Slug is `{200_byte_prefix}-{djb2_hash}`. Scan the projects
    // root for any directory starting with `{prefix}-`.
    let prefix = &sanitized[..MAX_SANITIZED_LENGTH];
    let needle = format!("{prefix}-");
    let root = projects_root(memory_base);
    let entries = match std::fs::read_dir(&root) {
        Ok(it) => it,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let Some(s) = name.to_str() else { continue };
        if s.starts_with(&needle) {
            return Ok(Some(root.join(s)));
        }
    }
    Ok(None)
}

#[cfg(test)]
#[path = "projects_root.test.rs"]
mod tests;
