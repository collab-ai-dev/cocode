//! Diagnostics storage with debouncing for system_reminder integration

use lsp_types::DiagnosticSeverity;
use lsp_types::PublishDiagnosticsParams;
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::hash::Hash;
use std::hash::Hasher;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use tokio::sync::RwLock;
use tracing::info;

/// Debounce interval in milliseconds
const DIAGNOSTIC_DEBOUNCE_MS: u64 = 150;

/// Stale entry expiration time in seconds (1 hour)
const STALE_ENTRY_EXPIRATION_SECS: u64 = 3600;

/// Maximum diagnostics per file in take_dirty output
const MAX_DIAGNOSTICS_PER_FILE: usize = 10;

/// Maximum total diagnostics in take_dirty output
const MAX_DIAGNOSTICS_TOTAL: usize = 30;

/// Maximum delivered diagnostic hashes to track (LRU)
const DELIVERED_LRU_CAPACITY: usize = 500;

/// Simplified diagnostic entry for AI consumption
#[derive(Debug, Clone)]
pub struct DiagnosticEntry {
    pub file: PathBuf,
    pub line: i32,
    pub character: i32,
    pub severity: DiagnosticSeverityLevel,
    pub message: String,
    pub code: Option<String>,
    pub source: Option<String>,
}

/// Severity level (simplified for AI)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticSeverityLevel {
    Error,
    Warning,
    Info,
    Hint,
}

impl From<Option<DiagnosticSeverity>> for DiagnosticSeverityLevel {
    fn from(severity: Option<DiagnosticSeverity>) -> Self {
        match severity {
            Some(DiagnosticSeverity::ERROR) => DiagnosticSeverityLevel::Error,
            Some(DiagnosticSeverity::WARNING) => DiagnosticSeverityLevel::Warning,
            Some(DiagnosticSeverity::INFORMATION) => DiagnosticSeverityLevel::Info,
            Some(DiagnosticSeverity::HINT) => DiagnosticSeverityLevel::Hint,
            None => DiagnosticSeverityLevel::Error,
            _ => DiagnosticSeverityLevel::Info,
        }
    }
}

impl DiagnosticSeverityLevel {
    /// Get display string for severity
    pub fn as_str(&self) -> &'static str {
        match self {
            DiagnosticSeverityLevel::Error => "error",
            DiagnosticSeverityLevel::Warning => "warning",
            DiagnosticSeverityLevel::Info => "info",
            DiagnosticSeverityLevel::Hint => "hint",
        }
    }

    /// Get numeric priority for severity comparison.
    ///
    /// Higher value = more severe. Used for filtering:
    /// `severity.priority() >= min_severity.priority()`
    pub fn priority(&self) -> i32 {
        match self {
            DiagnosticSeverityLevel::Error => 4,
            DiagnosticSeverityLevel::Warning => 3,
            DiagnosticSeverityLevel::Info => 2,
            DiagnosticSeverityLevel::Hint => 1,
        }
    }
}

struct FileDiagnostics {
    diagnostics: Vec<DiagnosticEntry>,
    last_update: Instant,
    last_accessed: Instant,
}

/// Compute a dedup key for a diagnostic entry.
///
/// Two diagnostics with the same file, line, column, severity, message,
/// and code are considered duplicates.
fn diagnostic_dedup_key(entry: &DiagnosticEntry) -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    entry.file.hash(&mut hasher);
    entry.line.hash(&mut hasher);
    entry.character.hash(&mut hasher);
    entry.severity.as_str().hash(&mut hasher);
    entry.message.hash(&mut hasher);
    entry.code.as_deref().hash(&mut hasher);
    hasher.finish()
}

/// Diagnostics storage with debouncing
pub struct DiagnosticsStore {
    files: Arc<RwLock<HashMap<PathBuf, FileDiagnostics>>>,
    dirty: Arc<RwLock<Vec<PathBuf>>>,
    /// LRU set of delivered diagnostic hashes to prevent re-showing
    delivered: Arc<RwLock<DeliveredLru>>,
}

/// Simple LRU hash set for delivered diagnostic deduplication
struct DeliveredLru {
    entries: VecDeque<u64>,
    set: HashSet<u64>,
}

impl DeliveredLru {
    fn new() -> Self {
        Self {
            entries: VecDeque::new(),
            set: HashSet::new(),
        }
    }

    /// Returns true if already delivered (duplicate)
    fn contains(&self, hash: u64) -> bool {
        self.set.contains(&hash)
    }

    /// Mark a hash as delivered, evicting oldest if at capacity
    fn insert(&mut self, hash: u64) {
        if self.set.contains(&hash) {
            return;
        }
        if self.entries.len() >= DELIVERED_LRU_CAPACITY
            && let Some(old) = self.entries.pop_front()
        {
            self.set.remove(&old);
        }
        self.entries.push_back(hash);
        self.set.insert(hash);
    }

    /// Clear all delivered hashes for a specific file
    fn clear_for_file(&mut self, entries_to_remove: &[u64]) {
        for hash in entries_to_remove {
            self.set.remove(hash);
        }
        self.entries.retain(|h| self.set.contains(h));
    }
}

impl std::fmt::Debug for DiagnosticsStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiagnosticsStore").finish_non_exhaustive()
    }
}

impl DiagnosticsStore {
    pub fn new() -> Self {
        Self {
            files: Arc::new(RwLock::new(HashMap::new())),
            dirty: Arc::new(RwLock::new(Vec::new())),
            delivered: Arc::new(RwLock::new(DeliveredLru::new())),
        }
    }

    /// Update diagnostics from publishDiagnostics notification
    pub async fn update(&self, params: PublishDiagnosticsParams) {
        let path = PathBuf::from(params.uri.path());
        let entry_count = params.diagnostics.len();
        let entries: Vec<DiagnosticEntry> = params
            .diagnostics
            .into_iter()
            .map(|d| DiagnosticEntry {
                file: path.clone(),
                line: d.range.start.line as i32 + 1,
                character: d.range.start.character as i32 + 1,
                severity: d.severity.into(),
                message: d.message,
                code: d.code.map(|c| match c {
                    lsp_types::NumberOrString::Number(n) => n.to_string(),
                    lsp_types::NumberOrString::String(s) => s,
                }),
                source: d.source,
            })
            .collect();

        let now = Instant::now();

        // Count by severity for logging (before moving entries)
        let error_count = entries
            .iter()
            .filter(|e| matches!(e.severity, DiagnosticSeverityLevel::Error))
            .count();
        let warning_count = entries
            .iter()
            .filter(|e| matches!(e.severity, DiagnosticSeverityLevel::Warning))
            .count();

        let mut files = self.files.write().await;

        files.insert(
            path.clone(),
            FileDiagnostics {
                diagnostics: entries,
                last_update: now,
                last_accessed: now,
            },
        );

        let mut dirty = self.dirty.write().await;
        if !dirty.contains(&path) {
            dirty.push(path.clone());
        }

        info!(
            "Received {} diagnostics for {} ({} errors, {} warnings)",
            entry_count,
            path.display(),
            error_count,
            warning_count
        );
    }

    /// Get diagnostics for a specific file
    pub async fn get_file(&self, path: &PathBuf) -> Vec<DiagnosticEntry> {
        // Use write lock to update last_accessed
        let mut files = self.files.write().await;
        if let Some(file_diags) = files.get_mut(path) {
            file_diags.last_accessed = Instant::now();
            file_diags.diagnostics.clone()
        } else {
            Vec::new()
        }
    }

    /// Get all diagnostics
    pub async fn get_all(&self) -> Vec<DiagnosticEntry> {
        let files = self.files.read().await;
        files.values().flat_map(|f| f.diagnostics.clone()).collect()
    }

    /// Take all dirty diagnostics (for system_reminder integration)
    ///
    /// Only returns diagnostics that have been stable for DIAGNOSTIC_DEBOUNCE_MS.
    /// Applies deduplication (skips already-delivered diagnostics) and volume
    /// limiting (max 10 per file, 30 total). Also triggers periodic cleanup
    /// of stale entries.
    pub async fn take_dirty(&self) -> Vec<DiagnosticEntry> {
        // Periodically clean up stale entries (runs on every take_dirty call)
        // This is a lightweight operation when there's nothing to clean up
        self.cleanup_stale().await;

        // Take dirty paths first, minimizing write lock duration
        let dirty_paths: Vec<PathBuf> = {
            let mut dirty = self.dirty.write().await;
            std::mem::take(&mut *dirty)
        };

        if dirty_paths.is_empty() {
            return Vec::new();
        }

        // Read files and check debounce status
        let mut candidates = Vec::new();
        let mut still_dirty = Vec::new();

        {
            let files = self.files.read().await;
            for path in dirty_paths {
                if let Some(file_diags) = files.get(&path) {
                    if file_diags.last_update.elapsed()
                        >= Duration::from_millis(DIAGNOSTIC_DEBOUNCE_MS)
                    {
                        candidates.extend(file_diags.diagnostics.iter().cloned());
                    } else {
                        // Still within debounce window, keep in dirty list
                        still_dirty.push(path);
                    }
                }
            }
        }

        // Re-add items that are still within debounce window
        if !still_dirty.is_empty() {
            let mut dirty = self.dirty.write().await;
            dirty.extend(still_dirty);
        }

        if candidates.is_empty() {
            return Vec::new();
        }

        // Sort each file's diagnostics by severity BEFORE truncating to
        // `MAX_DIAGNOSTICS_PER_FILE`, so errors are preserved when warnings
        // / hints would otherwise crowd them out of the cap. Stable sort
        // keeps within-severity order deterministic. Highest priority
        // (Error=4) first.
        candidates.sort_by(|a, b| b.severity.priority().cmp(&a.severity.priority()));

        // Dedup: skip diagnostics already delivered in previous turns
        let mut delivered = self.delivered.write().await;
        let mut per_file_counts: HashMap<&PathBuf, usize> = HashMap::new();
        let mut total = 0usize;
        let mut result = Vec::new();

        for entry in &candidates {
            // Volume limit: per-file cap
            let file_count = per_file_counts.entry(&entry.file).or_insert(0);
            if *file_count >= MAX_DIAGNOSTICS_PER_FILE {
                continue;
            }
            // Volume limit: total cap
            if total >= MAX_DIAGNOSTICS_TOTAL {
                break;
            }
            // Dedup: skip if already delivered
            let hash = diagnostic_dedup_key(entry);
            if delivered.contains(hash) {
                continue;
            }

            delivered.insert(hash);
            *file_count += 1;
            total += 1;
            result.push(entry.clone());
        }

        result
    }

    /// Check if there are pending dirty diagnostics
    pub async fn has_dirty(&self) -> bool {
        let dirty = self.dirty.read().await;
        !dirty.is_empty()
    }

    /// Clean up stale entries that haven't been accessed recently
    ///
    /// Removes entries that haven't been accessed for STALE_ENTRY_EXPIRATION_SECS.
    /// Returns the number of entries removed.
    pub async fn cleanup_stale(&self) -> usize {
        let expiration = Duration::from_secs(STALE_ENTRY_EXPIRATION_SECS);
        let mut files = self.files.write().await;
        let before_count = files.len();

        files.retain(|_path, file_diags| file_diags.last_accessed.elapsed() < expiration);

        let removed = before_count - files.len();
        if removed > 0 {
            info!(
                "Cleaned up {} stale diagnostic entries (older than {}s)",
                removed, STALE_ENTRY_EXPIRATION_SECS
            );
        }
        removed
    }

    /// Clear delivered diagnostic hashes for a file.
    ///
    /// Call this when a file is modified so that re-published diagnostics
    /// for the same file are not suppressed by deduplication.
    pub async fn clear_delivered_for_file(&self, path: &PathBuf) {
        let files = self.files.read().await;
        if let Some(file_diags) = files.get(path) {
            let hashes: Vec<u64> = file_diags
                .diagnostics
                .iter()
                .map(diagnostic_dedup_key)
                .collect();
            drop(files);
            let mut delivered = self.delivered.write().await;
            delivered.clear_for_file(&hashes);
        }
    }

    /// Clear all diagnostics
    pub async fn clear(&self) {
        let mut files = self.files.write().await;
        let mut dirty = self.dirty.write().await;
        files.clear();
        dirty.clear();
        let mut delivered = self.delivered.write().await;
        *delivered = DeliveredLru::new();
    }

    /// Format diagnostics for system_reminder
    pub fn format_for_system_reminder(entries: &[DiagnosticEntry]) -> String {
        if entries.is_empty() {
            return String::new();
        }

        let mut output = String::from("<new-diagnostics>\n");
        output.push_str("The following new diagnostic issues were detected:\n\n");

        let mut by_file: HashMap<&PathBuf, Vec<&DiagnosticEntry>> = HashMap::new();
        for entry in entries {
            by_file.entry(&entry.file).or_default().push(entry);
        }

        for (file, file_entries) in by_file {
            output.push_str(&format!("File: {}\n", file.display()));
            for entry in file_entries {
                let code_str = entry
                    .code
                    .as_ref()
                    .map(|c| format!(" [{c}]"))
                    .unwrap_or_default();
                let source_str = entry
                    .source
                    .as_ref()
                    .map(|s| format!(" ({s})"))
                    .unwrap_or_default();
                output.push_str(&format!(
                    "  Line {}: [{}] {}{}{}\n",
                    entry.line,
                    entry.severity.as_str(),
                    entry.message,
                    code_str,
                    source_str
                ));
            }
            output.push('\n');
        }

        output.push_str("</new-diagnostics>");
        output
    }
}

impl Default for DiagnosticsStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "diagnostics.test.rs"]
mod tests;
