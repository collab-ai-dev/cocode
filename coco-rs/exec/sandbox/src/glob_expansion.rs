//! Bounded-depth glob expansion for deny-read patterns.
//!
//! Mirrors the codex-rs `linux-sandbox` deny-read glob behavior
//! (see `codex-rs/sandboxing/src/policy_transforms.rs` and the
//! `glob_scan_max_depth` setting). Settings `sandbox.filesystem.deny_read`
//! entries that contain glob metacharacters (`*`, `?`, `[`) cannot be
//! handed to the platform deny list as paths — we have to enumerate
//! matching files first.
//!
//! TS parity: TS delegates this to the closed-source `@anthropic-ai/sandbox-runtime`,
//! which calls user-configured `sandbox.ripgrep.command` to enumerate
//! candidate paths and then matches them against the patterns. coco-rs
//! keeps the expansion in-tree with `globset` + `walkdir`. For the
//! ripgrep code path, see the codex-rs Linux helper.
//!
//! ## Bounding
//!
//! `mandatory_deny_search_depth` (default 3) caps the directory walk so a
//! poorly-scoped glob (e.g. `**/*.env`) cannot stall the bootstrap
//! traversing the entire workspace. Patterns that need deeper expansion
//! should add explicit absolute paths instead.

use std::path::PathBuf;

use globset::Glob;
use globset::GlobSetBuilder;

/// Classify a deny-read entry. Pure absolute paths stay in
/// `denied_read_paths`; entries with glob metacharacters are queued for
/// expansion.
pub fn looks_like_glob(pattern: &str) -> bool {
    pattern.contains('*') || pattern.contains('?') || pattern.contains('[')
}

/// Expand `globs` under each `root` up to `max_depth` directory levels.
///
/// `max_depth` is interpreted as `WalkDir::max_depth` — `0` disables
/// expansion entirely (the walker yields only the root). The caller is
/// responsible for capping `globs.len()` if untrusted input is involved;
/// here we simply build one `GlobSet` and walk once per root.
///
/// On glob compile error the malformed pattern is logged and skipped.
/// On filesystem walk error the offending entry is skipped (typical
/// causes: permission denied, broken symlink); enumeration continues.
pub fn expand(roots: &[PathBuf], globs: &[String], max_depth: usize) -> Vec<PathBuf> {
    if globs.is_empty() || max_depth == 0 {
        return Vec::new();
    }

    let mut builder = GlobSetBuilder::new();
    for pattern in globs {
        match Glob::new(pattern) {
            Ok(g) => {
                builder.add(g);
            }
            Err(e) => tracing::warn!(
                pattern = %pattern,
                error = %e,
                "invalid deny-read glob pattern; skipping"
            ),
        }
    }
    let set = match builder.build() {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "deny-read globset build failed; skipping expansion");
            return Vec::new();
        }
    };

    let mut matched: Vec<PathBuf> = Vec::new();
    for root in roots {
        for entry in walkdir::WalkDir::new(root)
            .max_depth(max_depth)
            .follow_links(false)
            .into_iter()
            .filter_map(Result::ok)
        {
            let path = entry.path();
            // Match against both the absolute path and the path relative
            // to the root — some patterns are rooted (`/secrets/*`) while
            // others target leaf names (`*.env`).
            let abs = path;
            let rel = path.strip_prefix(root).unwrap_or(path);
            if set.is_match(abs) || set.is_match(rel) {
                matched.push(abs.to_path_buf());
            }
        }
    }

    matched.sort();
    matched.dedup();
    matched
}

/// Build the full deny-read path set from the parsed settings, expanding
/// any glob patterns under the writable roots.
pub fn merge_paths_and_globs(
    explicit_paths: Vec<PathBuf>,
    globs: &[String],
    roots: &[PathBuf],
    max_depth: usize,
) -> Vec<PathBuf> {
    let mut out = explicit_paths;
    out.extend(expand(roots, globs, max_depth));
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
#[path = "glob_expansion.test.rs"]
mod tests;
