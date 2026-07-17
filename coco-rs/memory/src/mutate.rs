//! User-initiated memory mutations (the `/journey` delete action).
//!
//! Deliberately **not** in `store/` — that module is a documented pure data
//! layer with no I/O. This module performs disk writes on behalf of an explicit
//! user action.
//!
//! Invariant note: `coco-memory`'s documented rule is that the runtime never
//! *auto-regenerates* `MEMORY.md` (it only reads + truncates). A user-initiated
//! prune of a now-dangling index line is a *user mutation*, not autonomous
//! regeneration — the invariant is about background rewrites, so this stays
//! within it. Recorded here so the boundary stays sharp.

use std::path::Path;

use crate::store::ENTRYPOINT_NAME;
use crate::store::parse_index_line;

/// Failure modes of a memory mutation.
#[derive(Debug, thiserror::Error)]
pub enum MemoryMutateError {
    #[error("failed to delete memory file {path}: {source}")]
    RemoveFile {
        path: String,
        source: std::io::Error,
    },
    #[error("failed to rewrite MEMORY.md index: {source}")]
    WriteIndex { source: std::io::Error },
}

/// Delete one memory: remove the memdir-relative topic file and prune any
/// `MEMORY.md` index line whose link target is that filename.
///
/// Idempotent: a missing topic file still prunes the index (so a dangling
/// pointer to an already-deleted file is cleaned up). Blocking I/O — call from
/// `spawn_blocking` in async contexts.
pub fn delete_entry(memdir: &Path, filename: &str) -> Result<(), MemoryMutateError> {
    // 1. Remove the topic file (missing is fine).
    let file_path = memdir.join(filename);
    match std::fs::remove_file(&file_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(MemoryMutateError::RemoveFile {
                path: file_path.display().to_string(),
                source,
            });
        }
    }

    // 2. Prune dangling index lines (best-effort read; only rewrite on change).
    let index_path = memdir.join(ENTRYPOINT_NAME);
    if let Ok(content) = std::fs::read_to_string(&index_path) {
        let pruned = prune_index_lines(&content, filename);
        if pruned != content {
            coco_utils_common::write_atomic(&index_path, pruned)
                .map_err(|source| MemoryMutateError::WriteIndex { source })?;
        }
    }
    Ok(())
}

/// Drop every `MEMORY.md` line that is an index pointer to `filename`, keeping
/// all other lines (headers, prose, other pointers) verbatim.
fn prune_index_lines(content: &str, filename: &str) -> String {
    let had_trailing_newline = content.ends_with('\n');
    let kept: Vec<&str> = content
        .lines()
        .filter(|line| match parse_index_line(line) {
            Some(entry) => entry.file != filename,
            None => true,
        })
        .collect();
    let mut out = kept.join("\n");
    if had_trailing_newline && !out.is_empty() {
        out.push('\n');
    }
    out
}

#[cfg(test)]
#[path = "mutate.test.rs"]
mod tests;
