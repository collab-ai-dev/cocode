//! Prompt history persistence via JSONL.
//!
//! JSONL append-only log at `config home/history.jsonl`.
//! Entries are project-scoped, session-tagged, newest-first on read.

use serde::Deserialize;
use serde::Serialize;
use sha2::Digest;
use sha2::Sha256;
use std::collections::HashSet;
use std::io::BufRead;
use std::io::Seek;
use std::io::SeekFrom;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use coco_types::SessionId;

const MAX_HISTORY_ITEMS: usize = 100;
const MAX_STORED_HISTORY_ITEMS: usize = 500;
const MAX_INLINE_TEXT_BYTES: usize = 1024;
const MAX_ATTACHMENT_BLOB_BYTES: usize = 10 * 1024 * 1024;
const MAX_ATTACHMENT_BLOB_FILE_BYTES: u64 = 10 * 1024 * 1024;
const MAX_ATTACHMENT_STORE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_RESOLVED_HISTORY_BYTES: usize = 64 * 1024 * 1024;
const MAX_COMPOSER_TEXT_BYTES: usize = 10 * 1024 * 1024;
const MAX_COMPOSER_ATTACHMENT_BYTES: usize = 64 * 1024 * 1024;
const MAX_COMPOSER_ELEMENTS: usize = 4096;
const MAX_HISTORY_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_HISTORY_FILE_TAIL_BYTES: i64 = 64 * 1024 * 1024;

/// A single history log entry (serialized to JSONL).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct HistoryLogEntry {
    composer: StoredComposer,
    /// Unix timestamp (milliseconds).
    timestamp: i64,
    /// Project root path.
    project: String,
    /// Session ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_id: Option<SessionId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredComposer {
    text: String,
    next_attachment_label: i64,
    elements: Vec<StoredComposerElement>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredComposerElement {
    Paste {
        start: i64,
        end: i64,
        content: StoredText,
    },
    Image {
        start: i64,
        end: i64,
        media_type: String,
        content_hash: String,
    },
    FileRef {
        start: i64,
        end: i64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredText {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    inline: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    content_hash: Option<String>,
}

/// A resolved history entry for display.
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub composer: coco_types::PersistedComposer,
    pub timestamp: i64,
}

/// A history entry that resolves paste content lazily on demand —
/// used by the ctrl+r picker which renders display + timestamp eagerly
/// but defers paste-store reads until the user accepts a row.
pub struct TimestampedHistoryEntry {
    pub composer: coco_types::PersistedComposer,
    pub timestamp: i64,
    /// Resolves to the full HistoryEntry on demand. Sync because
    /// the paste store is a small file read.
    pub resolve: Box<dyn FnOnce() -> Option<HistoryEntry> + Send>,
}

/// Prompt history manager.
pub struct PromptHistory {
    history_path: PathBuf,
    attachment_store_dir: PathBuf,
    project: String,
    session_id: SessionId,
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
    pub fn new(config_dir: &Path, project: &str, session_id: &SessionId) -> Self {
        Self {
            history_path: config_dir.join("history.jsonl"),
            attachment_store_dir: config_dir.join("composer-store"),
            project: project.to_string(),
            session_id: session_id.clone(),
            last_added: Mutex::new(None),
            skipped: Mutex::new(HashSet::new()),
        }
    }

    /// Add an entry (no pasted content).
    pub fn add(&self, text: &str) -> crate::Result<()> {
        self.add_composer(&coco_types::PersistedComposer {
            text: text.to_string(),
            next_attachment_label: 0,
            elements: Vec::new(),
        })
    }

    pub fn add_composer(&self, composer: &coco_types::PersistedComposer) -> crate::Result<()> {
        validate_composer_shape(composer)?;
        let timestamp = current_timestamp_ms();

        if let Some(parent) = self.history_path.parent() {
            std::fs::create_dir_all(parent)?;
            restrict_dir_permissions(parent)?;
        }

        // Acquire an OS-level advisory file lock to serialize concurrent
        // PromptHistory writers (multiple coco processes against the
        // same `config home/history.jsonl`). Pure-Rust via the `fs2`
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
        restrict_file_permissions(&lock_path)?;
        lock_file.lock_exclusive()?;

        self.compact_history_and_store()?;
        let entry = HistoryLogEntry {
            composer: self.store_composer(composer)?,
            timestamp,
            project: self.project.clone(),
            session_id: Some(self.session_id.clone()),
        };

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.history_path)?;
        restrict_file_permissions(&self.history_path)?;

        let line = serde_json::to_string(&entry)?;
        writeln!(file, "{line}")?;
        file.sync_data()?;
        drop(file);
        self.compact_history_and_store()?;
        if let Ok(mut last) = self.last_added.lock() {
            *last = Some(timestamp);
        }
        Ok(())
    }

    fn store_composer(
        &self,
        composer: &coco_types::PersistedComposer,
    ) -> crate::Result<StoredComposer> {
        use base64::Engine as _;

        let mut elements = Vec::with_capacity(composer.elements.len());
        let mut attachment_bytes = 0usize;
        for element in &composer.elements {
            let stored = match element {
                coco_types::PersistedComposerElement::Paste {
                    start,
                    end,
                    content,
                } => {
                    attachment_bytes = attachment_bytes
                        .checked_add(content.len())
                        .filter(|total| *total <= MAX_COMPOSER_ATTACHMENT_BYTES)
                        .ok_or_else(|| {
                            crate::SessionError::generic(
                                "composer attachments exceed persistent history size limit",
                            )
                        })?;
                    let content = if content.len() <= MAX_INLINE_TEXT_BYTES {
                        StoredText {
                            inline: Some(content.clone()),
                            content_hash: None,
                        }
                    } else {
                        let hash = hash_bytes(content.as_bytes());
                        self.write_blob(&hash, content.as_bytes())?;
                        StoredText {
                            inline: None,
                            content_hash: Some(hash),
                        }
                    };
                    StoredComposerElement::Paste {
                        start: *start,
                        end: *end,
                        content,
                    }
                }
                coco_types::PersistedComposerElement::Image {
                    start,
                    end,
                    media_type,
                    data_base64,
                } => {
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(data_base64)
                        .map_err(|error| crate::SessionError::generic(error.to_string()))?;
                    attachment_bytes = attachment_bytes
                        .checked_add(bytes.len())
                        .filter(|total| *total <= MAX_COMPOSER_ATTACHMENT_BYTES)
                        .ok_or_else(|| {
                            crate::SessionError::generic(
                                "composer attachments exceed persistent history size limit",
                            )
                        })?;
                    let hash = hash_bytes(&bytes);
                    self.write_blob(&hash, &bytes)?;
                    StoredComposerElement::Image {
                        start: *start,
                        end: *end,
                        media_type: media_type.clone(),
                        content_hash: hash,
                    }
                }
                coco_types::PersistedComposerElement::FileRef { start, end } => {
                    StoredComposerElement::FileRef {
                        start: *start,
                        end: *end,
                    }
                }
            };
            elements.push(stored);
        }
        Ok(StoredComposer {
            text: composer.text.clone(),
            next_attachment_label: composer.next_attachment_label,
            elements,
        })
    }

    fn write_blob(&self, hash: &str, bytes: &[u8]) -> crate::Result<()> {
        if bytes.len() > MAX_ATTACHMENT_BLOB_BYTES {
            return Err(crate::SessionError::generic(
                "composer attachment exceeds persistent history size limit",
            ));
        }
        std::fs::create_dir_all(&self.attachment_store_dir)?;
        restrict_dir_permissions(&self.attachment_store_dir)?;
        let path = self.attachment_store_dir.join(hash);
        if path.exists() {
            if std::fs::metadata(&path)?.len() > MAX_ATTACHMENT_BLOB_FILE_BYTES {
                return Err(crate::SessionError::generic(
                    "composer attachment store blob exceeds size limit",
                ));
            }
            let existing = std::fs::read(&path)?;
            if hash_bytes(&existing) != hash {
                return Err(crate::SessionError::generic(
                    "composer attachment store hash mismatch",
                ));
            }
            return Ok(());
        }
        let temp_path = self
            .attachment_store_dir
            .join(format!(".{hash}.{}.tmp", uuid::Uuid::new_v4()));
        let write_result = (|| -> std::io::Result<()> {
            let mut temp = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temp_path)?;
            temp.write_all(bytes)?;
            temp.sync_all()
        })();
        if let Err(error) = write_result {
            let _ = std::fs::remove_file(&temp_path);
            return Err(error.into());
        }
        if let Err(error) = std::fs::rename(&temp_path, &path) {
            let _ = std::fs::remove_file(&temp_path);
            if !path.exists() {
                return Err(error.into());
            }
            let existing = std::fs::read(&path)?;
            if hash_bytes(&existing) != hash {
                return Err(crate::SessionError::generic(
                    "composer attachment store hash mismatch",
                ));
            }
        }
        restrict_file_permissions(&path)?;
        Ok(())
    }

    fn compact_history_and_store(&self) -> crate::Result<()> {
        let entries = self.read_all_entries()?;
        let mut retained = Vec::new();
        let mut hashes = HashSet::new();
        let mut stored_bytes = 0u64;
        for entry in entries.iter().rev() {
            if retained.len() == MAX_STORED_HISTORY_ITEMS {
                break;
            }
            let entry_hashes = stored_composer_hashes(&entry.composer);
            let additional_bytes = entry_hashes
                .iter()
                .filter(|hash| !hashes.contains(*hash))
                .filter_map(|hash| {
                    std::fs::metadata(self.attachment_store_dir.join(hash))
                        .ok()
                        .map(|metadata| metadata.len())
                })
                .try_fold(0u64, u64::checked_add)
                .unwrap_or(u64::MAX);
            if stored_bytes.saturating_add(additional_bytes) > MAX_ATTACHMENT_STORE_BYTES {
                continue;
            }
            stored_bytes += additional_bytes;
            hashes.extend(entry_hashes);
            retained.push(entry.clone());
        }
        retained.reverse();

        if retained.len() != entries.len() {
            let temp_path = self
                .history_path
                .with_extension(format!("jsonl.{}.tmp", uuid::Uuid::new_v4()));
            let rewrite = (|| -> crate::Result<()> {
                let mut file = std::fs::OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(&temp_path)?;
                for entry in &retained {
                    writeln!(file, "{}", serde_json::to_string(entry)?)?;
                }
                file.sync_all()?;
                std::fs::rename(&temp_path, &self.history_path)?;
                restrict_file_permissions(&self.history_path)?;
                Ok(())
            })();
            if let Err(error) = rewrite {
                let _ = std::fs::remove_file(&temp_path);
                return Err(error);
            }
        }

        if let Ok(files) = std::fs::read_dir(&self.attachment_store_dir) {
            for file in files.flatten() {
                let name = file.file_name();
                let Some(name) = name.to_str() else {
                    continue;
                };
                if is_content_hash(name) && !hashes.contains(name) {
                    let _ = std::fs::remove_file(file.path());
                }
            }
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
        let resolved_bytes = AtomicUsize::new(0);

        for entry in entries.into_iter().rev() {
            if entry.project != self.project {
                continue;
            }
            // Drop entries removed by `remove_last_from_history`
            // when they have raced past the in-memory buffer.
            if entry.session_id.as_ref() == Some(&self.session_id)
                && skipped.contains(&entry.timestamp)
            {
                continue;
            }
            if entry.session_id.as_ref() == Some(&self.session_id) {
                if let Some(entry) = self.to_history_entry(&entry, &resolved_bytes) {
                    current_session.push(entry);
                }
            } else if let Some(entry) = self.to_history_entry(&entry, &resolved_bytes) {
                other_sessions.push(entry);
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
    /// Yields typed composer snapshots newest-first, deduped by the complete
    /// stored composer shape.
    pub fn get_timestamped_history(&self) -> Vec<TimestampedHistoryEntry> {
        let entries = match self.read_all_entries() {
            Ok(e) => e,
            Err(_) => return Vec::new(),
        };
        let mut seen: HashSet<String> = HashSet::new();
        let mut out: Vec<TimestampedHistoryEntry> = Vec::new();
        let store_dir = self.attachment_store_dir.clone();
        let resolved_bytes = std::sync::Arc::new(AtomicUsize::new(0));
        for entry in entries.into_iter().rev() {
            if entry.project != self.project {
                continue;
            }
            let fingerprint = serde_json::to_string(&entry.composer).unwrap_or_default();
            if !seen.insert(fingerprint) {
                continue;
            }
            let timestamp = entry.timestamp;
            let entry_clone = entry.clone();
            let entry_store_dir = store_dir.clone();
            let entry_resolved_bytes = std::sync::Arc::clone(&resolved_bytes);
            out.push(TimestampedHistoryEntry {
                composer: coco_types::PersistedComposer {
                    text: entry.composer.text.clone(),
                    next_attachment_label: entry.composer.next_attachment_label,
                    elements: Vec::new(),
                },
                timestamp,
                resolve: Box::new(move || {
                    resolve_stored_composer(
                        &entry_clone.composer,
                        &entry_store_dir,
                        &entry_resolved_bytes,
                    )
                    .map(|composer| HistoryEntry {
                        composer,
                        timestamp: entry_clone.timestamp,
                    })
                }),
            });
            if out.len() >= MAX_HISTORY_ITEMS {
                break;
            }
        }
        out
    }

    fn to_history_entry(
        &self,
        entry: &HistoryLogEntry,
        resolved_bytes: &AtomicUsize,
    ) -> Option<HistoryEntry> {
        Some(HistoryEntry {
            composer: resolve_stored_composer(
                &entry.composer,
                &self.attachment_store_dir,
                resolved_bytes,
            )?,
            timestamp: entry.timestamp,
        })
    }

    /// Read all log entries from the JSONL file.
    fn read_all_entries(&self) -> crate::Result<Vec<HistoryLogEntry>> {
        if !self.history_path.exists() {
            return Ok(Vec::new());
        }
        let mut file = std::fs::File::open(&self.history_path)?;
        let file_len = file.metadata()?.len();
        let starts_mid_line = file_len > MAX_HISTORY_FILE_BYTES;
        if starts_mid_line {
            file.seek(SeekFrom::End(-MAX_HISTORY_FILE_TAIL_BYTES))?;
        }
        let mut reader = std::io::BufReader::new(file);
        if starts_mid_line {
            let mut partial = Vec::new();
            reader.read_until(b'\n', &mut partial)?;
        }
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
}

fn resolve_stored_composer(
    stored: &StoredComposer,
    store_dir: &Path,
    resolved_bytes: &AtomicUsize,
) -> Option<coco_types::PersistedComposer> {
    use base64::Engine as _;

    let mut elements = Vec::with_capacity(stored.elements.len());
    for element in &stored.elements {
        let resolved = match element {
            StoredComposerElement::Paste {
                start,
                end,
                content,
            } => {
                let content = match (&content.inline, &content.content_hash) {
                    (Some(content), None) => content.clone(),
                    (None, Some(hash)) => {
                        let bytes = read_verified_blob(store_dir, hash, resolved_bytes)?;
                        String::from_utf8(bytes).ok()?
                    }
                    (Some(_), Some(_)) | (None, None) => return None,
                };
                coco_types::PersistedComposerElement::Paste {
                    start: *start,
                    end: *end,
                    content,
                }
            }
            StoredComposerElement::Image {
                start,
                end,
                media_type,
                content_hash,
            } => {
                let bytes = read_verified_blob(store_dir, content_hash, resolved_bytes)?;
                coco_types::PersistedComposerElement::Image {
                    start: *start,
                    end: *end,
                    media_type: media_type.clone(),
                    data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
                }
            }
            StoredComposerElement::FileRef { start, end } => {
                coco_types::PersistedComposerElement::FileRef {
                    start: *start,
                    end: *end,
                }
            }
        };
        elements.push(resolved);
    }
    let composer = coco_types::PersistedComposer {
        text: stored.text.clone(),
        next_attachment_label: stored.next_attachment_label,
        elements,
    };
    validate_composer_shape(&composer).ok()?;
    Some(composer)
}

fn validate_composer_shape(composer: &coco_types::PersistedComposer) -> crate::Result<()> {
    if composer.text.len() > MAX_COMPOSER_TEXT_BYTES
        || composer.elements.len() > MAX_COMPOSER_ELEMENTS
    {
        return Err(crate::SessionError::generic(
            "persisted composer exceeds history shape limits",
        ));
    }
    if composer.next_attachment_label < 0 {
        return Err(crate::SessionError::generic(
            "invalid persisted composer attachment label",
        ));
    }
    let mut previous_end = 0usize;
    for element in &composer.elements {
        let (start, end) = match element {
            coco_types::PersistedComposerElement::Paste { start, end, .. }
            | coco_types::PersistedComposerElement::Image { start, end, .. }
            | coco_types::PersistedComposerElement::FileRef { start, end } => (*start, *end),
        };
        let start = usize::try_from(start).map_err(|_| {
            crate::SessionError::generic("invalid persisted composer element range")
        })?;
        let end = usize::try_from(end).map_err(|_| {
            crate::SessionError::generic("invalid persisted composer element range")
        })?;
        let source = composer.text.get(start..end).ok_or_else(|| {
            crate::SessionError::generic("invalid persisted composer element range")
        })?;
        if start < previous_end || start >= end || source.contains(['\n', '\r']) {
            return Err(crate::SessionError::generic(
                "invalid persisted composer element range",
            ));
        }
        previous_end = end;
    }
    Ok(())
}

fn read_verified_blob(
    store_dir: &Path,
    expected_hash: &str,
    resolved_bytes: &AtomicUsize,
) -> Option<Vec<u8>> {
    if !is_content_hash(expected_hash) {
        return None;
    }
    let path = store_dir.join(expected_hash);
    let _ = restrict_file_permissions(&path);
    let len = usize::try_from(std::fs::metadata(&path).ok()?.len()).ok()?;
    if len > MAX_ATTACHMENT_BLOB_BYTES {
        return None;
    }
    resolved_bytes
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current
                .checked_add(len)
                .filter(|next| *next <= MAX_RESOLVED_HISTORY_BYTES)
        })
        .ok()?;
    let bytes = std::fs::read(path).ok()?;
    (hash_bytes(&bytes) == expected_hash).then_some(bytes)
}

fn stored_composer_hashes(composer: &StoredComposer) -> HashSet<String> {
    composer
        .elements
        .iter()
        .filter_map(|element| match element {
            StoredComposerElement::Paste {
                content:
                    StoredText {
                        content_hash: Some(hash),
                        ..
                    },
                ..
            }
            | StoredComposerElement::Image {
                content_hash: hash, ..
            } => is_content_hash(hash).then(|| hash.clone()),
            StoredComposerElement::Paste { .. } | StoredComposerElement::FileRef { .. } => None,
        })
        .collect()
}

fn is_content_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn hash_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hash = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(hash, "{byte:02x}");
    }
    hash
}

fn current_timestamp_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(unix)]
fn restrict_dir_permissions(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn restrict_dir_permissions(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn restrict_file_permissions(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn restrict_file_permissions(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
#[path = "history.test.rs"]
mod tests;
