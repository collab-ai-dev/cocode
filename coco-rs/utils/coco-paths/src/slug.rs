//! `ProjectSlug` — NFC-normalised, TS-equivalent project slug newtype.
//!
//! The whole point of this newtype is to make the type system enforce
//! "the slug has been computed via the canonical
//! `normalize_nfc → sanitize_path` pipeline". Callers that take
//! `&ProjectSlug` no longer have to re-validate or wonder if some
//! upstream forgot to NFC-normalise. The historical bug
//! (`memory/src/path/resolve.rs::sanitize_project_path`) is exactly
//! the failure mode this prevents.

use std::path::Path;

use crate::nfc::normalize_nfc;
use crate::sanitize::sanitize_path;

/// A sanitised, NFC-normalised project slug suitable for use as a
/// single filesystem directory name.
///
/// Cheap to construct (one NFC pass + one linear scan) and `Clone`,
/// so callers can pass it around freely.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProjectSlug(String);

impl ProjectSlug {
    /// Build a slug from a project path — typically the canonical
    /// git root (so linked worktrees share one slug), falling back
    /// to the cwd itself when not inside a git repo.
    ///
    /// `to_string_lossy` substitutes `\u{FFFD}` for non-UTF-8 bytes;
    /// since `sanitize_path` collapses every non-alphanumeric byte
    /// to `-` anyway, lossy decoding is observationally equivalent
    /// to TS `path.normalize` on the same input.
    pub fn for_path(project_path: &Path) -> Self {
        let raw = project_path.to_string_lossy();
        let nfc = normalize_nfc(&raw);
        Self(sanitize_path(&nfc))
    }

    /// Build a slug from an already-NFC-normalised string. Useful
    /// in tests and round-trip code paths.
    pub fn from_normalized(s: &str) -> Self {
        Self(sanitize_path(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for ProjectSlug {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ProjectSlug {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
#[path = "slug.test.rs"]
mod tests;
