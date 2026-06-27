//! File filtering for indexing.
//!
//! Provides configurable file filtering with:
//! - Directory include/exclude lists
//! - Extension include/exclude lists
//! - Default text file extensions as fallback

use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

/// File filter configuration.
#[derive(Debug, Clone)]
pub struct FileFilter {
    /// Directories to include (relative to workdir).
    include_dirs: HashSet<PathBuf>,
    /// Directories to exclude (relative to workdir).
    exclude_dirs: HashSet<PathBuf>,
    /// Extensions to include (empty = use defaults).
    include_extensions: HashSet<String>,
    /// Extensions to exclude (patterns like "test.ts" supported).
    exclude_extensions: HashSet<String>,
    /// Working directory for relative path resolution.
    workdir: PathBuf,
}

impl FileFilter {
    /// Create a new file filter from config.
    pub fn new(
        workdir: &Path,
        include_dirs: &[String],
        exclude_dirs: &[String],
        include_extensions: &[String],
        exclude_extensions: &[String],
    ) -> Self {
        Self {
            workdir: workdir.to_path_buf(),
            include_dirs: include_dirs.iter().map(PathBuf::from).collect(),
            exclude_dirs: exclude_dirs.iter().map(PathBuf::from).collect(),
            include_extensions: include_extensions
                .iter()
                .map(|s| s.to_lowercase())
                .collect(),
            exclude_extensions: exclude_extensions
                .iter()
                .map(|s| s.to_lowercase())
                .collect(),
        }
    }

    /// Check if a file should be included based on filter config.
    ///
    /// Returns `true` if the file should be indexed.
    pub fn should_include(&self, path: &Path) -> bool {
        let rel_path = path.strip_prefix(&self.workdir).unwrap_or(path);

        // 1. Check exclude_dirs
        for exclude in &self.exclude_dirs {
            if rel_path.starts_with(exclude) {
                return false;
            }
        }

        // 2. Check include_dirs (whitelist mode if non-empty)
        if !self.include_dirs.is_empty() {
            let in_included = self
                .include_dirs
                .iter()
                .any(|inc| rel_path.starts_with(inc));
            if !in_included {
                return false;
            }
        }

        // 3. Check exclude_extensions (compound patterns like .test.ts)
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_lowercase())
            .unwrap_or_default();

        for pattern in &self.exclude_extensions {
            if filename.ends_with(&format!(".{pattern}")) {
                return false;
            }
        }

        // 4. Check include_extensions (whitelist mode if non-empty)
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_lowercase())
            .unwrap_or_default();

        if !self.include_extensions.is_empty() {
            return self.include_extensions.contains(&ext);
        }

        // 5. Use default text file extensions
        is_default_text_file(path)
    }

    /// Get a summary of the filter configuration.
    pub fn summary(&self) -> FilterSummary {
        FilterSummary {
            include_dirs: self
                .include_dirs
                .iter()
                .map(|p| p.display().to_string())
                .collect(),
            exclude_dirs: self
                .exclude_dirs
                .iter()
                .map(|p| p.display().to_string())
                .collect(),
            include_extensions: self.include_extensions.iter().cloned().collect(),
            exclude_extensions: self.exclude_extensions.iter().cloned().collect(),
        }
    }
}

/// Summary of active file filters.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct FilterSummary {
    /// Directories to include (empty = all).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include_dirs: Vec<String>,
    /// Directories to exclude.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude_dirs: Vec<String>,
    /// Extensions to include (empty = defaults).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include_extensions: Vec<String>,
    /// Extensions to exclude.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude_extensions: Vec<String>,
}

impl FilterSummary {
    /// Check if any filters are configured.
    pub fn has_filters(&self) -> bool {
        !self.include_dirs.is_empty()
            || !self.exclude_dirs.is_empty()
            || !self.include_extensions.is_empty()
            || !self.exclude_extensions.is_empty()
    }

    /// Format as a human-readable string for CLI/LLM output.
    pub fn to_display_string(&self) -> String {
        let mut parts = Vec::new();

        if !self.include_dirs.is_empty() {
            parts.push(format!("Include dirs: [{}]", self.include_dirs.join(", ")));
        }
        if !self.exclude_dirs.is_empty() {
            parts.push(format!("Exclude dirs: [{}]", self.exclude_dirs.join(", ")));
        }
        if !self.include_extensions.is_empty() {
            parts.push(format!(
                "Include extensions: [{}]",
                self.include_extensions.join(", ")
            ));
        }
        if !self.exclude_extensions.is_empty() {
            parts.push(format!(
                "Exclude extensions: [{}]",
                self.exclude_extensions.join(", ")
            ));
        }

        if parts.is_empty() {
            "Using default text file extensions".to_string()
        } else {
            parts.join(" | ")
        }
    }
}

/// Check if a file is a default text file based on extension.
///
/// This is the fallback when no include_extensions are configured.
fn is_default_text_file(path: &Path) -> bool {
    let text_extensions = [
        // Programming languages
        "rs",
        "go",
        "py",
        "java",
        "js",
        "jsx",
        "ts",
        "tsx",
        "c",
        "cpp",
        "cc",
        "cxx",
        "h",
        "hpp",
        "cs",
        "rb",
        "php",
        "swift",
        "kt",
        "kts",
        "scala",
        "lua",
        "sh",
        "bash",
        "zsh",
        "fish",
        "pl",
        "pm",
        "r",
        "m",
        "mm",
        "hs",
        "ex",
        "exs",
        "erl",
        "hrl",
        "clj",
        "cljs",
        "elm",
        "fs",
        "fsx",
        "ml",
        "mli",
        "nim",
        "zig",
        "v",
        "vala",
        "d",
        "dart",
        "groovy",
        "gradle",
        // Web
        "html",
        "htm",
        "css",
        "scss",
        "sass",
        "less",
        "vue",
        "svelte",
        // Data/Config
        "json",
        "yaml",
        "yml",
        "toml",
        "xml",
        "ini",
        "cfg",
        "conf",
        "properties",
        // Documentation
        "md",
        "rst",
        "txt",
        "adoc",
        // SQL
        "sql",
        // Build
        "mk",
        "cmake",
        "makefile",
        "dockerfile",
        // Other
        "proto",
        "thrift",
        "graphql",
        "gql",
    ];

    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| text_extensions.contains(&ext.to_lowercase().as_str()))
}

#[cfg(test)]
#[path = "file_filter.test.rs"]
mod tests;
