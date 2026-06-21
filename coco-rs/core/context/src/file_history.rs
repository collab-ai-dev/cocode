//! File edit tracking with per-turn snapshots and content-addressed backups.
//!
//! Three-phase async pattern:
//! 1. Capture state (read current)
//! 2. Async I/O (backup files outside state lock)
//! 3. Commit (update state with fresh read)

use crate::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;
use sha2::Digest;
use sha2::Sha256;
use std::collections::HashMap;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs;

/// Maximum snapshots retained per session (oldest evicted).
const MAX_SNAPSHOTS: usize = 100;

/// IDE-bridge sink for file-update notifications.
///
/// Called by `make_snapshot` after a fresh snapshot is committed,
/// once per file whose backup name changed between the previous and
/// new snapshot. The bridge implementation forwards these to the
/// IDE-side MCP so the editor can refresh open buffers.
#[async_trait]
pub trait FileUpdateSink: Send + Sync {
    async fn notify(
        &self,
        file_path: &Path,
        old_content: Option<String>,
        new_content: Option<String>,
    );
}

/// Persistence sink for file-history snapshots.
///
/// `record(message_id, snapshot, is_snapshot_update)` is called
/// every time `FileHistoryState::track_edit` or `make_snapshot`
/// mutates the in-memory snapshot vec. The implementer (typically
/// `coco-session::TranscriptStore`) appends a `file-history-snapshot`
/// entry to the JSONL transcript so that on resume, the state can
/// be rebuilt by replaying the chain.
///
/// We pass `serde_json::Value` instead of the typed snapshot to keep the
/// dependency edge from coco-context up to coco-session, not the
/// other way (coco-session would otherwise need to depend on
/// coco-context just for the typed shape).
#[async_trait]
pub trait FileHistorySnapshotSink: Send + Sync {
    async fn record(
        &self,
        message_id: &str,
        snapshot_json: serde_json::Value,
        is_snapshot_update: bool,
    );
}

/// Tracks file edits across the conversation for undo/snapshot capability.
#[derive(Default, Serialize, Deserialize)]
pub struct FileHistoryState {
    /// Ordered snapshots (newest last, max `MAX_SNAPSHOTS`).
    pub snapshots: Vec<FileHistorySnapshot>,
    /// Files currently being tracked.
    pub tracked_files: HashSet<PathBuf>,
    /// Monotonically increasing counter (activity signal).
    pub snapshot_sequence: i64,
    /// JSONL transcript writer. Skipped on serialization (sink isn't
    /// serializable; resume uses `restore_from_snapshots` to rebuild).
    #[serde(skip, default)]
    pub sink: Option<Arc<dyn FileHistorySnapshotSink>>,
    /// Optional IDE-bridge sink — called after each snapshot for
    /// every file whose content changed since the prior snapshot.
    #[serde(skip, default)]
    pub file_update_sink: Option<Arc<dyn FileUpdateSink>>,
}

impl Clone for FileHistoryState {
    fn clone(&self) -> Self {
        Self {
            snapshots: self.snapshots.clone(),
            tracked_files: self.tracked_files.clone(),
            snapshot_sequence: self.snapshot_sequence,
            sink: self.sink.clone(),
            file_update_sink: self.file_update_sink.clone(),
        }
    }
}

impl std::fmt::Debug for FileHistoryState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileHistoryState")
            .field("snapshots", &self.snapshots)
            .field("tracked_files", &self.tracked_files)
            .field("snapshot_sequence", &self.snapshot_sequence)
            .field("sink", &self.sink.as_ref().map(|_| "<sink>"))
            .field(
                "file_update_sink",
                &self.file_update_sink.as_ref().map(|_| "<sink>"),
            )
            .finish()
    }
}

/// A snapshot of file states at a particular point in the conversation.
///
/// Wire shape is snake_case JSON. Timestamps are `chrono::DateTime<Utc>`
/// serialized to RFC 3339 strings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileHistorySnapshot {
    /// Message UUID that triggered this snapshot.
    pub message_id: String,
    /// Per-file backup info.
    pub tracked_file_backups: HashMap<PathBuf, FileHistoryBackup>,
    /// When the snapshot was taken (RFC 3339 string).
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Backup info for a single file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileHistoryBackup {
    /// Content-addressed backup name. `None` = file didn't exist at this version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_file_name: Option<String>,
    /// Version counter for this file within the session.
    pub version: i32,
    /// When the backup was created (RFC 3339 string).
    pub backup_time: chrono::DateTime<chrono::Utc>,
}

/// Result of a diff stats preview (what `rewind` would change).
#[derive(Debug, Clone, Default)]
pub struct DiffStats {
    pub files_changed: Vec<PathBuf>,
    pub insertions: i64,
    pub deletions: i64,
}

/// Resolves the backup directory for a session.
pub fn backup_dir(config_home: &Path, session_id: &str) -> PathBuf {
    config_home.join("file-history").join(session_id)
}

/// Content-addressed backup file name: first 16 hex chars of SHA-256(path) + version.
fn backup_file_name(file_path: &Path, version: i32) -> String {
    let hash = Sha256::digest(file_path.as_os_str().as_encoded_bytes());
    let hex = hex_encode_16(&hash);
    format!("{hex}@v{version}")
}

/// First 16 hex chars of a hash digest.
fn hex_encode_16(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take(8)
        .fold(String::with_capacity(16), |mut s, b| {
            use std::fmt::Write;
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// Resolve full path to a backup file on disk.
fn resolve_backup_path(config_home: &Path, session_id: &str, backup_name: &str) -> PathBuf {
    backup_dir(config_home, session_id).join(backup_name)
}

/// Check if the origin file has changed compared to a backup.
///
/// 1. Both missing → unchanged.
/// 2. One missing → changed.
/// 3. Different size → changed.
/// 4. Different mode (Unix) → changed (e.g. chmod-only edits).
/// 5. Origin mtime older than backup → unchanged (no need to read content).
/// 6. Else fall through to full byte-comparison.
async fn origin_file_changed(origin: &Path, backup_path: &Path) -> Result<bool> {
    let origin_meta = match fs::metadata(origin).await {
        Ok(m) => Some(m),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    let backup_meta = match fs::metadata(backup_path).await {
        Ok(m) => Some(m),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    let (origin_meta, backup_meta) = match (origin_meta, backup_meta) {
        (None, None) => return Ok(false),
        (Some(_), None) | (None, Some(_)) => return Ok(true),
        (Some(o), Some(b)) => (o, b),
    };

    // Different size means changed.
    if origin_meta.len() != backup_meta.len() {
        return Ok(true);
    }

    // chmod-only diffs: compare `mode` to detect permission flips.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if origin_meta.permissions().mode() != backup_meta.permissions().mode() {
            return Ok(true);
        }
    }

    // mtime short-circuit: if the origin was last modified before the
    // backup, the backup represents a newer state — nothing to do.
    // Avoids the byte read entirely on the common "not edited"
    // path.
    if let (Ok(origin_mtime), Ok(backup_mtime)) = (origin_meta.modified(), backup_meta.modified())
        && origin_mtime < backup_mtime
    {
        return Ok(false);
    }

    // Fall back to byte comparison.
    let origin_bytes = fs::read(origin).await?;
    let backup_bytes = fs::read(backup_path).await?;
    Ok(origin_bytes != backup_bytes)
}

/// Read the contents of a content-addressed backup, returning `None`
/// if the backup name is missing or the file cannot be read. Shared
/// by `get_diff_stats` and `get_diff_stats_between`.
async fn read_backup_content(
    config_home: &Path,
    session_id: &str,
    backup_file_name: &Option<String>,
) -> Option<String> {
    let name = backup_file_name.as_ref()?;
    let bp = resolve_backup_path(config_home, session_id, name);
    fs::read_to_string(&bp).await.ok()
}

/// Accumulate line-level diff stats for one file path into `stats`.
/// Skips identical content. Line counting: count `\n` boundaries in each
/// chunk, treating a trailing no-newline chunk as +1 line.
fn accumulate_diff(
    stats: &mut DiffStats,
    file_path: &Path,
    old_content: &Option<String>,
    new_content: &Option<String>,
) {
    if old_content == new_content {
        return;
    }
    stats.files_changed.push(file_path.to_path_buf());
    let old = old_content.as_deref().unwrap_or("");
    let new = new_content.as_deref().unwrap_or("");
    let diff = similar::TextDiff::from_lines(old, new);
    for change in diff.iter_all_changes() {
        let v = change.value();
        let nl = v.bytes().filter(|&b| b == b'\n').count() as i64;
        let line_count = if v.ends_with('\n') || v.is_empty() {
            nl
        } else {
            nl + 1
        };
        match change.tag() {
            similar::ChangeTag::Insert => stats.insertions += line_count,
            similar::ChangeTag::Delete => stats.deletions += line_count,
            similar::ChangeTag::Equal => {}
        }
    }
}

fn current_time_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl FileHistoryState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start tracking a file for edit history.
    pub fn track_file(&mut self, path: PathBuf) {
        self.tracked_files.insert(path);
    }

    /// Whether a snapshot exists for a given message.
    pub fn can_restore(&self, message_id: &str) -> bool {
        self.snapshots.iter().any(|s| s.message_id == message_id)
    }

    /// Get the most recent backup for a file across all snapshots.
    pub fn latest_backup(&self, path: &Path) -> Option<&FileHistoryBackup> {
        self.snapshots
            .iter()
            .rev()
            .find_map(|s| s.tracked_file_backups.get(path))
    }

    /// Evict oldest snapshots beyond the cap.
    fn enforce_cap(&mut self) {
        while self.snapshots.len() > MAX_SNAPSHOTS {
            self.snapshots.remove(0);
        }
    }

    /// Track a file edit BEFORE writing. Creates a backup of pre-edit content.
    ///
    /// Three-phase: check state → async I/O → commit. **Always
    /// updates the most-recent snapshot in place.** Never pushes a new
    /// snapshot — that is `make_snapshot`'s job, which runs once per
    /// user message and bumps per-file `version`. Within a single
    /// snapshot the per-file `version` is fixed at `1` because the
    /// snapshot represents "pre-edit state for this turn"; subsequent
    /// re-edits of the same file inside the same turn observe the
    /// file is already tracked and skip (Phase 1).
    ///
    /// - Phase 1 skip is keyed on the most-recent snapshot's tracked files —
    ///   **independent of `messageId`**;
    /// - Phase 2 calls `createBackup(filePath, 1)` — version literal;
    /// - Phase 3 writes back the same `mostRecentSnapshot` with the
    ///   new backup spliced in (`isSnapshotUpdate = true`). The OUTER
    ///   metadata-entry `messageId` is the `messageId` parameter (the message
    ///   currently being authored); the INNER `snapshot.messageId`
    ///   stays as the snapshot's own messageId (unchanged).
    pub async fn track_edit(
        &mut self,
        file_path: &Path,
        message_id: &str,
        config_home: &Path,
        session_id: &str,
    ) -> Result<()> {
        const TRACK_EDIT_VERSION: i32 = 1;

        // Phase 1: skip if the file is already tracked in the most
        // recent snapshot, regardless of which messageId that snapshot
        // belongs to. Speculative writes would otherwise clobber the
        // {hash}@v1 backup on every repeat call.
        //
        // No snapshot yet → bootstrap an empty one for `message_id`
        // and proceed (the original `logError`s and returns here, but
        // this approach lets the typical turn
        // lifecycle (`make_snapshot` → tool → `track_edit`) and the
        // edge case (tool runs before the per-turn `make_snapshot`)
        // both work without callers having to know which path they're
        // on.
        let already_tracked = match self.snapshots.last() {
            Some(snap) => snap.tracked_file_backups.contains_key(file_path),
            None => {
                self.snapshots.push(FileHistorySnapshot {
                    message_id: message_id.to_string(),
                    tracked_file_backups: HashMap::new(),
                    timestamp: chrono::Utc::now(),
                });
                false
            }
        };
        if already_tracked {
            return Ok(());
        }

        self.tracked_files.insert(file_path.to_path_buf());

        // Phase 2: Create backup of pre-edit content. Version is
        // hard-coded to 1 — see method doc.
        let backup_name = backup_file_name(file_path, TRACK_EDIT_VERSION);
        let dest = resolve_backup_path(config_home, session_id, &backup_name);

        let backup_file_name = match fs::read(file_path).await {
            Ok(content) => {
                ensure_parent_dir(&dest).await?;
                let size = content.len() as u64;
                fs::write(&dest, &content).await.map_err(|e| {
                    crate::ContextError::generic(format!(
                        "writing backup to {}: {e}",
                        dest.display()
                    ))
                })?;
                // Preserve file permissions (chmod backup to match source mode).
                #[cfg(unix)]
                if let Ok(meta) = fs::metadata(file_path).await {
                    let _ = fs::set_permissions(&dest, meta.permissions()).await;
                }
                coco_otel::events::emit_file_backup_created(
                    &file_path.display().to_string(),
                    TRACK_EDIT_VERSION,
                    size,
                );
                Some(backup_name)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // File doesn't exist yet — will be created by the tool.
                None
            }
            Err(e) => return Err(e.into()),
        };

        // Phase 3: Re-check race and commit by splicing into the most
        // recent snapshot. Never push a new snapshot — `make_snapshot`
        // is the only producer of new snapshots.
        let is_new_file = backup_file_name.is_none();
        let backup = FileHistoryBackup {
            backup_file_name,
            version: TRACK_EDIT_VERSION,
            backup_time: chrono::Utc::now(),
        };

        let Some(snapshot) = self.snapshots.last_mut() else {
            return Ok(());
        };
        if snapshot.tracked_file_backups.contains_key(file_path) {
            // Lost the race; the other writer won — leave its backup intact.
            return Ok(());
        }
        snapshot
            .tracked_file_backups
            .insert(file_path.to_path_buf(), backup);

        coco_otel::events::emit_file_track_edit_success(
            &file_path.display().to_string(),
            TRACK_EDIT_VERSION,
            is_new_file,
        );

        if let Some(sink) = self.sink.clone()
            && let Some(snap) = self.snapshots.last().cloned()
            && let Ok(snap_json) = serde_json::to_value(&snap)
        {
            // Outer entry.messageId = the messageId arg (current
            // turn). Inner snapshot.messageId stays as snap's own
            // messageId (unchanged by serializing the existing
            // snapshot). isSnapshotUpdate is always true — trackEdit
            // never produces a fresh snapshot.
            sink.record(message_id, snap_json, true).await;
        }
        Ok(())
    }

    /// Create a full snapshot of all tracked files.
    ///
    /// Called after each user message. Backs up any files that changed since
    /// the last snapshot.
    pub async fn make_snapshot(
        &mut self,
        message_id: &str,
        config_home: &Path,
        session_id: &str,
    ) -> Result<()> {
        // Capture the prior snapshot before mutating so we can
        // compute file-update diffs for the IDE-bridge sink.
        let prior_snapshot = self.snapshots.last().cloned();
        // Inherit backups from previous snapshot.
        let mut backups = self
            .snapshots
            .last()
            .map(|s| s.tracked_file_backups.clone())
            .unwrap_or_default();

        // Back up each tracked file if it changed.
        for file_path in &self.tracked_files.clone() {
            let prev_backup = self.latest_backup(file_path);
            let version = prev_backup.map_or(1, |b| b.version + 1);

            let needs_backup = if let Some(prev) = prev_backup {
                if let Some(ref bname) = prev.backup_file_name {
                    let bp = resolve_backup_path(config_home, session_id, bname);
                    origin_file_changed(file_path, &bp).await.unwrap_or(true)
                } else {
                    // Previous was null (didn't exist) — check if file now exists.
                    fs::metadata(file_path).await.is_ok()
                }
            } else {
                fs::metadata(file_path).await.is_ok()
            };

            if needs_backup {
                let bname = backup_file_name(file_path, version);
                let dest = resolve_backup_path(config_home, session_id, &bname);

                let backup_file = match fs::read(file_path).await {
                    Ok(content) => {
                        ensure_parent_dir(&dest).await?;
                        let size = content.len() as u64;
                        fs::write(&dest, &content).await?;
                        // Preserve file permissions (chmod backup to match source mode).
                        #[cfg(unix)]
                        if let Ok(meta) = fs::metadata(file_path).await {
                            let _ = fs::set_permissions(&dest, meta.permissions()).await;
                        }
                        coco_otel::events::emit_file_backup_created(
                            &file_path.display().to_string(),
                            version,
                            size,
                        );
                        Some(bname)
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                    Err(e) => return Err(e.into()),
                };

                backups.insert(
                    file_path.clone(),
                    FileHistoryBackup {
                        backup_file_name: backup_file,
                        version,
                        backup_time: chrono::Utc::now(),
                    },
                );
            }
        }

        self.snapshot_sequence += 1;
        let new_snapshot = FileHistorySnapshot {
            message_id: message_id.to_string(),
            tracked_file_backups: backups,
            timestamp: chrono::Utc::now(),
        };
        self.snapshots.push(new_snapshot.clone());
        self.enforce_cap();
        coco_otel::events::emit_file_snapshot_success(
            self.tracked_files.len(),
            self.snapshots.len(),
        );
        // `false` appends a fresh entry; resume rebuilds the chain
        // by reading these in order.
        if let Some(sink) = self.sink.clone()
            && let Ok(snap_json) = serde_json::to_value(&new_snapshot)
        {
            sink.record(message_id, snap_json, false).await;
        }

        // IDE-bridge: notify per-file content updates.
        if let Some(file_sink) = self.file_update_sink.clone() {
            for (path, new_backup) in &new_snapshot.tracked_file_backups {
                let old_backup = prior_snapshot
                    .as_ref()
                    .and_then(|s| s.tracked_file_backups.get(path));
                // Skip when name + version unchanged (same backup blob,
                // no edit between snapshots).
                if old_backup.map(|b| (&b.backup_file_name, b.version))
                    == Some((&new_backup.backup_file_name, new_backup.version))
                {
                    continue;
                }
                let old_content = match old_backup.and_then(|b| b.backup_file_name.as_ref()) {
                    Some(name) => {
                        let bp = resolve_backup_path(config_home, session_id, name);
                        fs::read_to_string(&bp).await.ok()
                    }
                    None => None,
                };
                let new_content = match new_backup.backup_file_name.as_ref() {
                    Some(name) => {
                        let bp = resolve_backup_path(config_home, session_id, name);
                        fs::read_to_string(&bp).await.ok()
                    }
                    None => None,
                };
                if old_content != new_content {
                    file_sink.notify(path, old_content, new_content).await;
                }
            }
        }
        Ok(())
    }

    /// Restore files to the state captured in a snapshot.
    pub async fn rewind(
        &self,
        message_id: &str,
        config_home: &Path,
        session_id: &str,
    ) -> Result<Vec<PathBuf>> {
        let snapshot = self
            .snapshots
            .iter()
            .rfind(|s| s.message_id == message_id)
            .ok_or_else(|| crate::ContextError::generic("no snapshot found for message"))?;

        let changed = match apply_snapshot(self, snapshot, config_home, session_id).await {
            Ok(c) => c,
            Err(e) => {
                coco_otel::events::emit_file_rewind_failed(&e.to_string());
                return Err(e);
            }
        };
        coco_otel::events::emit_file_rewind_success(
            snapshot.tracked_file_backups.len(),
            changed.len(),
        );
        Ok(changed)
    }

    /// Look up the first-version backup for a tracked file.
    ///
    /// Returns:
    /// - `Some(Some(name))` — restore from this backup
    /// - `Some(None)` — file did not exist at v1 (delete it on rewind)
    /// - `None` — no v1 entry for this path; cannot resolve
    ///
    /// Used by `apply_snapshot` and `get_diff_stats` when the target
    /// snapshot has no entry for a file that became tracked later
    /// (e.g. file first edited in turn 5, rewinding to turn 7's
    /// snapshot which inherits backups but the entry only exists if
    /// the file was modified in turns 5–7).
    pub fn first_version_backup_name(&self, path: &Path) -> Option<Option<String>> {
        for snapshot in &self.snapshots {
            if let Some(backup) = snapshot.tracked_file_backups.get(path)
                && backup.version == 1
            {
                return Some(backup.backup_file_name.clone());
            }
        }
        None
    }

    /// Preview what `rewind` would change without actually modifying files.
    pub async fn get_diff_stats(
        &self,
        message_id: &str,
        config_home: &Path,
        session_id: &str,
    ) -> Result<DiffStats> {
        let snapshot = self
            .snapshots
            .iter()
            .rfind(|s| s.message_id == message_id)
            .ok_or_else(|| crate::ContextError::generic("no snapshot found for message"))?;

        let mut stats = DiffStats::default();
        // Walk `tracked_files`, falling back to v1 backup for files
        // not in the target snapshot's map. Mirrors `apply_snapshot`'s
        // coverage.
        for file_path in &self.tracked_files {
            let from_backup = match snapshot.tracked_file_backups.get(file_path) {
                Some(b) => b.backup_file_name.clone(),
                None => match self.first_version_backup_name(file_path) {
                    Some(name) => name,
                    None => continue,
                },
            };

            let current = fs::read_to_string(file_path).await.ok();
            let backed_up = read_backup_content(config_home, session_id, &from_backup).await;
            accumulate_diff(&mut stats, file_path, &backed_up, &current);
        }
        Ok(stats)
    }

    /// Compute the file-history diff between two checkpoints (or
    /// from a checkpoint to the live working tree when `to_message_id`
    /// is `None`). Used by the rewind picker to populate the per-row
    /// `+X -Y` summary in one async batch on picker mount.
    ///
    /// Semantics: for each tracked file, take the backup version
    /// captured at `from_message_id` (with v1 fallback) and the
    /// backup version captured at `to_message_id` (likewise), or the
    /// live file content when `to_message_id` is `None`. Run
    /// `similar::TextDiff::from_lines` on the resulting (old, new)
    /// pair and accumulate inserts/deletes per the same line-count
    /// rule used by [`Self::get_diff_stats`].
    ///
    /// `from_message_id` must resolve to a snapshot — otherwise the
    /// caller has nothing to compare against and we return an error
    /// matching [`Self::get_diff_stats`]'s contract.
    pub async fn get_diff_stats_between(
        &self,
        from_message_id: &str,
        to_message_id: Option<&str>,
        config_home: &Path,
        session_id: &str,
    ) -> Result<DiffStats> {
        let from_snapshot = self
            .snapshots
            .iter()
            .rfind(|s| s.message_id == from_message_id)
            .ok_or_else(|| crate::ContextError::generic("no snapshot found for from_message_id"))?;
        let to_snapshot = match to_message_id {
            Some(id) => Some(
                self.snapshots
                    .iter()
                    .rfind(|s| s.message_id == id)
                    .ok_or_else(|| {
                        crate::ContextError::generic("no snapshot found for to_message_id")
                    })?,
            ),
            None => None,
        };

        let mut stats = DiffStats::default();
        for file_path in &self.tracked_files {
            let from_backup = match from_snapshot.tracked_file_backups.get(file_path) {
                Some(b) => b.backup_file_name.clone(),
                None => match self.first_version_backup_name(file_path) {
                    Some(name) => name,
                    None => continue,
                },
            };
            let old_content = read_backup_content(config_home, session_id, &from_backup).await;
            let new_content = match to_snapshot {
                Some(snap) => {
                    let to_backup = match snap.tracked_file_backups.get(file_path) {
                        Some(b) => b.backup_file_name.clone(),
                        None => match self.first_version_backup_name(file_path) {
                            Some(name) => name,
                            None => continue,
                        },
                    };
                    read_backup_content(config_home, session_id, &to_backup).await
                }
                None => fs::read_to_string(file_path).await.ok(),
            };
            accumulate_diff(&mut stats, file_path, &old_content, &new_content);
        }
        Ok(stats)
    }

    /// Fast boolean check: has any tracked file changed since the given snapshot?
    pub async fn has_any_changes(
        &self,
        message_id: &str,
        config_home: &Path,
        session_id: &str,
    ) -> bool {
        let Some(snapshot) = self.snapshots.iter().rfind(|s| s.message_id == message_id) else {
            return false;
        };
        // Walk tracked_files with v1 fallback.
        for file_path in &self.tracked_files {
            let backup_file_name = match snapshot.tracked_file_backups.get(file_path) {
                Some(b) => b.backup_file_name.clone(),
                None => match self.first_version_backup_name(file_path) {
                    Some(name) => name,
                    None => continue,
                },
            };
            if let Some(ref bname) = backup_file_name {
                let bp = resolve_backup_path(config_home, session_id, bname);
                if origin_file_changed(file_path, &bp).await.unwrap_or(true) {
                    return true;
                }
            } else if fs::metadata(file_path).await.is_ok() {
                return true;
            }
        }
        false
    }

    /// Install a JSONL persistence sink. Calls to `track_edit` and
    /// `make_snapshot` after this will append to the sink.
    pub fn with_sink(mut self, sink: Arc<dyn FileHistorySnapshotSink>) -> Self {
        self.sink = Some(sink);
        self
    }

    /// Install or replace the snapshot sink in-place.
    pub fn set_sink(&mut self, sink: Arc<dyn FileHistorySnapshotSink>) {
        self.sink = Some(sink);
    }

    /// Rebuild state from persisted snapshots (session resume).
    ///
    /// `snapshot_sequence` is the **count of snapshots**, not the
    /// maximum per-file version. The UI's activity polling treats this
    /// as a monotonic tick; using max_version would let one heavily-edited
    /// file inflate the counter independently of how many snapshots exist.
    pub fn restore_from_snapshots(snapshots: Vec<FileHistorySnapshot>) -> Self {
        let mut tracked_files = HashSet::new();
        for snapshot in &snapshots {
            for path in snapshot.tracked_file_backups.keys() {
                tracked_files.insert(path.clone());
            }
        }
        Self {
            snapshot_sequence: snapshots.len() as i64,
            tracked_files,
            snapshots,
            sink: None,
            file_update_sink: None,
        }
    }

    /// Install an IDE-bridge file-update sink. After each
    /// `make_snapshot`, files whose backup name changed will be
    /// notified through this sink.
    pub fn set_file_update_sink(&mut self, sink: Arc<dyn FileUpdateSink>) {
        self.file_update_sink = Some(sink);
    }
}

/// Apply a snapshot: restore each tracked file from its backup.
/// Returns the list of files that were actually changed.
async fn apply_snapshot(
    state: &FileHistoryState,
    snapshot: &FileHistorySnapshot,
    config_home: &Path,
    session_id: &str,
) -> Result<Vec<PathBuf>> {
    let mut plan = Vec::new();
    // Iterate the full tracked-files set (not just snapshot's
    // backups). For files the snapshot has no entry for, fall back
    // to their first-version backup so files first edited mid-
    // conversation are restorable to any earlier rewind point.
    for file_path in &state.tracked_files {
        let backup_file_name = match snapshot.tracked_file_backups.get(file_path) {
            Some(backup) => backup.backup_file_name.clone(),
            None => match state.first_version_backup_name(file_path) {
                Some(name) => name,
                None => {
                    tracing::warn!(
                        target: "file_history",
                        path = %file_path.display(),
                        "no v1 backup; cannot restore — leaving file untouched",
                    );
                    continue;
                }
            },
        };

        match plan_snapshot_file(config_home, session_id, file_path, backup_file_name).await {
            Ok(Some(action)) => plan.push(action),
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(
                    target: "file_history",
                    path = %file_path.display(),
                    error = %e,
                    "failed to restore file from snapshot; continuing",
                );
            }
        }
    }
    apply_restore_plan(plan).await
}

struct RestorePlanAction {
    path: PathBuf,
    op: RestorePlanOp,
}

enum RestorePlanOp {
    Copy {
        content: Vec<u8>,
        #[cfg(unix)]
        permissions: Option<std::fs::Permissions>,
    },
    Delete,
}

async fn plan_snapshot_file(
    config_home: &Path,
    session_id: &str,
    file_path: &Path,
    backup_file_name: Option<String>,
) -> Result<Option<RestorePlanAction>> {
    match backup_file_name {
        Some(bname) => {
            let bp = checked_backup_path(config_home, session_id, &bname)?;
            if origin_file_changed(file_path, &bp).await.unwrap_or(true) {
                let content = fs::read(&bp).await.map_err(|e| {
                    crate::ContextError::generic(format!("reading backup {}: {e}", bp.display()))
                })?;
                #[cfg(unix)]
                let permissions = fs::metadata(&bp).await.ok().map(|meta| meta.permissions());
                return Ok(Some(RestorePlanAction {
                    path: file_path.to_path_buf(),
                    op: RestorePlanOp::Copy {
                        content,
                        #[cfg(unix)]
                        permissions,
                    },
                }));
            }
            Ok(None)
        }
        None => {
            // File didn't exist at snapshot time — delete if it exists now.
            if fs::metadata(file_path).await.is_ok() {
                return Ok(Some(RestorePlanAction {
                    path: file_path.to_path_buf(),
                    op: RestorePlanOp::Delete,
                }));
            }
            Ok(None)
        }
    }
}

async fn apply_restore_plan(plan: Vec<RestorePlanAction>) -> Result<Vec<PathBuf>> {
    let mut changed = Vec::new();
    for action in plan {
        match apply_restore_action(&action).await {
            Ok(()) => changed.push(action.path),
            Err(e) => {
                tracing::warn!(
                    target: "file_history",
                    path = %action.path.display(),
                    error = %e,
                    "failed to apply file restore action; continuing",
                );
            }
        }
    }
    Ok(changed)
}

async fn apply_restore_action(action: &RestorePlanAction) -> Result<()> {
    match &action.op {
        RestorePlanOp::Copy {
            content,
            #[cfg(unix)]
            permissions,
        } => {
            ensure_parent_dir(&action.path).await?;
            atomic_write(&action.path, content).await?;
            #[cfg(unix)]
            if let Some(permissions) = permissions {
                let _ = fs::set_permissions(&action.path, permissions.clone()).await;
            }
            Ok(())
        }
        RestorePlanOp::Delete => {
            fs::remove_file(&action.path).await?;
            Ok(())
        }
    }
}

fn checked_backup_path(config_home: &Path, session_id: &str, backup_name: &str) -> Result<PathBuf> {
    let backup_name_path = Path::new(backup_name);
    if backup_name_path.is_absolute()
        || backup_name_path
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err(crate::ContextError::generic(format!(
            "invalid backup name: {backup_name}"
        )));
    }
    let dir = backup_dir(config_home, session_id);
    let path = dir.join(backup_name_path);
    if !path.starts_with(&dir) {
        return Err(crate::ContextError::generic(format!(
            "backup path escapes backup dir: {}",
            path.display()
        )));
    }
    Ok(path)
}

async fn atomic_write(path: &Path, content: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| crate::ContextError::generic("target path has no parent directory"))?;
    let file_name = path.file_name().and_then(OsStr::to_str).unwrap_or("file");
    let tmp = parent.join(format!(".{file_name}.coco-tmp-{}", current_time_ms()));
    fs::write(&tmp, content).await?;
    match fs::rename(&tmp, path).await {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&tmp).await;
            Err(e.into())
        }
    }
}

/// Copy file history backups for session resume (hard-link with copy fallback).
///
/// **Replay step (`snapshots` + `sink`)**: does not stop at copying backup
/// files — after each successful copy it calls
/// `recordFileHistorySnapshot(messageId, snapshot, false)` so the resumed
/// session's transcript contains the snapshot chain. Without that replay
/// the new transcript has no `file-history-snapshot` entries and the rewind
/// picker can't reach any pre-resume checkpoint. Pass the prior session's
/// `snapshots` and a sink wired to the **new** session's JSONL to replicate
/// the behavior. Callers that only want the file-copy half (e.g. tests)
/// can pass `(&[], None)`.
pub async fn copy_file_history_for_resume(
    config_home: &Path,
    from_session: &str,
    to_session: &str,
    snapshots: &[FileHistorySnapshot],
    sink: Option<&dyn FileHistorySnapshotSink>,
) -> Result<i32> {
    let src_dir = backup_dir(config_home, from_session);
    let dst_dir = backup_dir(config_home, to_session);

    let mut copied = 0i32;
    if src_dir.exists() {
        fs::create_dir_all(&dst_dir).await?;
        let mut entries = fs::read_dir(&src_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let src = entry.path();
            let dst = dst_dir.join(entry.file_name());
            if dst.exists() {
                continue;
            }
            // Try hard-link first (fast, no copy).
            if fs::hard_link(&src, &dst).await.is_err() {
                // Fallback to copy.
                if let Err(e) = fs::copy(&src, &dst).await {
                    tracing::warn!("failed to copy backup {}: {e}", src.display());
                    continue;
                }
            }
            copied += 1;
        }
    }

    // Replay snapshot chain into the new session's transcript.
    // `isSnapshotUpdate: false` for every replayed snapshot re-creates
    // the chain rather than diff-overlaying.
    if let Some(sink) = sink {
        for snapshot in snapshots {
            let Ok(snap_json) = serde_json::to_value(snapshot) else {
                tracing::warn!(
                    message_id = %snapshot.message_id,
                    "failed to serialize snapshot during resume replay"
                );
                continue;
            };
            sink.record(&snapshot.message_id, snap_json, false).await;
        }
    }

    Ok(copied)
}

async fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "file_history.test.rs"]
mod tests;
