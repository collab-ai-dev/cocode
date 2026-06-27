//! Path-level ignore checker for checking individual paths against ignore rules.
//!
//! Unlike `IgnoreService` which provides directory walking via `WalkBuilder`,
//! `PathChecker` checks individual paths against `.gitignore`, `.ignore`,
//! global gitignore, and custom exclude rules. Designed for filtering a list
//! of known paths (e.g., LSP results) without directory traversal.

use std::path::Path;
use std::path::PathBuf;

use ignore::Match;
use ignore::gitignore::Gitignore;
use ignore::gitignore::GitignoreBuilder;
use walkdir::WalkDir;

use crate::config::IgnoreConfig;
use crate::matcher::PatternMatcher;

/// Ignore file names to scan when building matchers.
const GITIGNORE_FILENAME: &str = ".gitignore";
const IGNORE_FILENAME: &str = ".ignore";
/// AI-agent-only ignore file. Same syntax as `.gitignore`, intentionally
/// not in `.gitignore` because the user wants the file checked into
/// version control but **hidden from agent tooling** (secrets, fixtures,
/// generated artifacts that confuse the model). Gated by the same
/// `IgnoreConfig::respect_ignore` flag as `.ignore` — both are
/// "additional gitignore files" from the perspective of the matcher
/// chain.
const AGENTIGNORE_FILENAME: &str = ".agentignore";

/// Maximum depth when scanning for ignore files.
const MAX_SCAN_DEPTH: usize = 20;

/// Maximum depth when walking UP to find parent ignore files.
const MAX_PARENT_DEPTH: usize = 20;

/// Path-level ignore checker.
///
/// Builds a chain of gitignore-style matchers from the directory tree,
/// allowing individual paths to be checked against all applicable rules.
///
/// Supports:
/// - `.gitignore` files (including negation patterns and directory-only patterns)
/// - `.ignore` files (ripgrep native, same syntax as `.gitignore`)
/// - Global gitignore (`~/.config/git/ignore` or `core.excludesFile`)
/// - `.git/info/exclude`
/// - Custom exclude patterns from `IgnoreConfig`
/// - Hidden file filtering
///
/// Uses the `ignore` crate's `Gitignore` and `matched_path_or_any_parents()`
/// API, which is designed for checking path lists without hierarchy traversal.
pub struct PathChecker {
    /// Matchers per directory level, sorted deepest-first for precedence.
    matchers: Vec<(PathBuf, Gitignore)>,
    /// Global gitignore matcher.
    global: Gitignore,
    /// Hardcoded default pattern matcher (node_modules, .git, build, etc.).
    defaults: Option<PatternMatcher>,
    /// Whether to filter hidden files.
    filter_hidden: bool,
}

impl std::fmt::Debug for PathChecker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PathChecker")
            .field("matchers_count", &self.matchers.len())
            .field("filter_hidden", &self.filter_hidden)
            .finish()
    }
}

impl PathChecker {
    /// Build a `PathChecker` by scanning ignore files from `root`.
    ///
    /// Walks the directory tree rooted at `root` to find `.gitignore` and
    /// `.ignore` files, then builds matchers for each. Also walks UP from
    /// `root` to find parent ignore files.
    pub fn new(root: &Path, config: &IgnoreConfig) -> Self {
        // Canonicalize root to avoid symlink/relative path issues
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let mut matchers = Vec::new();

        if config.respect_gitignore || config.respect_ignore {
            collect_matchers(&root, config, &mut matchers);
        }

        // Add custom excludes as an override matcher at root level
        if !config.custom_excludes.is_empty() {
            let mut builder = GitignoreBuilder::new(&root);
            for pattern in &config.custom_excludes {
                // Errors from invalid patterns are silently ignored
                let _ = builder.add_line(None, pattern);
            }
            if let Ok(gi) = builder.build()
                && !gi.is_empty()
            {
                matchers.push((root.clone(), gi));
            }
        }

        // Sort deepest-first so child .gitignore takes precedence over parent
        matchers.sort_by(|a, b| b.0.components().count().cmp(&a.0.components().count()));

        // Build global gitignore rooted at our root (not CWD)
        // so matched_path_or_any_parents can correctly strip paths
        let global = if config.respect_gitignore {
            let (gi, _err) = GitignoreBuilder::new(&root).build_global();
            gi
        } else {
            Gitignore::empty()
        };

        let defaults = PatternMatcher::default_excludes().ok();

        Self {
            matchers,
            global,
            defaults,
            filter_hidden: !config.include_hidden,
        }
    }

    /// Check if a path should be ignored.
    ///
    /// Checks against (in order):
    /// 1. Hidden file filter (dotfiles)
    /// 2. Default hardcoded patterns (node_modules, .git, build, etc.)
    /// 3. Nearest `.gitignore` / `.ignore` matchers (deepest-first)
    /// 4. Global gitignore
    ///
    /// Respects negation patterns (`!pattern`) — a whitelisted path
    /// short-circuits to "not ignored" regardless of parent rules.
    pub fn is_ignored(&self, path: &Path) -> bool {
        // Canonicalize to match matchers built with canonicalized root
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let path = path.as_path();

        // Hidden file check
        if self.filter_hidden
            && let Some(name) = path.file_name()
        {
            let name_str = name.to_string_lossy();
            if name_str.starts_with('.') && name_str != "." && name_str != ".." {
                return true;
            }
        }

        // Default hardcoded patterns (fast, in-process)
        if let Some(ref defaults) = self.defaults {
            let path_str = path.to_string_lossy();
            if defaults.is_match(&path_str) {
                return true;
            }
        }

        let is_dir = path.is_dir();

        // Check from most specific (deepest) matcher first.
        // The starts_with guard ensures the path is under the matcher's root,
        // which is required by matched_path_or_any_parents.
        for (dir, matcher) in &self.matchers {
            if path.starts_with(dir) {
                match matcher.matched_path_or_any_parents(path, is_dir) {
                    Match::Ignore(_) => return true,
                    Match::Whitelist(_) => return false,
                    Match::None => {} // continue to parent matchers
                }
            }
        }

        // Check global gitignore (use matched() to avoid assertion
        // if path is somehow not under the global matcher's root)
        matches!(self.global.matched(path, is_dir), Match::Ignore(_))
    }

    /// Filter a slice of paths, returning only non-ignored ones.
    pub fn filter_paths<'a>(&self, paths: &'a [PathBuf]) -> Vec<&'a Path> {
        paths
            .iter()
            .map(PathBuf::as_path)
            .filter(|p| !self.is_ignored(p))
            .collect()
    }
}

/// Collect gitignore/ignore matchers from the directory tree.
///
/// Walks DOWN from `root` to find nested ignore files, and UP from `root`
/// to find parent ignore files (up to git root or MAX_PARENT_DEPTH).
fn collect_matchers(root: &Path, config: &IgnoreConfig, out: &mut Vec<(PathBuf, Gitignore)>) {
    // Walk UP from root to find parent ignore files
    let mut current = root.parent().map(Path::to_path_buf);
    let mut depth = 0;
    while let Some(dir) = current {
        if depth >= MAX_PARENT_DEPTH {
            break;
        }
        add_ignore_files_for_dir(&dir, config, out);
        // Stop at git root
        if dir.join(".git").exists() {
            break;
        }
        depth += 1;
        current = dir.parent().map(Path::to_path_buf);
    }

    // Walk DOWN from root to find nested ignore files
    // Using walkdir (not ignore's WalkBuilder) since we need to find
    // ALL directories, including those that would be ignored
    for entry in WalkDir::new(root)
        .max_depth(MAX_SCAN_DEPTH)
        .into_iter()
        .filter_map(Result::ok)
    {
        if entry.file_type().is_dir() {
            add_ignore_files_for_dir(entry.path(), config, out);
        }
    }
}

/// Check a directory for `.gitignore` and `.ignore` files and build matchers.
fn add_ignore_files_for_dir(
    dir: &Path,
    config: &IgnoreConfig,
    out: &mut Vec<(PathBuf, Gitignore)>,
) {
    // Canonicalize to match paths in is_ignored()
    let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let mut builder = GitignoreBuilder::new(&dir);
    let mut has_rules = false;

    if config.respect_gitignore {
        let gitignore_path = dir.join(GITIGNORE_FILENAME);
        if gitignore_path.is_file() {
            builder.add(&gitignore_path);
            has_rules = true;
        }

        // Also check .git/info/exclude at git root
        let exclude_path = dir.join(".git").join("info").join("exclude");
        if exclude_path.is_file() {
            builder.add(&exclude_path);
            has_rules = true;
        }
    }

    if config.respect_ignore {
        let ignore_path = dir.join(IGNORE_FILENAME);
        if ignore_path.is_file() {
            builder.add(&ignore_path);
            has_rules = true;
        }
        // `.agentignore` is the AI-only sibling of `.ignore`. Same
        // gate, same syntax — added to the SAME builder so negation /
        // ordering interacts naturally with `.ignore` rules within
        // this directory level.
        let agentignore_path = dir.join(AGENTIGNORE_FILENAME);
        if agentignore_path.is_file() {
            builder.add(&agentignore_path);
            has_rules = true;
        }
    }

    if has_rules
        && let Ok(gi) = builder.build()
        && !gi.is_empty()
    {
        out.push((dir, gi));
    }
}

#[cfg(test)]
#[path = "path_checker.test.rs"]
mod tests;
