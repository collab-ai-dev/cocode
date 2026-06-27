//! Strip file extension from a path.
//!
//! This module provides utilities for removing file extensions from paths.

use std::path::Path;

/// Strip the file extension from a path.
///
/// Returns the path without its extension. If the path has no extension,
/// returns the original path.
///
/// # Examples
///
/// ```
/// use vercel_ai_provider_utils::strip_extension;
///
/// assert_eq!(strip_extension("file.txt"), "file");
/// assert_eq!(strip_extension("file.tar.gz"), "file.tar");
/// assert_eq!(strip_extension("file"), "file");
/// assert_eq!(strip_extension("/path/to/file.txt"), "/path/to/file");
/// ```
pub fn strip_extension(path: &str) -> &str {
    let p = Path::new(path);
    match p.file_stem() {
        Some(stem) => {
            // file_stem returns the file name without the extension
            // We need to preserve the directory part
            match p.parent() {
                Some(parent) if !parent.as_os_str().is_empty() => {
                    // Reconstruct with parent directory
                    // This is a bit tricky since we're returning &str
                    // For simplicity, we'll just return the stem if there's a parent
                    // The caller should handle the full path reconstruction if needed
                    path.strip_suffix(
                        p.extension()
                            .map(|e| e.to_str().unwrap_or(""))
                            .unwrap_or(""),
                    )
                    .map(|s| s.trim_end_matches('.'))
                    .unwrap_or(path)
                }
                _ => stem.to_str().unwrap_or(path),
            }
        }
        None => path,
    }
}

/// Strip a specific extension from a path.
///
/// Only strips the extension if it matches the given extension.
///
/// # Examples
///
/// ```
/// use vercel_ai_provider_utils::strip_specific_extension;
///
/// assert_eq!(strip_specific_extension("file.txt", "txt"), Some("file"));
/// assert_eq!(strip_specific_extension("file.txt", "md"), None);
/// assert_eq!(strip_specific_extension("file", "txt"), None);
/// ```
pub fn strip_specific_extension<'a>(path: &'a str, ext: &str) -> Option<&'a str> {
    let expected_ext = format!(".{ext}");
    if path.to_lowercase().ends_with(&expected_ext.to_lowercase()) {
        Some(&path[..path.len() - expected_ext.len()])
    } else {
        None
    }
}

#[cfg(test)]
#[path = "strip_extension.test.rs"]
mod tests;
