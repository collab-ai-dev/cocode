//! Pattern matcher for gitignore-style glob patterns.
//!
//! Provides efficient path matching using compiled glob patterns.

use crate::patterns::BINARY_FILE_PATTERNS;
use crate::patterns::COMMON_DIRECTORY_EXCLUDES;
use crate::patterns::COMMON_IGNORE_PATTERNS;
use crate::patterns::SYSTEM_FILE_EXCLUDES;
use globset::Glob;
use globset::GlobSet;
use globset::GlobSetBuilder;

/// A compiled pattern matcher for efficient path filtering.
///
/// Pre-compiles glob patterns into a single matcher for optimal performance
/// when checking many paths against the same set of patterns.
#[derive(Debug)]
pub struct PatternMatcher {
    glob_set: GlobSet,
}

impl PatternMatcher {
    /// Create a new pattern matcher from a slice of gitignore-style patterns.
    ///
    /// Patterns support:
    /// - `*` - matches any sequence of characters except `/`
    /// - `**` - matches any sequence including `/`
    /// - `?` - matches any single character except `/`
    /// - `[abc]` - matches any character in the set
    /// - `{a,b}` - matches either `a` or `b`
    ///
    /// # Errors
    ///
    /// Returns error if any pattern is invalid.
    pub fn new(patterns: &[&str]) -> Result<Self, globset::Error> {
        let mut builder = GlobSetBuilder::new();
        for pattern in patterns {
            builder.add(Glob::new(pattern)?);
        }
        Ok(Self {
            glob_set: builder.build()?,
        })
    }

    /// Check if the path matches any of the patterns.
    pub fn is_match(&self, path: &str) -> bool {
        self.glob_set.is_match(path)
    }

    /// Create a matcher from all default exclude patterns.
    ///
    /// Combines COMMON_IGNORE_PATTERNS, BINARY_FILE_PATTERNS,
    /// COMMON_DIRECTORY_EXCLUDES, and SYSTEM_FILE_EXCLUDES.
    pub fn default_excludes() -> Result<Self, globset::Error> {
        let mut patterns = Vec::with_capacity(
            COMMON_IGNORE_PATTERNS.len()
                + BINARY_FILE_PATTERNS.len()
                + COMMON_DIRECTORY_EXCLUDES.len()
                + SYSTEM_FILE_EXCLUDES.len(),
        );
        patterns.extend(COMMON_IGNORE_PATTERNS);
        patterns.extend(BINARY_FILE_PATTERNS);
        patterns.extend(COMMON_DIRECTORY_EXCLUDES);
        patterns.extend(SYSTEM_FILE_EXCLUDES);
        Self::new(&patterns)
    }
}

impl Default for PatternMatcher {
    fn default() -> Self {
        Self {
            glob_set: GlobSet::empty(),
        }
    }
}

#[cfg(test)]
#[path = "matcher.test.rs"]
mod tests;
