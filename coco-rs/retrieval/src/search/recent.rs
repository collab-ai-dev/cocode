//! Recently Edited Files Tracking.
//!
//! LRU cache for tracking recently accessed file paths.
//! Provides temporal relevance signal for search results.
//!
//! Note: Only stores paths, not content. Content is read fresh on demand
//! to avoid consistency issues with stale cached chunks.

use lru::LruCache;
use std::num::NonZeroUsize;
use std::path::Path;
use std::path::PathBuf;
use std::time::Instant;

/// LRU cache for recently edited/accessed files.
///
/// Tracks file paths and access times only. Content is read on demand
/// to ensure consistency (no stale cached chunks).
pub struct RecentFilesCache {
    cache: LruCache<PathBuf, Instant>,
}

impl RecentFilesCache {
    /// Create a new recent files cache with the specified capacity.
    ///
    /// # Arguments
    /// * `max_entries` - Maximum number of files to track (LRU eviction)
    pub fn new(max_entries: usize) -> Self {
        let capacity = NonZeroUsize::new(max_entries).unwrap_or(NonZeroUsize::MIN);
        Self {
            cache: LruCache::new(capacity),
        }
    }

    /// Notify the cache that a file has been accessed or edited.
    ///
    /// This should be called when:
    /// - A file is opened in the editor
    /// - A file is modified
    /// - A file is explicitly marked as relevant
    pub fn notify_file_accessed(&mut self, path: impl AsRef<Path>) {
        self.cache.put(path.as_ref().to_path_buf(), Instant::now());
    }

    /// Mark a file as accessed (touch) without adding if not present.
    ///
    /// Moves the file to the front of the LRU if it exists.
    /// Returns false if the file is not in the cache.
    pub fn touch(&mut self, path: impl AsRef<Path>) -> bool {
        let key = path.as_ref().to_path_buf();
        if self.cache.contains(&key) {
            self.cache.promote(&key);
            // Update timestamp
            if let Some(ts) = self.cache.get_mut(&key) {
                *ts = Instant::now();
            }
            true
        } else {
            false
        }
    }

    /// Remove a file from the cache.
    ///
    /// Call this when a file is deleted.
    pub fn remove(&mut self, path: impl AsRef<Path>) -> bool {
        self.cache.pop(&path.as_ref().to_path_buf()).is_some()
    }

    /// Get recent file paths, ordered by most recently accessed first.
    ///
    /// # Arguments
    /// * `limit` - Maximum number of paths to return
    pub fn get_recent_paths(&self, limit: usize) -> Vec<PathBuf> {
        self.cache
            .iter()
            .take(limit)
            .map(|(path, _)| path.clone())
            .collect()
    }

    /// Get recent file paths with their age in seconds.
    ///
    /// Returns (path, age_in_seconds) pairs for scoring purposes.
    pub fn get_recent_paths_with_age(&self, limit: usize) -> Vec<(PathBuf, u64)> {
        let now = Instant::now();
        self.cache
            .iter()
            .take(limit)
            .map(|(path, ts)| (path.clone(), now.duration_since(*ts).as_secs()))
            .collect()
    }

    /// Get all files currently in the cache.
    pub fn files(&self) -> Vec<PathBuf> {
        self.cache.iter().map(|(path, _)| path.clone()).collect()
    }

    /// Get the number of files in the cache.
    pub fn len(&self) -> usize {
        self.cache.len()
    }

    /// Check if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }

    /// Check if a file is in the cache.
    pub fn contains(&self, path: impl AsRef<Path>) -> bool {
        self.cache.contains(&path.as_ref().to_path_buf())
    }

    /// Clear all entries from the cache.
    pub fn clear(&mut self) {
        self.cache.clear();
    }

    /// Get the access time for a specific file if it exists.
    pub fn get_access_time(&self, path: impl AsRef<Path>) -> Option<Instant> {
        self.cache.peek(&path.as_ref().to_path_buf()).copied()
    }
}

impl Default for RecentFilesCache {
    fn default() -> Self {
        // Default capacity: 50 files
        Self::new(50)
    }
}

#[cfg(test)]
#[path = "recent.test.rs"]
mod tests;
