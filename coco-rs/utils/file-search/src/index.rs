//! File index with caching for autocomplete suggestions.
//!
//! This module provides a cached file index for fast file suggestions,
//! aligned with Claude Code's FileIndex system.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use nucleo::Config;
use nucleo::Matcher;
use nucleo::Utf32String;
use nucleo::pattern::AtomKind;
use nucleo::pattern::CaseMatching;
use nucleo::pattern::Normalization;
use nucleo::pattern::Pattern;
use tokio::process::Command;
use tokio::sync::RwLock;

/// Maximum number of suggestions to return.
pub const MAX_SUGGESTIONS: i32 = 15;

/// Cache time-to-live in seconds.
pub const CACHE_TTL_SECS: u64 = 60;

/// A single file suggestion with relevance score.
#[derive(Debug, Clone)]
pub struct FileSuggestion {
    /// Relative path to the file.
    pub path: String,
    /// Display text (may include icons or formatting).
    pub display_text: String,
    /// Relevance score from fuzzy matching (higher = better).
    pub score: u32,
    /// Character indices that matched the query (for highlighting).
    pub match_indices: Vec<i32>,
    /// Whether this is a directory (for @src/ style navigation).
    pub is_directory: bool,
}

/// Result of file discovery operation.
#[derive(Debug, Clone, Default)]
pub struct DiscoveryResult {
    /// List of tracked files (relative paths).
    pub files: Vec<String>,
    /// Extracted directory prefixes.
    pub directories: Vec<String>,
}

#[derive(Debug, Clone)]
struct IndexedPath {
    path: String,
    matcher_text: Utf32String,
    is_directory: bool,
}

/// Cached file index with background refresh support.
pub struct FileIndex {
    /// Cached files and directory prefixes (relative paths).
    entries: Vec<IndexedPath>,
    /// Last refresh timestamp.
    last_refresh: Option<Instant>,
    /// Working directory.
    cwd: std::path::PathBuf,
}

impl FileIndex {
    /// Create a new file index for the given directory.
    pub fn new(cwd: impl Into<std::path::PathBuf>) -> Self {
        Self {
            entries: Vec::new(),
            last_refresh: None,
            cwd: cwd.into(),
        }
    }

    /// Check if the cache is still valid.
    pub fn is_cache_valid(&self) -> bool {
        self.last_refresh
            .map(|t| t.elapsed() < Duration::from_secs(CACHE_TTL_SECS))
            .unwrap_or(false)
    }

    /// Get file suggestions for a query from the cached entries.
    pub fn get_suggestions(&self, query: &str, max_results: i32) -> Vec<FileSuggestion> {
        self.search_cached_entries(query, max_results)
    }

    /// Search files using fuzzy matching.
    fn search_cached_entries(&self, query: &str, max_results: i32) -> Vec<FileSuggestion> {
        if query.is_empty() || self.entries.is_empty() || max_results <= 0 {
            return Vec::new();
        }

        let pattern = Pattern::new(
            query,
            CaseMatching::Smart,
            Normalization::Smart,
            AtomKind::Fuzzy,
        );
        let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
        let mut scored = Vec::new();

        for entry in &self.entries {
            let haystack = entry.matcher_text.slice(..);
            let Some(score) = pattern.score(haystack, &mut matcher) else {
                continue;
            };
            let mut indices = Vec::<u32>::new();
            let _ = pattern.indices(haystack, &mut matcher, &mut indices);
            indices.sort_unstable();
            indices.dedup();
            scored.push(FileSuggestion {
                path: entry.path.clone(),
                display_text: entry.path.clone(),
                score,
                match_indices: indices.into_iter().map(|i| i as i32).collect(),
                is_directory: entry.is_directory,
            });
        }

        scored.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| a.path.cmp(&b.path))
                .then_with(|| b.is_directory.cmp(&a.is_directory))
        });
        scored.truncate(max_results as usize);
        scored
    }

    /// Refresh the shared cache when the TTL has expired.
    ///
    /// Discovery runs outside the write lock so cached reads are not blocked
    /// while git or ripgrep scans the filesystem.
    pub async fn refresh_if_stale(index: &SharedFileIndex) {
        let cwd = {
            let guard = index.read().await;
            if guard.is_cache_valid() {
                return;
            }
            guard.cwd.clone()
        };
        let result = discover_files(&cwd).await;
        let mut guard = index.write().await;
        if guard.cwd == cwd && !guard.is_cache_valid() {
            guard.apply_discovery(result);
        }
    }

    /// Force-refresh the shared file index.
    pub async fn refresh(index: &SharedFileIndex) {
        let cwd = {
            let guard = index.read().await;
            guard.cwd.clone()
        };
        let result = discover_files(&cwd).await;
        let mut guard = index.write().await;
        if guard.cwd == cwd {
            guard.apply_discovery(result);
        }
    }

    fn apply_discovery(&mut self, result: DiscoveryResult) {
        self.entries = entries_from_discovery(result);
        self.last_refresh = Some(Instant::now());
    }

    /// Force a background refresh.
    pub fn refresh_background(index: SharedFileIndex) {
        tokio::spawn(async move {
            Self::refresh(&index).await;
        });
    }

    /// Get the current file count.
    pub fn file_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| !entry.is_directory)
            .count()
    }

    /// Get the current directory count.
    pub fn directory_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.is_directory)
            .count()
    }
}

fn entries_from_discovery(result: DiscoveryResult) -> Vec<IndexedPath> {
    let mut entries = Vec::with_capacity(result.files.len() + result.directories.len());
    entries.extend(
        result
            .directories
            .into_iter()
            .map(|path| indexed_path(ensure_trailing_slash(&path), true)),
    );
    entries.extend(
        result
            .files
            .into_iter()
            .map(|path| indexed_path(path, false)),
    );
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    entries.dedup_by(|a, b| a.path == b.path && a.is_directory == b.is_directory);
    entries
}

fn indexed_path(path: String, is_directory: bool) -> IndexedPath {
    IndexedPath {
        matcher_text: Utf32String::from(path.as_str()),
        path,
        is_directory,
    }
}

fn ensure_trailing_slash(path: &str) -> String {
    if path.ends_with('/') {
        path.to_string()
    } else {
        format!("{path}/")
    }
}

/// Discover project files using git or ripgrep fallback.
///
/// Strategy (aligned with Claude Code):
/// 1. Try: `git ls-files --recurse-submodules`
/// 2. Fallback: `rg --files --follow --hidden --glob '!.git/'`
pub async fn discover_files(cwd: &Path) -> DiscoveryResult {
    // Try git ls-files first
    if let Some(result) = try_git_ls_files(cwd).await {
        return result;
    }

    // Fallback to ripgrep
    if let Some(result) = try_ripgrep_files(cwd).await {
        return result;
    }

    // Empty result if both fail
    DiscoveryResult::default()
}

/// Try to discover files using git ls-files.
async fn try_git_ls_files(cwd: &Path) -> Option<DiscoveryResult> {
    let output = Command::new("git")
        .arg("ls-files")
        .arg("--recurse-submodules")
        .current_dir(cwd)
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let files: Vec<String> = stdout
        .lines()
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();

    let directories = extract_directories(&files);

    Some(DiscoveryResult { files, directories })
}

/// Try to discover files using ripgrep.
async fn try_ripgrep_files(cwd: &Path) -> Option<DiscoveryResult> {
    let output = Command::new("rg")
        .args(["--files", "--follow", "--hidden", "--glob", "!.git/"])
        .current_dir(cwd)
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let files: Vec<String> = stdout
        .lines()
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();

    let directories = extract_directories(&files);

    Some(DiscoveryResult { files, directories })
}

/// Extract directory prefixes from file paths.
///
/// Example: `src/components/Button.tsx` → `["src/", "src/components/"]`
pub fn extract_directories(files: &[String]) -> Vec<String> {
    let mut dirs: HashSet<String> = HashSet::new();

    for path in files {
        let components = path
            .split('/')
            .filter(|component| !component.is_empty())
            .collect::<Vec<_>>();
        for end in 1..components.len() {
            dirs.insert(format!("{}/", components[..end].join("/")));
        }
    }

    let mut result: Vec<String> = dirs.into_iter().collect();
    result.sort();
    result
}

/// Shared file index for use across the application.
pub type SharedFileIndex = Arc<RwLock<FileIndex>>;

/// Create a shared file index.
pub fn create_shared_index(cwd: impl Into<std::path::PathBuf>) -> SharedFileIndex {
    Arc::new(RwLock::new(FileIndex::new(cwd)))
}

#[cfg(test)]
#[path = "index.test.rs"]
mod tests;
