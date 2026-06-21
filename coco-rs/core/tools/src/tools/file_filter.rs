//! Shared file-filtering primitives for the Glob and Grep tools.
//!
//! Both tools reproduce ripgrep's `--glob` semantics using the same
//! `ignore::overrides::Override` matcher ripgrep itself uses, applied in **two
//! distinct roles** so that `.agentignore` stays authoritative:
//!
//! * **Positive glob filter** ([`compile_glob_matcher`]) — the user's
//!   pattern(s). Applied *per file* via
//!   `Override::matched(rel, false).is_whitelist()`, NOT as the walker's
//!   override. A whitelist override outranks every ignore file (verified: real
//!   `rg -g '*.rs'` re-includes `.gitignore`d files), which would let the model
//!   read agent-hidden files with `Glob "**/*"`. Filtering per file keeps the
//!   walk's ignore matchers — including `.agentignore` — authoritative.
//!
//! * **Exclusion override** ([`build_exclusion_override`]) — VCS dirs and the
//!   file-read ignore patterns, as `!` negatives. Because it contains no
//!   whitelist it prunes matching paths (directories included, so big trees
//!   like `.git/` are skipped) without overriding any ignore file.
//!
//! A slash-less pattern matches the basename at any depth (`Cargo.toml` finds
//! every `Cargo.toml`), matching `rg --files --glob Cargo.toml`; path patterns
//! match relative to the override root.

use ignore::overrides::Override;
use ignore::overrides::OverrideBuilder;
use std::path::Path;

/// Compile the user's positive glob pattern(s) into an [`Override`] for use as
/// a per-file matcher. Callers must pass at least one pattern; an empty slice
/// yields an empty matcher that whitelists nothing.
pub(crate) fn compile_glob_matcher<S: AsRef<str>>(
    root: &Path,
    patterns: &[S],
) -> Result<Override, ignore::Error> {
    let mut builder = OverrideBuilder::new(root);
    for pattern in patterns {
        builder.add(pattern.as_ref())?;
    }
    builder.build()
}

/// Build a negatives-only [`Override`] for the walker: VCS excludes (already
/// `!`-prefixed) plus the file-read ignore patterns. Contains no whitelist, so
/// it prunes without overriding `.gitignore` / `.ignore` / `.agentignore`.
/// Returns an empty (no-op) override when both lists are empty.
pub(crate) fn build_exclusion_override(
    root: &Path,
    vcs_excludes: &[&str],
    read_ignore_patterns: &[String],
) -> Result<Override, ignore::Error> {
    let mut builder = OverrideBuilder::new(root);
    for excl in vcs_excludes {
        builder.add(excl)?;
    }
    for negative in read_ignore_negative_globs(read_ignore_patterns) {
        builder.add(&negative)?;
    }
    builder.build()
}

/// Convert file-read ignore patterns into ripgrep `--glob '!...'` negatives.
///
/// Mirrors the TS GrepTool rule: a `/`-anchored pattern is kept as-is (anchored
/// to the search root), while a relative pattern is prefixed with `**/` so it
/// matches at any depth — ripgrep only applies globs relative to the search
/// root otherwise. See
/// <https://github.com/BurntSushi/ripgrep/discussions/2156>.
fn read_ignore_negative_globs(patterns: &[String]) -> Vec<String> {
    patterns
        .iter()
        .map(|p| {
            if p.starts_with('/') {
                format!("!{p}")
            } else {
                format!("!**/{p}")
            }
        })
        .collect()
}

#[cfg(test)]
#[path = "file_filter.test.rs"]
mod tests;
