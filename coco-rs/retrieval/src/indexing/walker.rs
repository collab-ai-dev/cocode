//! File walker using codex-file-ignore.

use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

use coco_file_ignore::IgnoreConfig;
use coco_file_ignore::IgnoreService;

use crate::error::Result;
use crate::indexing::file_filter::FileFilter;
use crate::indexing::file_filter::FilterSummary;

/// File walker for traversing directories.
///
/// Uses codex-file-ignore for .gitignore and .ignore support.
/// Handles symlinks with loop detection.
pub struct FileWalker {
    ignore_service: IgnoreService,
    max_file_size: u64,
    follow_symlinks: bool,
    file_filter: FileFilter,
}

impl FileWalker {
    /// Create a new file walker with default filter (uses default text extensions).
    pub fn new(workdir: &Path, max_file_size_mb: i32) -> Self {
        Self::with_filter(workdir, max_file_size_mb, &[], &[], &[], &[])
    }

    /// Create a file walker with custom filter configuration.
    pub fn with_filter(
        workdir: &Path,
        max_file_size_mb: i32,
        include_dirs: &[String],
        exclude_dirs: &[String],
        include_extensions: &[String],
        exclude_extensions: &[String],
    ) -> Self {
        let config = IgnoreConfig::respecting_all();
        Self {
            ignore_service: IgnoreService::new(config),
            max_file_size: (max_file_size_mb as u64) * 1024 * 1024,
            follow_symlinks: true,
            file_filter: FileFilter::new(
                workdir,
                include_dirs,
                exclude_dirs,
                include_extensions,
                exclude_extensions,
            ),
        }
    }

    /// Create a file walker with custom symlink behavior.
    pub fn with_symlink_follow(workdir: &Path, max_file_size_mb: i32, follow: bool) -> Self {
        let config = IgnoreConfig::respecting_all();
        Self {
            ignore_service: IgnoreService::new(config),
            max_file_size: (max_file_size_mb as u64) * 1024 * 1024,
            follow_symlinks: follow,
            file_filter: FileFilter::new(workdir, &[], &[], &[], &[]),
        }
    }

    /// Get the filter summary.
    pub fn filter_summary(&self) -> FilterSummary {
        self.file_filter.summary()
    }

    /// Walk a directory and return file paths.
    ///
    /// Handles symlinks with loop detection to avoid infinite recursion.
    /// Symlinks are resolved to their canonical paths.
    pub fn walk(&self, root: &Path) -> Result<Vec<PathBuf>> {
        let mut builder = self.ignore_service.create_walk_builder(root);

        // Configure symlink following
        builder.follow_links(self.follow_symlinks);

        let mut files = Vec::new();
        let mut seen_paths: HashSet<PathBuf> = HashSet::new();

        for entry in builder.build() {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            let path = entry.path();

            // Skip directories
            if path.is_dir() {
                continue;
            }

            // Handle symlinks
            let resolved_path = if path.is_symlink() {
                match self.resolve_symlink(path) {
                    Some(p) => p,
                    None => continue, // Skip broken symlinks
                }
            } else {
                path.to_path_buf()
            };

            // Skip if we've already seen this file (handles symlink loops)
            if !seen_paths.insert(resolved_path.clone()) {
                continue;
            }

            // Skip files that are too large
            if let Ok(metadata) = resolved_path.metadata()
                && metadata.len() > self.max_file_size
            {
                continue;
            }

            // Skip files that don't match filter criteria
            if !self.file_filter.should_include(&resolved_path) {
                continue;
            }

            // Return original path for symlinks (not resolved) for consistency
            files.push(path.to_path_buf());
        }

        Ok(files)
    }

    /// Resolve a symlink to its canonical path.
    ///
    /// Returns None if the symlink is broken or points outside the filesystem.
    fn resolve_symlink(&self, path: &Path) -> Option<PathBuf> {
        // Try to get canonical path (resolves all symlinks)
        match path.canonicalize() {
            Ok(canonical) => {
                // Verify the target exists and is a file
                if canonical.is_file() {
                    Some(canonical)
                } else {
                    None
                }
            }
            Err(_) => None, // Broken symlink
        }
    }
}

#[cfg(test)]
#[path = "walker.test.rs"]
mod tests;
