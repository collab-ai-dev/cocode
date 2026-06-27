//! Single Scanner — the only entry point for listing memory files.
//!
//! Walks the directory, reads only the first 30 lines of each `.md`
//! (enough for frontmatter), sorts by mtime descending, caps at 200 files.

use std::path::Path;
use std::path::PathBuf;
use std::time::SystemTime;

use crate::store::MemoryFrontmatter;
use crate::store::parse_memory_frontmatter;

/// Hard cap on files scanned in a single pass. Anything past this is
/// silently dropped after mtime-sort.
pub const MAX_SCANNED_FILES: usize = 200;

/// Lines read for frontmatter parsing. Prevents loading large bodies
/// when only the manifest fields are needed.
pub const FRONTMATTER_MAX_LINES: usize = 30;

/// One scanned memory file.
#[derive(Debug, Clone)]
pub struct ScannedMemory {
    pub path: PathBuf,
    pub filename: String,
    pub mtime_ms: i64,
    pub size_bytes: i64,
    pub frontmatter: Option<MemoryFrontmatter>,
}

/// Scan a directory for memory files (`*.md` excluding `MEMORY.md`).
///
/// Sorted newest-first by mtime, capped at 200. Errors (unreadable
/// directory, broken symlinks) are swallowed — return an empty vec.
pub fn scan_memory_files(dir: &Path) -> Vec<ScannedMemory> {
    scan_memory_files_with_cancel(dir, None)
}

/// Cancellable variant — bails early when `cancel` flips. A long
/// directory walk can be killed when the caller (recall ranker /
/// extract subagent dispatch) is aborted. `None` means "no
/// cancellation".
///
/// **Recursive** — a topic file at `<memdir>/feedback/testing.md` is visible to
/// both the ranker manifest and the heuristic fallback. `filename`
/// holds the path *relative to* `dir` (e.g. `"feedback/testing.md"`),
/// which is the form the ranker returns in its `selected_memories`
/// list and the lookup HashMap keys on.
///
/// Hidden directories (leading `.`) and `team/` are skipped:
///
/// - `.consolidate-lock` and any other dotfile must not surface as a
///   ranked memory.
/// - The team subtree is a separate memory dir managed by the team-
///   sync subsystem; surfacing those files in the personal recall
///   pool would double-count and bypass the team-vs-personal
///   distinction the system prompt sets up.
pub fn scan_memory_files_with_cancel(
    dir: &Path,
    cancel: Option<&tokio_util::sync::CancellationToken>,
) -> Vec<ScannedMemory> {
    let mut out = Vec::new();
    if !dir.is_dir() {
        return out;
    }
    if cancel.is_some_and(tokio_util::sync::CancellationToken::is_cancelled) {
        return out;
    }
    let walker = walkdir::WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            // Always include the scan root, even if its own name
            // starts with `.` (e.g. when the caller passes a
            // `tempfile::tempdir()` path like `/tmp/.tmpJfFwwR` —
            // `filter_entry` rejecting the root suppresses the
            // entire walk, which the regression tests at
            // `scan.test.rs` hit). At depth ≥ 1, skip hidden + the
            // team subtree.
            if e.depth() == 0 {
                return true;
            }
            let name = e.file_name().to_string_lossy();
            !(name.starts_with('.') || name == "team")
        });
    for entry in walker {
        if cancel.is_some_and(tokio_util::sync::CancellationToken::is_cancelled) {
            return Vec::new();
        }
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path().to_path_buf();
        let basename = match path.file_name().and_then(|s| s.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if !basename.ends_with(".md") || basename == crate::store::ENTRYPOINT_NAME {
            continue;
        }
        // Relative path from the scan root, normalized to forward
        // slashes for cross-platform consistency.
        let filename = match path.strip_prefix(dir) {
            Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
            Err(_) => basename.to_string(),
        };
        let metadata = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let mtime_ms = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let size_bytes = metadata.len() as i64;
        let frontmatter = read_frontmatter_only(&path);
        out.push(ScannedMemory {
            path,
            filename,
            mtime_ms,
            size_bytes,
            frontmatter,
        });
    }
    out.sort_by(|a, b| b.mtime_ms.cmp(&a.mtime_ms));
    out.truncate(MAX_SCANNED_FILES);
    out
}

/// Format scanned memories as a manifest line list for prompt injection.
///
/// One line per file: `- [type] filename (iso-timestamp): description`.
///
/// - The `[type] ` tag is rendered ONLY when frontmatter parsed (with
///   trailing space). Files without frontmatter render as
///   `- filename (iso): desc`.
/// - The `: description` suffix is included only when frontmatter has
///   a non-empty description.
/// - Empty input → empty string (caller decides whether to render the
///   "## Existing memory files" wrapper; an empty list omits the section
///   entirely).
pub fn format_memory_manifest(memories: &[ScannedMemory]) -> String {
    if memories.is_empty() {
        return String::new();
    }
    let mut lines = Vec::with_capacity(memories.len());
    for mem in memories {
        let tag = mem
            .frontmatter
            .as_ref()
            .map(|fm| format!("[{}] ", fm.memory_type.as_str()))
            .unwrap_or_default();
        let ts = format_iso_timestamp(mem.mtime_ms);
        let desc = mem
            .frontmatter
            .as_ref()
            .map(|fm| fm.description.as_str())
            .filter(|s| !s.is_empty());
        let line = match desc {
            Some(d) => format!("- {tag}{} ({ts}): {d}", mem.filename),
            None => format!("- {tag}{} ({ts})", mem.filename),
        };
        lines.push(line);
    }
    lines.join("\n")
}

/// Render an mtime (ms since epoch) as an ISO-8601 UTC timestamp.
fn format_iso_timestamp(mtime_ms: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(mtime_ms)
        .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
        .unwrap_or_else(|| "1970-01-01T00:00:00.000Z".to_string())
}

/// Days since `mtime_ms`. 0 = today, 1 = yesterday, etc.
pub fn memory_age_days(mtime_ms: i64) -> i64 {
    let now_ms = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let diff = now_ms.saturating_sub(mtime_ms);
    if diff <= 0 {
        0
    } else {
        diff / (24 * 60 * 60 * 1000)
    }
}

/// Human-readable age: `today` / `yesterday` / `N days ago`.
pub fn memory_age_string(mtime_ms: i64) -> String {
    match memory_age_days(mtime_ms) {
        0 => "today".to_string(),
        1 => "yesterday".to_string(),
        n => format!("{n} days ago"),
    }
}

/// Stale-memory caveat prepended to surfaced content when the memory
/// is older than one day. The caller inserts the blank line before this
/// text. Returns an empty string for memories ≤1 day old (treat as fresh).
pub fn memory_freshness_text(mtime_ms: i64) -> String {
    let days = memory_age_days(mtime_ms);
    if days <= 1 {
        return String::new();
    }
    format!(
        "This memory is {days} days old. Memories are point-in-time \
         observations, not live state — claims about code behavior or \
         file:line citations may be outdated. Verify against current code \
         before asserting as fact."
    )
}

/// Return mtime in ms since epoch, or `None` if the file is missing.
pub fn file_mtime_ms(path: &Path) -> Option<i64> {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
}

fn read_frontmatter_only(path: &Path) -> Option<MemoryFrontmatter> {
    let content = std::fs::read_to_string(path).ok()?;
    let mut bounded = String::new();
    for (i, line) in content.lines().enumerate() {
        if i >= FRONTMATTER_MAX_LINES {
            break;
        }
        bounded.push_str(line);
        bounded.push('\n');
    }
    parse_memory_frontmatter(&bounded)
}

#[cfg(test)]
#[path = "scan.test.rs"]
mod tests;
