//! `.coco/rules/*.md` discovery + frontmatter `paths` glob matching.
//!
//! ## Two flavours of rule
//!
//! - **Unconditional** — no `paths:` frontmatter. Eager-loaded once
//!   per session by [`crate::discover_memory_files`] for every dir
//!   `root → CWD`. Per-file traversal does NOT re-load these in dirs
//!   already covered eagerly; for **descendants** of CWD it does
//!   (those weren't eager-loaded).
//! - **Conditional** — has `paths: <glob list>` frontmatter. Loaded
//!   per-file-trigger when the trigger file matches one of the
//!   patterns. Uses gitignore-style matching via the `ignore` crate,
//!   normalised to `relative_to(base_dir)` paths.
//!
//! ## Filename matching for rules
//!
//! Rule files are content-defined (frontmatter `paths`), not basename-
//! defined — so the AGENTS.md / case-insensitive divergence in
//! [`crate::memory_filenames`] does NOT apply here. We accept any
//! `.md` file (case-insensitively for the extension only) under
//! `<dir>/.coco/rules/**/*` recursively.

use std::path::Path;
use std::path::PathBuf;

use crate::memory_discovery::MemoryFileSource;
use crate::nested_memory::LoadedMemoryEntry;

/// One rule file's parsed shape.
#[derive(Debug, Clone)]
pub struct RuleFile {
    pub path: PathBuf,
    pub raw_content: String,
    pub content: String,
    /// Glob patterns from frontmatter `paths`. `None` ⇒ unconditional
    /// (loaded eagerly). `Some(...)` ⇒ conditional (loaded only when a
    /// triggered file matches one of the patterns).
    pub paths: Option<Vec<String>>,
}

/// Parse the `paths` frontmatter field of a rule file.
///
/// Splits on commas (respecting brace nesting), then expands `{a,b}`
/// brace alternatives into separate patterns, drops trailing `/**`
/// (gitignore normalises `path/**` to `path` matching path +
/// descendants), and treats an empty result or pure `**` as "no
/// globs" (= unconditional).
///
/// Accepts:
/// - YAML string: `paths: "src/**/*.ts, lib/**/*.rs"`
/// - YAML list: `paths: [src/**/*.ts, lib/**/*.rs]`
/// - YAML brace expansion: `paths: "src/*.{ts,tsx}"` → `["src/*.ts", "src/*.tsx"]`
pub fn parse_paths_field(input: &coco_frontmatter::FrontmatterValue) -> Option<Vec<String>> {
    let raw_patterns: Vec<String> = match input {
        coco_frontmatter::FrontmatterValue::String(s) => split_paths_in_string(s),
        coco_frontmatter::FrontmatterValue::Sequence(items) => items
            .iter()
            .flat_map(|v| match v {
                coco_frontmatter::FrontmatterValue::String(s) => split_paths_in_string(s),
                _ => Vec::new(),
            })
            .collect(),
        _ => return None,
    };

    let normalised: Vec<String> = raw_patterns
        .into_iter()
        .flat_map(|p| expand_braces(&p))
        // gitignore: `path/**` matches `path` and everything inside.
        // Strip the trailing `/**` suffix.
        .map(|p| p.strip_suffix("/**").map(String::from).unwrap_or(p))
        .filter(|p| !p.is_empty())
        .collect();

    // Empty or all `**` → no globs (= load unconditionally / never match).
    if normalised.is_empty() || normalised.iter().all(|p| p == "**") {
        return None;
    }
    Some(normalised)
}

/// Comma-split that respects brace nesting (`{a,b}` stays together).
/// Brace expansion happens in [`expand_braces`].
fn split_paths_in_string(input: &str) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut brace_depth: i32 = 0;
    for ch in input.chars() {
        match ch {
            '{' => {
                brace_depth += 1;
                current.push(ch);
            }
            '}' => {
                brace_depth -= 1;
                current.push(ch);
            }
            ',' if brace_depth == 0 => {
                let trimmed = current.trim();
                if !trimmed.is_empty() {
                    parts.push(trimmed.to_string());
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        parts.push(trimmed.to_string());
    }
    parts
}

/// Recursive brace expansion: `src/*.{ts,tsx}` → `[src/*.ts, src/*.tsx]`,
/// `{a,b}/{c,d}` → `[a/c, a/d, b/c, b/d]`. Returns the input unchanged
/// when no brace pair is present.
fn expand_braces(pattern: &str) -> Vec<String> {
    // Find first `{...}`. Bail out if no opening brace.
    let Some(open) = pattern.find('{') else {
        return vec![pattern.to_string()];
    };
    // Find matching `}` after `open`. Innermost-first: scan for next `}`.
    let after_open = &pattern[open + 1..];
    let Some(close_rel) = after_open.find('}') else {
        return vec![pattern.to_string()];
    };
    let close = open + 1 + close_rel;
    let prefix = &pattern[..open];
    let alternatives = &pattern[open + 1..close];
    let suffix = &pattern[close + 1..];

    let mut out: Vec<String> = Vec::new();
    for alt in alternatives.split(',') {
        let combined = format!("{prefix}{}{suffix}", alt.trim());
        // Recurse for nested braces in either prefix or suffix.
        out.extend(expand_braces(&combined));
    }
    out
}

/// Recursively walk `rules_dir` for `*.md` files, parsing each into a
/// [`RuleFile`]. Filters by `conditional` flag:
/// - `conditional=false` ⇒ only files without `paths:` frontmatter.
/// - `conditional=true` ⇒ only files with `paths:` frontmatter.
///
/// Empty result on read errors (`ENOENT`/`EACCES`/`ENOTDIR`) — defensive
/// read, matching expected behaviour on inaccessible paths.
///
/// Cycle detection on visited directories.
pub fn collect_rule_files(rules_dir: &Path, conditional: bool) -> Vec<RuleFile> {
    let mut out: Vec<RuleFile> = Vec::new();
    let mut visited: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    walk_rules_dir(rules_dir, conditional, &mut out, &mut visited);
    out
}

fn walk_rules_dir(
    dir: &Path,
    conditional: bool,
    out: &mut Vec<RuleFile>,
    visited: &mut std::collections::HashSet<PathBuf>,
) {
    // Cycle guard via canonical path (handles symlinked rules dirs).
    let canonical = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    if !visited.insert(canonical) {
        return;
    }

    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<PathBuf> = Vec::new();
    let mut subdirs: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if meta.is_dir() {
            subdirs.push(path);
        } else if meta.is_file() {
            // Case-insensitive .md suffix check.
            if path
                .extension()
                .and_then(|e| e.to_str())
                .map(|s| s.eq_ignore_ascii_case("md"))
                .unwrap_or(false)
            {
                files.push(path);
            }
        }
    }
    // Stable order so identical trees produce identical sequences.
    files.sort();
    subdirs.sort();

    for path in files {
        if let Some(rule) = read_rule_file(&path)
            && rule.paths.is_some() == conditional
        {
            out.push(rule);
        }
    }
    for sub in subdirs {
        walk_rules_dir(&sub, conditional, out, visited);
    }
}

fn read_rule_file(path: &Path) -> Option<RuleFile> {
    let raw_content = std::fs::read_to_string(path).ok()?;
    let fm = coco_frontmatter::parse(&raw_content);
    let paths = fm.data.get("paths").and_then(parse_paths_field);
    Some(RuleFile {
        path: path.to_path_buf(),
        raw_content,
        content: fm.content,
        paths,
    })
}

/// Filter `rules` to only those whose `paths` glob matches `target_file`,
/// resolved relative to `base_dir`.
///
/// `base_dir` for Project rules is `dirname(dirname(rules_dir))` — i.e.
/// the dir hosting the `.coco/rules/` subtree. For Managed/User it's
/// the original CWD.
///
/// Matching uses [`ignore`] (gitignore semantics).
pub fn filter_rules_matching(
    rules: Vec<RuleFile>,
    target_file: &Path,
    base_dir: &Path,
) -> Vec<RuleFile> {
    use ignore::gitignore::GitignoreBuilder;

    // Compute relative path for matching. Reject paths outside base_dir
    // (would yield `../..` which doesn't fit gitignore semantics).
    let target_canon = target_file
        .canonicalize()
        .unwrap_or_else(|_| target_file.to_path_buf());
    let base_canon = base_dir
        .canonicalize()
        .unwrap_or_else(|_| base_dir.to_path_buf());
    let Ok(rel) = target_canon.strip_prefix(&base_canon) else {
        return Vec::new();
    };
    if rel.as_os_str().is_empty() || rel.starts_with("..") || rel.is_absolute() {
        return Vec::new();
    }

    rules
        .into_iter()
        .filter(|r| {
            let Some(globs) = &r.paths else {
                return false;
            };
            // Build a one-shot Gitignore matcher from the rule's globs.
            // `add_line` matches gitignore's pattern syntax; failures
            // fall through as "no match" rather than poisoning the
            // whole filter.
            let mut builder = GitignoreBuilder::new(&base_canon);
            for g in globs {
                let _ = builder.add_line(None, g);
            }
            let Ok(gi) = builder.build() else {
                return false;
            };
            gi.matched(rel, false).is_ignore()
        })
        .collect()
}

/// Convert a [`RuleFile`] into a [`LoadedMemoryEntry`] tagged with the
/// given source. Used by the per-file traversal to feed entries into
/// the dedup pipeline.
pub(crate) fn rule_to_entry(rule: RuleFile, source: MemoryFileSource) -> LoadedMemoryEntry {
    LoadedMemoryEntry {
        path: rule.path,
        content: rule.content,
        raw_content: rule.raw_content,
        source,
    }
}

#[cfg(test)]
#[path = "memory_rules.test.rs"]
mod tests;
