//! Memory path validation — traversal, null bytes, UNC, drive root,
//! tilde, fullwidth Unicode, URL-encoded traversal.

use std::path::Path;
use std::path::PathBuf;

/// Validation error for memory paths.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathValidationError {
    #[error("path is empty")]
    Empty,
    #[error("path contains null bytes")]
    NullByte,
    #[error("path contains '..' traversal")]
    Traversal,
    #[error("path must be relative")]
    AbsolutePath,
    #[error("UNC paths are not allowed")]
    UncPath,
    #[error("Windows drive-root paths are not allowed")]
    DriveRoot,
    #[error("path contains a bare or non-home tilde")]
    Tilde,
    #[error("path contains fullwidth or other Unicode-traversal characters")]
    UnicodeTraversal,
    #[error("path is too short")]
    TooShort,
    #[error("path contains backslash separators")]
    Backslash,
    #[error("path escapes the memory directory")]
    Escape,
}

impl coco_error::StackError for PathValidationError {
    fn debug_fmt(&self, layer: usize, buf: &mut Vec<String>) {
        buf.push(format!("{layer}: {self}"));
    }

    fn next(&self) -> Option<&dyn coco_error::StackError> {
        None
    }
}

impl coco_error::ErrorExt for PathValidationError {
    fn status_code(&self) -> coco_error::StatusCode {
        coco_error::StatusCode::InvalidArguments
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Validate a relative memory file path.
///
/// Rejects, in order:
///
/// - empty / `len < 3` (a one- or two-byte string is never a
///   legitimate memory file — rejects `.length < 3` after sep-strip).
/// - null bytes
/// - UNC (`\\server\share` AND POSIX-style `//server/share`)
/// - absolute paths (`/foo`)
/// - Windows drive roots (`C:\foo` / `C:foo`)
/// - bare or non-home tilde, including `~/..`, `~/.` after lexical
///   collapse (rejects any tilde-prefixed path that resolves outside
///   `$HOME`)
/// - fullwidth traversal characters AND any other character that
///   normalizes (NFKC) into `..` / `/` / `\` / null
/// - URL-encoded traversal (`%2e%2e`, `%2f`, mixed case)
/// - traversal components after path-separator split (covers both
///   `/` and `\`)
pub fn validate_memory_path(path: &str) -> Result<(), PathValidationError> {
    if path.is_empty() {
        return Err(PathValidationError::Empty);
    }
    if path.contains('\0') {
        return Err(PathValidationError::NullByte);
    }
    // Reject both Windows UNC (`\\foo`) and POSIX-double-slash
    // (`//foo` — RFC 3986 reserves the leading double-slash for
    // network locations on POSIX-leaning toolchains).
    if path.starts_with("\\\\") || path.starts_with("//") {
        return Err(PathValidationError::UncPath);
    }
    if path.starts_with('/') {
        return Err(PathValidationError::AbsolutePath);
    }
    let bytes = path.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && (bytes[0] as char).is_ascii_alphabetic() {
        return Err(PathValidationError::DriveRoot);
    }
    // Tilde: only accepts `~/` and `~\\` followed by something
    // that doesn't lexically escape `$HOME`. Bare `~`, `~/..`,
    // `~/.`, `~/../foo` are all out.
    if path.starts_with('~') {
        if !path.starts_with("~/") && !path.starts_with("~\\") {
            return Err(PathValidationError::Tilde);
        }
        // Strip the `~/` (or `~\`) prefix; the remainder must not
        // contain traversal that would escape `$HOME`.
        let tail = &path[2..];
        if tail.is_empty()
            || tail == ".."
            || tail == "."
            || tail.split(['/', '\\']).any(|c| c == "..")
        {
            return Err(PathValidationError::Tilde);
        }
    }
    if path.len() < 3 {
        return Err(PathValidationError::TooShort);
    }
    // NFKC-normalise first so `U+FF0E` (fullwidth full stop),
    // `U+2024` (one dot leader), `U+2025` (two dot leader),
    // `U+FE52` (small full stop), etc. all collapse into `..` and
    // get caught by the same substring check. NFKC normalization
    // is used for this attack class.
    // We compare the *normalised* form to the original: any new `..`
    // / `/` / `\` / null introduced by normalisation indicates the
    // input was crafted to bypass a pre-normalisation check.
    let normalised = coco_paths::nfc::normalize_nfkc(path);
    if normalised != path
        && (normalised.contains("..")
            || normalised.contains('/')
            || normalised.contains('\\')
            || normalised.contains('\0'))
    {
        return Err(PathValidationError::UnicodeTraversal);
    }
    // Belt-and-braces: still reject the explicit fullwidth
    // codepoints even if NFKC didn't catch them (older
    // unicode-normalization versions had gaps).
    if path.contains('\u{FF0E}') || path.contains('\u{FF0F}') {
        return Err(PathValidationError::UnicodeTraversal);
    }
    let decoded = path
        .replace("%2e", ".")
        .replace("%2E", ".")
        .replace("%2f", "/")
        .replace("%2F", "/");
    for component in decoded.split(['/', '\\']) {
        if component == ".." {
            return Err(PathValidationError::Traversal);
        }
    }
    Ok(())
}

/// Lexically resolve `path` against `memory_dir` and verify the result
/// stays inside the memory dir. No filesystem access — symlink-aware
/// validation is in [`super::symlink`].
pub fn validate_resolved_path(
    path: &Path,
    memory_dir: &Path,
) -> Result<PathBuf, PathValidationError> {
    let resolved = memory_dir.join(path);
    let normalized = lexical_normalize(&resolved);
    let mem_normalized = lexical_normalize(memory_dir);
    if !normalized.starts_with(&mem_normalized) {
        return Err(PathValidationError::Escape);
    }
    Ok(normalized)
}

/// Predicate variant: is `path` already within `memory_dir`?
pub fn is_within_memory_dir(path: &Path, memory_dir: &Path) -> bool {
    lexical_normalize(path).starts_with(lexical_normalize(memory_dir))
}

/// Sanitize a free-form string for use as a memory file basename.
pub fn sanitize_path_key(key: &str) -> String {
    key.to_lowercase()
        .chars()
        .filter_map(|c| {
            if c.is_alphanumeric() || c == '_' || c == '-' || c == '.' {
                Some(c)
            } else if c == ' ' {
                Some('_')
            } else {
                None
            }
        })
        .collect()
}

/// Lexically normalize a path: collapse `..`/`.` components without
/// touching the filesystem. Equivalent to `path.normalize` on a path
/// that's already been joined.
pub(crate) fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
#[path = "validate.test.rs"]
mod tests;
