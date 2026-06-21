//! Prompt history persistence via JSONL.
//!
//! JSONL append-only log at `~/.coco/history.jsonl`.
//! Entries are project-scoped, session-tagged, newest-first on read.

use serde::Deserialize;
use serde::Serialize;
use sha2::Digest;
use sha2::Sha256;
use std::collections::HashMap;
use std::collections::HashSet;
use std::io::BufRead;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;

const MAX_HISTORY_ITEMS: usize = 100;
/// Pastes shorter than this are stored inline; longer ones go to the
/// content-addressed paste store.
const MAX_PASTED_CONTENT_LENGTH: usize = 1024;

/// A single history log entry (serialized to JSONL).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryLogEntry {
    /// Display text shown in up-arrow history.
    pub display: String,
    /// Pasted content references (id → content).
    #[serde(default)]
    pub pasted_contents: HashMap<i32, PastedContentRef>,
    /// Unix timestamp (milliseconds).
    pub timestamp: i64,
    /// Project root path.
    pub project: String,
    /// Session ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// A pasted content reference (stored inline for small, hash for large).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PastedContentRef {
    pub id: i32,
    #[serde(rename = "type")]
    pub content_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
}

/// A resolved history entry for display.
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub display: String,
    pub timestamp: i64,
    pub pasted_contents: HashMap<i32, String>,
}

/// A history entry that resolves paste content lazily on demand —
/// used by the ctrl+r picker which renders display + timestamp eagerly
/// but defers paste-store reads until the user accepts a row.
pub struct TimestampedHistoryEntry {
    pub display: String,
    pub timestamp: i64,
    /// Resolves to the full HistoryEntry on demand. Sync because
    /// the paste store is a small file read.
    pub resolve: Box<dyn FnOnce() -> HistoryEntry + Send>,
}

/// Prompt history manager.
pub struct PromptHistory {
    history_path: PathBuf,
    paste_store_dir: PathBuf,
    project: String,
    session_id: String,
    /// Timestamp of the last `add()` call. Used by
    /// `remove_last_from_history` so an Esc-driven auto-restore can
    /// undo the just-flushed entry.
    last_added: Mutex<Option<i64>>,
    /// Timestamps that should be skipped when reading (in-memory
    /// skip set for undo-on-interrupt).
    skipped: Mutex<HashSet<i64>>,
}

impl PromptHistory {
    /// Create a new history manager.
    pub fn new(config_dir: &Path, project: &str, session_id: &str) -> Self {
        Self {
            history_path: config_dir.join("history.jsonl"),
            paste_store_dir: config_dir.join("paste-store"),
            project: project.to_string(),
            session_id: session_id.to_string(),
            last_added: Mutex::new(None),
            skipped: Mutex::new(HashSet::new()),
        }
    }

    /// Add an entry (no pasted content).
    pub fn add(&self, display: &str) -> crate::Result<()> {
        self.add_with_pastes(display, &HashMap::new())
    }

    /// Add an entry with optional pasted content. Pastes longer than
    /// `MAX_PASTED_CONTENT_LENGTH` are stored externally and
    /// referenced by SHA-256 hash; shorter pastes are stored inline.
    pub fn add_with_pastes(
        &self,
        display: &str,
        pasted_contents: &HashMap<i32, String>,
    ) -> crate::Result<()> {
        let mut stored: HashMap<i32, PastedContentRef> = HashMap::new();
        for (id, content) in pasted_contents {
            if content.len() <= MAX_PASTED_CONTENT_LENGTH {
                stored.insert(
                    *id,
                    PastedContentRef {
                        id: *id,
                        content_type: "text".into(),
                        content: Some(content.clone()),
                        content_hash: None,
                    },
                );
            } else {
                let hash = hash_paste(content);
                self.write_paste(&hash, content)?;
                stored.insert(
                    *id,
                    PastedContentRef {
                        id: *id,
                        content_type: "text".into(),
                        content: None,
                        content_hash: Some(hash),
                    },
                );
            }
        }

        let timestamp = current_timestamp_ms();
        let entry = HistoryLogEntry {
            display: display.to_string(),
            pasted_contents: stored,
            timestamp,
            project: self.project.clone(),
            session_id: Some(self.session_id.clone()),
        };

        if let Some(parent) = self.history_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Acquire an OS-level advisory file lock to serialize concurrent
        // PromptHistory writers (multiple coco processes against the
        // same `~/.coco/history.jsonl`). Pure-Rust via the `fs2`
        // workspace dep — TS uses `proper-lockfile` with retries;
        // `fs2::FileExt::lock_exclusive` blocks until acquired and
        // releases on drop.
        use fs2::FileExt;
        let lock_path = self.history_path.with_extension("jsonl.lock");
        let lock_file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)?;
        let _ = lock_file.lock_exclusive();

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.history_path)?;

        let line = serde_json::to_string(&entry)?;
        writeln!(file, "{line}")?;
        if let Ok(mut last) = self.last_added.lock() {
            *last = Some(timestamp);
        }
        Ok(())
    }

    /// Undo the most recent `add()` for the current session.
    ///
    /// Used by auto-restore-on-interrupt: an Esc immediately after a
    /// submit semantically undoes the prompt, so the JSONL entry
    /// should also be undone or the up-arrow shows the restored
    /// text twice.
    pub fn remove_last_from_history(&self) {
        let ts = match self.last_added.lock() {
            Ok(mut l) => l.take(),
            Err(_) => return,
        };
        if let (Some(ts), Ok(mut s)) = (ts, self.skipped.lock()) {
            s.insert(ts);
        }
    }

    /// Read history entries for the current project, newest first.
    ///
    /// Current session entries come first, then other sessions.
    /// Limited to MAX_HISTORY_ITEMS total.
    pub fn get_history(&self) -> Vec<HistoryEntry> {
        let entries = match self.read_all_entries() {
            Ok(e) => e,
            Err(_) => return Vec::new(),
        };

        let skipped = self.skipped.lock().map(|s| s.clone()).unwrap_or_default();
        let mut current_session = Vec::new();
        let mut other_sessions = Vec::new();

        for entry in entries.into_iter().rev() {
            if entry.project != self.project {
                continue;
            }
            // Drop entries removed by `remove_last_from_history`
            // when they have raced past the in-memory buffer.
            if entry.session_id.as_deref() == Some(&self.session_id)
                && skipped.contains(&entry.timestamp)
            {
                continue;
            }
            if entry.session_id.as_deref() == Some(&self.session_id) {
                current_session.push(self.to_history_entry(&entry));
            } else {
                other_sessions.push(self.to_history_entry(&entry));
            }
            if current_session.len() + other_sessions.len() >= MAX_HISTORY_ITEMS {
                break;
            }
        }

        // Current session first, then others
        current_session.extend(other_sessions);
        current_session.truncate(MAX_HISTORY_ITEMS);
        current_session
    }

    /// Read project-scoped history for the ctrl+r picker.
    ///
    /// Yields (display, timestamp, lazy-resolver) triples newest-first
    /// deduped by display. The resolver fetches paste-store contents
    /// only when invoked.
    pub fn get_timestamped_history(&self) -> Vec<TimestampedHistoryEntry> {
        let entries = match self.read_all_entries() {
            Ok(e) => e,
            Err(_) => return Vec::new(),
        };
        let mut seen: HashSet<String> = HashSet::new();
        let mut out: Vec<TimestampedHistoryEntry> = Vec::new();
        let paste_store_dir = self.paste_store_dir.clone();
        for entry in entries.into_iter().rev() {
            if entry.project != self.project {
                continue;
            }
            if !seen.insert(entry.display.clone()) {
                continue;
            }
            let display = entry.display.clone();
            let timestamp = entry.timestamp;
            let entry_clone = entry.clone();
            let dir = paste_store_dir.clone();
            out.push(TimestampedHistoryEntry {
                display: display.clone(),
                timestamp,
                resolve: Box::new(move || resolve_pastes_for(&entry_clone, &dir)),
            });
            if out.len() >= MAX_HISTORY_ITEMS {
                break;
            }
        }
        out
    }

    /// Resolve a HistoryLogEntry's pasted-content refs into inline
    /// strings, fetching from the paste store as needed.
    fn to_history_entry(&self, entry: &HistoryLogEntry) -> HistoryEntry {
        resolve_pastes_for(entry, &self.paste_store_dir)
    }

    /// Read all log entries from the JSONL file.
    fn read_all_entries(&self) -> crate::Result<Vec<HistoryLogEntry>> {
        if !self.history_path.exists() {
            return Ok(Vec::new());
        }
        let file = std::fs::File::open(&self.history_path)?;
        let reader = std::io::BufReader::new(file);
        let mut entries = Vec::new();
        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(entry) = serde_json::from_str::<HistoryLogEntry>(&line) {
                entries.push(entry);
            }
        }
        Ok(entries)
    }

    /// Write a paste blob to the content-addressed store.
    fn write_paste(&self, hash: &str, content: &str) -> crate::Result<()> {
        std::fs::create_dir_all(&self.paste_store_dir)?;
        let path = self.paste_store_dir.join(format!("{hash}.txt"));
        if path.exists() {
            return Ok(());
        }
        std::fs::write(&path, content)?;
        Ok(())
    }
}

fn resolve_pastes_for(entry: &HistoryLogEntry, paste_store_dir: &Path) -> HistoryEntry {
    let mut pasted = HashMap::new();
    for (id, ref_entry) in &entry.pasted_contents {
        if let Some(content) = &ref_entry.content {
            pasted.insert(*id, content.clone());
            continue;
        }
        if let Some(hash) = &ref_entry.content_hash {
            let path = paste_store_dir.join(format!("{hash}.txt"));
            if let Ok(content) = std::fs::read_to_string(&path) {
                pasted.insert(*id, content);
            }
        }
    }
    HistoryEntry {
        display: entry.display.clone(),
        timestamp: entry.timestamp,
        pasted_contents: pasted,
    }
}

fn hash_paste(content: &str) -> String {
    let digest = Sha256::digest(content.as_bytes());
    let mut s = String::with_capacity(digest.len() * 2);
    for b in digest.iter() {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn current_timestamp_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// Format a pasted text reference.
pub fn format_pasted_text_ref(id: i32, num_lines: i32) -> String {
    if num_lines == 0 {
        format!("[Pasted text #{id}]")
    } else {
        format!("[Pasted text #{id} +{num_lines} lines]")
    }
}

/// Format an image reference.
pub fn format_image_ref(id: i32) -> String {
    format!("[Image #{id}]")
}

/// Replace `[Pasted text #N]` placeholders in `input` with their
/// actual content from `pasted_contents`. Image refs are left alone
/// — they become content blocks, not inlined text.
///
/// Splices at the regex match offsets so placeholder-like strings
/// inside pasted content are never confused for real refs. Reverse
/// order keeps earlier offsets valid after later replacements.
pub fn expand_pasted_text_refs(input: &str, pasted_contents: &HashMap<i32, String>) -> String {
    // Match `[Pasted text #N (+M lines)?]`. Image / truncated
    // variants are intentionally not expanded.
    let mut matches: Vec<(usize, usize, i32)> = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0usize;
    while i + 14 <= bytes.len() {
        if !input[i..].starts_with("[Pasted text #") {
            i += 1;
            continue;
        }
        let start = i;
        let id_start = i + "[Pasted text #".len();
        let mut j = id_start;
        while j < bytes.len() && (bytes[j] as char).is_ascii_digit() {
            j += 1;
        }
        if j == id_start {
            i += 1;
            continue;
        }
        let id: i32 = match input[id_start..j].parse() {
            Ok(n) => n,
            Err(_) => {
                i = j;
                continue;
            }
        };
        // Optional "+N lines" suffix, then ']'.
        let mut k = j;
        if input[k..].starts_with(" +") {
            k += 2;
            while k < bytes.len() && (bytes[k] as char).is_ascii_digit() {
                k += 1;
            }
            if !input[k..].starts_with(" lines") {
                i = j;
                continue;
            }
            k += " lines".len();
        }
        if !input[k..].starts_with(']') {
            i = j;
            continue;
        }
        let end = k + 1;
        matches.push((start, end, id));
        i = end;
    }

    let mut out = input.to_string();
    for (start, end, id) in matches.into_iter().rev() {
        if let Some(content) = pasted_contents.get(&id) {
            out.replace_range(start..end, content);
        }
    }
    out
}

#[cfg(test)]
#[path = "history.test.rs"]
mod tests;
