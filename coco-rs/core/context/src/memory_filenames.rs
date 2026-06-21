//! Memory-file basename matching with case-insensitive lookup.
//!
//! coco-rs supports both `CLAUDE.md` and `AGENTS.md`
//! (cross-ecosystem convention shared with Codex / Cursor / similar
//! agents). Filenames match case-insensitively so `Claude.md`,
//! `agents.md`, `CLAUDE.MD` etc. all load identically across
//! case-sensitive (Linux ext4) and case-insensitive (macOS APFS,
//! Windows NTFS) filesystems.
//!
//! The original only loads files literally named `CLAUDE.md` /
//! `CLAUDE.local.md` with exact case. coco-rs broadens both axes.
//! The `<system-reminder>` template, dedup keys (absolute path
//! strings), and trigger pipeline are unchanged — only filename
//! resolution is broader.

use std::path::Path;
use std::path::PathBuf;

/// Memory-file basenames at directory roots. Matched case-insensitively.
pub const MEMORY_FILE_CANDIDATES: &[&str] = &["CLAUDE.md", "AGENTS.md"];

/// Local (gitignored) variants. Matched case-insensitively.
pub const MEMORY_LOCAL_FILE_CANDIDATES: &[&str] = &["CLAUDE.local.md", "AGENTS.local.md"];

/// Case-insensitively find any of `candidates` directly under `dir`.
///
/// Returns disk-cased absolute paths (so `Claude.md` on disk surfaces
/// as `Claude.md`, not normalized) in deterministic alphabetical order
/// of the lowercased basename — repeated calls on identical trees
/// produce identical sequences.
///
/// When a directory holds more than one matching file with **byte-identical
/// content** (e.g. a `CLAUDE.md` / `AGENTS.md` pair that are exact copies),
/// the duplicates are collapsed to the single file matching the earliest
/// entry in `candidates` — so `CLAUDE.md` wins over `AGENTS.md`. Files whose
/// contents differ are all kept. This runs on every directory read, so the
/// same tree never injects the same memory twice. Files that differ in size
/// are never read for comparison (cheap pre-filter).
///
/// Empty result on read errors (`ENOENT`, `EACCES`, `ENOTDIR`) — defensive
/// read behavior; skips entries that happen to share a candidate name.
/// Only regular files (or symlinks resolving to files) are returned.
pub fn find_memory_files(dir: &Path, candidates: &[&str]) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut hits: Vec<(String, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(String::from) else {
            continue;
        };
        if !candidates.iter().any(|c| name.eq_ignore_ascii_case(c)) {
            continue;
        }
        let path = entry.path();
        // Use fs::metadata (follows symlinks) instead of DirEntry::metadata
        // (which calls symlink_metadata) so a symlink pointing at a file
        // counts as a file. Without this, a workspace where CLAUDE.md is a
        // symlink to a shared rule file would silently drop.
        let is_file = std::fs::metadata(&path)
            .map(|m| m.is_file())
            .unwrap_or(false);
        if !is_file {
            continue;
        }
        hits.push((name.to_ascii_lowercase(), path));
    }

    hits.sort_by(|a, b| a.0.cmp(&b.0));
    let paths: Vec<PathBuf> = hits.into_iter().map(|(_, path)| path).collect();
    dedup_byte_identical(paths, candidates)
}

/// Collapse byte-identical matches in a single directory, keeping the file
/// that matches the earliest `candidates` entry (so a duplicated
/// `CLAUDE.md` / `AGENTS.md` pair survives as `CLAUDE.md`). Survivor order is
/// preserved. Only equal-length files are read, so non-duplicates cost no
/// extra I/O beyond a `stat`.
fn dedup_byte_identical(paths: Vec<PathBuf>, candidates: &[&str]) -> Vec<PathBuf> {
    if paths.len() < 2 {
        return paths;
    }

    // Preference rank: index of the first candidate the basename matches.
    // Lower rank wins (CLAUDE.md is candidates[0]); unmatched sort last.
    let rank = |p: &Path| -> usize {
        p.file_name()
            .and_then(|n| n.to_str())
            .and_then(|name| candidates.iter().position(|c| name.eq_ignore_ascii_case(c)))
            .unwrap_or(usize::MAX)
    };

    let sizes: Vec<Option<u64>> = paths
        .iter()
        .map(|p| std::fs::metadata(p).map(|m| m.len()).ok())
        .collect();

    let mut dropped = vec![false; paths.len()];
    for i in 0..paths.len() {
        for j in (i + 1)..paths.len() {
            if dropped[i] || dropped[j] {
                continue;
            }
            // Cheap pre-filter: different size ⇒ not byte-identical.
            match (sizes[i], sizes[j]) {
                (Some(a), Some(b)) if a == b => {}
                _ => continue,
            }
            let (Ok(ci), Ok(cj)) = (std::fs::read(&paths[i]), std::fs::read(&paths[j])) else {
                continue;
            };
            if ci == cj {
                if rank(&paths[i]) <= rank(&paths[j]) {
                    dropped[j] = true;
                } else {
                    dropped[i] = true;
                }
            }
        }
    }

    paths
        .into_iter()
        .zip(dropped)
        .filter_map(|(path, drop)| (!drop).then_some(path))
        .collect()
}

#[cfg(test)]
#[path = "memory_filenames.test.rs"]
mod tests;
