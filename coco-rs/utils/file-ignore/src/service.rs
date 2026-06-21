//! File ignore service for consistent file filtering.

use ignore::WalkBuilder;
use ignore::overrides::OverrideBuilder;
use std::path::Path;
use std::path::PathBuf;
use walkdir::WalkDir;

use crate::config::IgnoreConfig;

/// Agent-level ignore filename.
///
/// `.agentignore` uses the same gitignore syntax as `.ignore`, but is the
/// agreed name for **AI-agent-only** exclusions — checked-in files the
/// user wants hidden from Claude / coco-rs (secrets, fixtures, generated
/// artifacts) without affecting the rest of git tooling.
///
/// It is wired into the walker via [`ignore::WalkBuilder::add_custom_ignore_filename`],
/// which the `ignore` crate honors **independently** of the `.gitignore` /
/// `.ignore` toggles — so `.agentignore` stays in force even in the Glob
/// tool's `--no-ignore` discovery mode. That is the deliberate split from
/// `.ignore`: same syntax, stronger (agent-scoped, always-on) intent.
pub const AGENT_IGNORE_FILE: &str = ".agentignore";

/// Standard ripgrep `.ignore` filename (handled natively by the `ignore`
/// crate's `.ignore(bool)` toggle).
pub const IGNORE_FILE: &str = ".ignore";

/// Ignore file names discovered by [`find_ignore_files`] (both the native
/// `.ignore` and the agent-level `.agentignore`).
pub const IGNORE_FILES: &[&str] = &[IGNORE_FILE, AGENT_IGNORE_FILE];
use crate::patterns::BINARY_FILE_PATTERNS;
use crate::patterns::COMMON_DIRECTORY_EXCLUDES;
use crate::patterns::COMMON_IGNORE_PATTERNS;
use crate::patterns::SYSTEM_FILE_EXCLUDES;

/// Service for handling file ignore patterns.
///
/// Provides consistent file filtering behavior across all file-related
/// operations (glob, list_dir, grep, file_search, etc.).
///
/// # Example
///
/// ```rust,no_run
/// use coco_file_ignore::{IgnoreService, IgnoreConfig};
/// use std::path::Path;
///
/// let service = IgnoreService::with_defaults();
/// let walker = service.create_walk_builder(Path::new("."));
///
/// for entry in walker.build() {
///     match entry {
///         Ok(e) => println!("{}", e.path().display()),
///         Err(e) => eprintln!("Error: {}", e),
///     }
/// }
/// ```
#[derive(Debug, Clone)]
pub struct IgnoreService {
    config: IgnoreConfig,
}

impl IgnoreService {
    /// Create a new service with the given configuration.
    pub fn new(config: IgnoreConfig) -> Self {
        Self { config }
    }

    /// Create a new service with default configuration.
    ///
    /// Defaults:
    /// - Respects `.gitignore` files
    /// - Respects `.ignore` files (ripgrep native support)
    /// - Respects `.agentignore` files (agent-level exclusions)
    /// - Excludes hidden files
    /// - Does not follow symlinks
    pub fn with_defaults() -> Self {
        Self::new(IgnoreConfig::default())
    }

    /// Create a WalkBuilder with all ignore rules applied.
    ///
    /// The returned WalkBuilder is configured to:
    /// - Respect `.gitignore` if enabled
    /// - Respect `.ignore` if enabled (ripgrep native support)
    /// - Handle hidden files according to config
    /// - Apply custom exclude patterns
    ///
    /// # Arguments
    ///
    /// * `root` - The root directory to start walking from
    ///
    /// # Returns
    ///
    /// A configured `WalkBuilder` ready for traversal.
    pub fn create_walk_builder(&self, root: &Path) -> WalkBuilder {
        let mut builder = WalkBuilder::new(root);

        // Configure gitignore handling
        builder
            .git_ignore(self.config.respect_gitignore)
            .git_global(self.config.respect_gitignore)
            .git_exclude(self.config.respect_gitignore);

        // Configure native `.ignore` (and `.rgignore`) handling. This is the
        // `ignore` crate's own toggle — driving it directly (rather than the
        // old `add_custom_ignore_filename(".ignore")`, which double-registered
        // a file the crate already reads natively and left the toggle a no-op)
        // means `respect_ignore = false` actually disables `.ignore`.
        builder.ignore(self.config.respect_ignore);

        // Configure `.agentignore`. Registered as a custom ignore filename,
        // which the `ignore` crate applies regardless of the git/ignore
        // toggles above — so agent-hidden files stay hidden even when a caller
        // (e.g. Glob discovery) turns gitignore and `.ignore` off.
        if self.config.respect_agentignore {
            builder.add_custom_ignore_filename(AGENT_IGNORE_FILE);
        }

        // Configure hidden files and symlinks
        builder
            .hidden(!self.config.include_hidden)
            .follow_links(self.config.follow_links)
            .require_git(false); // Don't require git repo

        // Apply custom exclude patterns
        if !self.config.custom_excludes.is_empty()
            && let Ok(overrides) = self.build_overrides(root)
        {
            builder.overrides(overrides);
        }

        builder
    }

    /// Build override matcher for custom exclude patterns.
    fn build_overrides(&self, root: &Path) -> Result<ignore::overrides::Override, ignore::Error> {
        let mut override_builder = OverrideBuilder::new(root);
        for pattern in &self.config.custom_excludes {
            // Prefix with ! for exclusion in override syntax
            override_builder.add(&format!("!{pattern}"))?;
        }
        override_builder.build()
    }

    /// Get common ignore patterns for basic operations.
    ///
    /// Returns patterns like `**/node_modules/**`, `**/.git/**`, etc.
    pub fn get_core_patterns() -> &'static [&'static str] {
        COMMON_IGNORE_PATTERNS
    }

    /// Get all default exclude patterns combined.
    ///
    /// Includes:
    /// - Common ignore patterns (node_modules, .git, etc.)
    /// - Binary file patterns (*.exe, *.dll, etc.)
    /// - Common directory excludes (dist, build, etc.)
    /// - System file excludes (.DS_Store, etc.)
    pub fn get_default_excludes() -> Vec<&'static str> {
        let mut patterns = Vec::new();
        patterns.extend(COMMON_IGNORE_PATTERNS);
        patterns.extend(BINARY_FILE_PATTERNS);
        patterns.extend(COMMON_DIRECTORY_EXCLUDES);
        patterns.extend(SYSTEM_FILE_EXCLUDES);
        patterns
    }

    /// Get the current configuration.
    pub fn config(&self) -> &IgnoreConfig {
        &self.config
    }

    /// Create a `PathChecker` for checking individual paths against ignore rules.
    ///
    /// Unlike `create_walk_builder` which provides directory traversal, the
    /// `PathChecker` checks specific paths against `.gitignore`, `.ignore`,
    /// global gitignore, and custom exclude rules. Designed for filtering
    /// a list of known paths (e.g., LSP results).
    pub fn create_path_checker(&self, root: &Path) -> crate::PathChecker {
        crate::PathChecker::new(root, &self.config)
    }
}

impl Default for IgnoreService {
    fn default() -> Self {
        Self::with_defaults()
    }
}

/// Find all .ignore files for a given root path.
///
/// This is useful for external tools that need explicit file paths
/// rather than built-in ignore handling.
///
/// Note: ripgrep natively supports .ignore files, so this function
/// is typically not needed when using rg directly.
///
/// Searches:
/// 1. UP - from root through parent directories (for project-level ignores)
/// 2. DOWN - into subdirectories (for nested ignores like src/.ignore)
///
/// # Arguments
///
/// * `root` - The root directory to search from
///
/// # Returns
///
/// A vector of paths to found ignore files.
pub fn find_ignore_files(root: &Path) -> Vec<PathBuf> {
    let mut ignore_files = Vec::new();

    // 1. Walk UP to parent directories (for project-level ignores)
    // Stop at git root or max depth to avoid walking all the way to filesystem root
    const MAX_PARENT_DEPTH: usize = 20;
    let mut current = Some(root.to_path_buf());
    let mut depth = 0;
    while let Some(dir) = current {
        for name in IGNORE_FILES {
            let path = dir.join(name);
            if path.exists() {
                ignore_files.push(path);
            }
        }
        depth += 1;
        // Stop at git root or max depth
        if depth >= MAX_PARENT_DEPTH || dir.join(".git").exists() {
            break;
        }
        current = dir.parent().map(Path::to_path_buf);
    }

    // 2. Walk DOWN into subdirectories (for nested ignores)
    if root.is_dir() {
        for entry in WalkDir::new(root)
            .max_depth(10) // Limit depth to avoid performance issues
            .into_iter()
            .filter_map(Result::ok)
        {
            if entry.file_type().is_file() {
                let name = entry.file_name().to_string_lossy();
                if IGNORE_FILES.iter().any(|&n| n == name) {
                    let path = entry.path().to_path_buf();
                    // Avoid duplicates (root was already added in step 1)
                    if !ignore_files.contains(&path) {
                        ignore_files.push(path);
                    }
                }
            }
        }
    }

    ignore_files
}

#[cfg(test)]
#[path = "service.test.rs"]
mod tests;
