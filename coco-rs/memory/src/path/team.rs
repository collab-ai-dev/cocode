//! Team-memory path validation combinators.
//!
//! Wires path-string validation, lexical containment, and
//! symlink-aware real-path checks against the project's team-memory
//! directory. The team directory is a shared subtree
//! (`<personal>/team/`); writes pass through these combinators so a
//! malicious server key (relative) or model-supplied path (absolute)
//! can't escape the directory via traversal segments or planted
//! symlinks.

use std::path::Path;
use std::path::PathBuf;

use super::symlink::realpath_deepest_existing;
use super::validate::PathValidationError;
use super::validate::lexical_normalize;
use super::validate::validate_memory_path;
use super::validate::validate_resolved_path;

/// Validate a relative key against `team_dir`. Two-pass check:
///
/// 1. **Sanitize + lexical:** the key passes [`validate_memory_path`]
///    (null bytes, traversal, UNC, drive root, tilde, fullwidth /
///    URL-encoded `..`), then `team_dir.join(key)` is normalized and
///    confirmed to stay under `team_dir`.
/// 2. **Symlink-aware:** `realpath` is resolved on the deepest
///    existing ancestor and compared against the canonical `team_dir`.
///    A planted symlink in `team_dir` pointing outside (e.g. to
///    `~/.ssh/authorized_keys`) is rejected here even when the lexical
///    check passed.
///
/// Returns the resolved absolute path on success.
pub fn validate_team_mem_key(
    relative_key: &str,
    team_dir: &Path,
) -> Result<PathBuf, PathValidationError> {
    validate_memory_path(relative_key)?;
    let resolved = validate_resolved_path(Path::new(relative_key), team_dir)?;
    verify_real_containment(&resolved, team_dir)?;
    Ok(resolved)
}

/// Validate an absolute file path against `team_dir`. Same two
/// passes as [`validate_team_mem_key`] but the input is an absolute
/// path (model-supplied via the Edit/Write tool). Only null bytes are
/// rejected at the string level — absolute paths are otherwise OK
/// since the model has full FS access; containment vs `team_dir` is
/// what the caller actually needs.
pub fn validate_team_mem_write_path(
    absolute_path: &Path,
    team_dir: &Path,
) -> Result<PathBuf, PathValidationError> {
    if absolute_path.as_os_str().to_string_lossy().contains('\0') {
        return Err(PathValidationError::NullByte);
    }
    let normalized = lexical_normalize(absolute_path);
    let team_norm = lexical_normalize(team_dir);
    if !normalized.starts_with(&team_norm) {
        return Err(PathValidationError::Escape);
    }
    verify_real_containment(&normalized, team_dir)?;
    Ok(normalized)
}

/// Symlink-aware containment check.
///
/// Resolves the deepest existing ancestor of *both* `candidate` and `team_dir`
/// so a planted symlink in `team_dir`'s parent chain — which
/// `team_dir.canonicalize()` alone would not see because `team_dir`
/// doesn't yet exist — still trips the containment check.
///
/// Fails closed on any unexpected I/O error (EACCES, ELOOP, EIO):
/// a security boundary that silently passes on errors is worse than
/// one that occasionally rejects a recoverable case.
fn verify_real_containment(candidate: &Path, team_dir: &Path) -> Result<(), PathValidationError> {
    let Some(real_candidate) = realpath_deepest_existing(candidate) else {
        // No canonicalizable ancestor at all — only happens on a
        // degenerate FS. Caller's lexical check already passed; nothing
        // more we can verify, accept.
        return Ok(());
    };
    let Some(real_team) = realpath_deepest_existing(team_dir) else {
        // Same — degenerate FS for team_dir; nothing to verify against.
        return Ok(());
    };
    if real_candidate == real_team || real_candidate.starts_with(&real_team) {
        Ok(())
    } else {
        Err(PathValidationError::Escape)
    }
}

#[cfg(test)]
#[path = "team.test.rs"]
mod tests;
