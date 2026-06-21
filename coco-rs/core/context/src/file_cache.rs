//! LRU file read cache for repeated reads in the same turn.

use std::collections::HashMap;
use std::path::PathBuf;

/// Maximum number of cached file reads.
const MAX_CACHE_ENTRIES: usize = 64;

/// Cached file content with metadata.
#[derive(Debug, Clone)]
pub struct CachedFile {
    pub content: String,
    pub line_count: usize,
    pub byte_size: u64,
    pub last_read_ms: i64,
}

/// LRU file read cache.
#[derive(Debug, Default)]
pub struct FileReadCache {
    entries: HashMap<PathBuf, CachedFile>,
    access_order: Vec<PathBuf>,
}

impl FileReadCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Get a cached file read, if available and not stale.
    pub fn get(&mut self, path: &PathBuf) -> Option<&CachedFile> {
        if self.entries.contains_key(path) {
            // Move to end of access order (most recent)
            self.access_order.retain(|p| p != path);
            self.access_order.push(path.clone());
            self.entries.get(path)
        } else {
            None
        }
    }

    /// Insert a file read into the cache, evicting oldest if at capacity.
    pub fn insert(&mut self, path: PathBuf, content: String) {
        let line_count = content.lines().count();
        let byte_size = content.len() as u64;
        let now_ms = current_time_ms();

        // Evict oldest if at capacity
        while self.entries.len() >= MAX_CACHE_ENTRIES {
            if let Some(oldest) = self.access_order.first().cloned() {
                self.entries.remove(&oldest);
                self.access_order.remove(0);
            } else {
                break;
            }
        }

        self.access_order.retain(|p| p != &path);
        self.access_order.push(path.clone());
        self.entries.insert(
            path,
            CachedFile {
                content,
                line_count,
                byte_size,
                last_read_ms: now_ms,
            },
        );
    }

    /// Invalidate a specific file (e.g., after write/edit).
    pub fn invalidate(&mut self, path: &PathBuf) {
        self.entries.remove(path);
        self.access_order.retain(|p| p != path);
    }

    /// Clear the entire cache (e.g., at turn boundary).
    pub fn clear(&mut self) {
        self.entries.clear();
        self.access_order.clear();
    }

    /// Number of cached entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

fn current_time_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "file_cache.test.rs"]
mod tests;
