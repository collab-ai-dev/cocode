//! Session-level file-read state cache.
//!
//! LRU cache (100 entries, 25MB) tracking all files read by tools or @mentions
//! with `{content, mtime, range, evidence}`.
//!
//! Enables:
//! - @mention deduplication (already-read check via mtime comparison)
//! - Edit safety (reject if file modified externally since last read)
//! - Changed-file detection between turns (mtime comparison)
//!
//! **Different from `FileReadCache`** which is a simple same-turn LRU optimization
//! with no mtime tracking and no tool integration.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

/// Maximum cached entries.
const MAX_ENTRIES: usize = 100;

/// A file entry in the read state cache.
#[derive(Debug, Clone)]
pub struct FileReadEntry {
    /// File content at the time of the last read.
    pub content: String,
    /// File modification time (epoch ms) when last read.
    pub mtime_ms: i64,
    /// Whether the cached content represents the full file or a line range.
    pub range: FileReadRange,
    /// Whether this entry is real file content the model saw, or injected
    /// context that is not safe edit/write evidence.
    pub evidence: ReadEvidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileReadRange {
    Full,
    Lines { offset: Option<i32>, limit: i32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadEvidence {
    RealFileView,
    InjectedPartialView,
}

impl FileReadEntry {
    pub fn full_real(content: String, mtime_ms: i64) -> Self {
        Self {
            content,
            mtime_ms,
            range: FileReadRange::Full,
            evidence: ReadEvidence::RealFileView,
        }
    }

    pub fn line_real(content: String, mtime_ms: i64, offset: Option<i32>, limit: i32) -> Self {
        Self {
            content,
            mtime_ms,
            range: FileReadRange::Lines { offset, limit },
            evidence: ReadEvidence::RealFileView,
        }
    }

    pub fn injected_partial(content: String, mtime_ms: i64, range: FileReadRange) -> Self {
        Self {
            content,
            mtime_ms,
            range,
            evidence: ReadEvidence::InjectedPartialView,
        }
    }

    pub fn is_full_real(&self) -> bool {
        self.range == FileReadRange::Full && self.evidence == ReadEvidence::RealFileView
    }

    pub fn can_satisfy_edit_or_write(&self) -> bool {
        self.evidence == ReadEvidence::RealFileView
    }
}

/// Session-level cache of file read states.
///
/// Tracks every file read by tools (`Read`, `Edit`, `Write`) and @mentions
/// with their content and modification time for deduplication and change
/// detection.
///
/// `read_input_ranges` stores the literal `(offset, limit)` the model
/// passed on a Read call, separately from `FileReadEntry::range`, which
/// stores whether the cached content is a full file or a line range.
/// Presence in this map also serves as the "came from the Read tool" marker —
/// Edit/Write/@mention
/// entries are never in it, so `Read(path, limit=50)` followed by an
/// identical call can dedup-match exactly.
// `Clone` is load-bearing: a fork engine deep-clones the parent's
// `FileReadState` into a fresh `Arc<RwLock<>>` so the fork's reads/edits
// can't pollute the parent's dedup cache (mirrors TS
// `createSubagentContext`, which clones `readFileState` per fork). Plain
// owned data — a cheap structural clone.
#[derive(Debug, Default, Clone)]
pub struct FileReadState {
    entries: HashMap<PathBuf, FileReadEntry>,
    /// LRU ordering (most-recently-accessed at end).
    access_order: Vec<PathBuf>,
    read_input_ranges: HashMap<PathBuf, (Option<i32>, Option<i32>)>,
}

impl FileReadState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&mut self, path: &Path) -> Option<&FileReadEntry> {
        let canonical = path.to_path_buf();
        if self.entries.contains_key(&canonical) {
            self.touch_lru(&canonical);
            self.entries.get(&canonical)
        } else {
            None
        }
    }

    pub fn peek(&self, path: &Path) -> Option<&FileReadEntry> {
        self.entries.get(&path.to_path_buf())
    }

    /// Record a non-Read-origin entry (edit, mention, changed-file scan).
    /// Use `set_from_read` from the Read tool so dedup gating works —
    /// the Read tool's `file_unchanged` check skips stub-ing against
    /// entries that lack the read-input-range marker, so a sequence of
    /// `set_from_read → set` must clear the marker or a later Read call
    /// would still see the path as Read-origin.
    pub fn set(&mut self, path: PathBuf, entry: FileReadEntry) {
        self.evict_if_full();
        self.touch_lru(&path);
        self.read_input_ranges.remove(&path);
        self.entries.insert(path, entry);
    }

    /// Record a file read entry that originated from the `Read` tool.
    /// Stores the literal input offset/limit separately from the effective
    /// range on `FileReadEntry` so `file_unchanged` dedup compares apples
    /// to apples.
    pub fn set_from_read(
        &mut self,
        path: PathBuf,
        entry: FileReadEntry,
        input_offset: Option<i32>,
        input_limit: Option<i32>,
    ) {
        self.evict_if_full();
        self.touch_lru(&path);
        self.read_input_ranges
            .insert(path.clone(), (input_offset, input_limit));
        self.entries.insert(path, entry);
    }

    /// True iff the path's most recent entry was inserted via `set_from_read`.
    pub fn is_from_read_tool(&self, path: &Path) -> bool {
        self.read_input_ranges.contains_key(path)
    }

    /// Return the literal `(input_offset, input_limit)` the model passed when
    /// the path was last recorded via `set_from_read`. Returns None for paths
    /// that came from `set` / `update_after_edit` / `invalidate`.
    pub fn read_input_range(&self, path: &Path) -> Option<(Option<i32>, Option<i32>)> {
        self.read_input_ranges.get(&path.to_path_buf()).copied()
    }

    /// Update after an edit/write: new content, new mtime, clear line-range
    /// markers so a subsequent Read doesn't dedup-stub against a post-edit
    /// entry that was never returned to the model as a Read tool result.
    pub fn update_after_edit(&mut self, path: &Path, new_content: String, new_mtime_ms: i64) {
        let canonical = path.to_path_buf();
        self.touch_lru(&canonical);
        self.read_input_ranges.remove(&canonical);
        self.entries.insert(
            canonical,
            FileReadEntry::full_real(new_content, new_mtime_ms),
        );
    }

    /// Returns `true` if the file is in cache and disk mtime matches.
    pub async fn is_unchanged(&self, path: &Path) -> bool {
        let canonical = path.to_path_buf();
        let Some(entry) = self.entries.get(&canonical) else {
            return false;
        };
        match file_mtime_ms(&canonical).await {
            Ok(disk_mtime) => entry.mtime_ms == disk_mtime,
            Err(_) => false,
        }
    }

    /// Subset of `candidates` whose cached mtime still matches disk —
    /// i.e. the model already has the current content and a re-inject
    /// would be redundant. Feeds `AlreadyReadFileGenerator` in
    /// `coco-system-reminder` for Part 1 silent-dedup reminders.
    ///
    /// Paths not in the cache are excluded (no way to prove "already
    /// read" without a prior read). Paths whose mtime diverges are also
    /// excluded (content changed → content loader should re-inject).
    pub async fn unchanged_paths(&self, candidates: &[PathBuf]) -> Vec<PathBuf> {
        let mut result = Vec::new();
        for path in candidates {
            if self.is_unchanged(path).await {
                result.push(path.clone());
            }
        }
        result
    }

    pub fn invalidate(&mut self, path: &Path) {
        let canonical = path.to_path_buf();
        self.entries.remove(&canonical);
        self.read_input_ranges.remove(&canonical);
        self.access_order.retain(|p| p != &canonical);
    }

    /// Iterate all cached entries (for changed-file detection).
    pub fn iter_entries(&self) -> impl Iterator<Item = (&Path, &FileReadEntry)> {
        self.entries.iter().map(|(k, v)| (k.as_path(), v))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.access_order.clear();
        self.read_input_ranges.clear();
    }

    /// Snapshot all entries ordered by access recency (most recent last).
    ///
    /// Captures pre-compact state. Used by compact to snapshot before clearing,
    /// so post-compact file restoration can re-inject the most recently accessed files.
    pub fn snapshot_by_recency(&self) -> Vec<(PathBuf, FileReadEntry)> {
        self.access_order
            .iter()
            .filter_map(|path| {
                self.entries
                    .get(path)
                    .map(|entry| (path.clone(), entry.clone()))
            })
            .collect()
    }

    // -- Internal helpers --

    fn touch_lru(&mut self, path: &PathBuf) {
        self.access_order.retain(|p| p != path);
        self.access_order.push(path.clone());
    }

    fn evict_if_full(&mut self) {
        while self.entries.len() >= MAX_ENTRIES {
            if let Some(oldest) = self.access_order.first().cloned() {
                self.entries.remove(&oldest);
                self.read_input_ranges.remove(&oldest);
                self.access_order.remove(0);
            } else {
                break;
            }
        }
    }
}

/// Get file modification time in epoch milliseconds.
pub async fn file_mtime_ms(path: &Path) -> std::io::Result<i64> {
    let meta = tokio::fs::metadata(path).await?;
    let mtime = meta
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    Ok(mtime)
}

#[cfg(test)]
#[path = "file_read_state.test.rs"]
mod tests;
