//! Windows path → POSIX path conversion for bash interop.
//!
//! Git Bash on Windows expects POSIX-shaped paths: drive `C:` becomes
//! `/c`, backslashes become forward slashes. PowerShell consumes
//! native Windows paths and skips this conversion. The function is
//! cross-platform (returns the input unchanged on non-Windows) so
//! callers can pass both bash hooks and arbitrary path strings without
//! a platform fork at the callsite.

/// Convert a Windows-style path (`C:\Users\foo`) to its Git Bash form
/// (`/c/Users/foo`). On non-Windows the input is returned unchanged.
///
/// Handles:
/// - Drive letter prefix `C:` / `c:` → `/c`
/// - Backslash separators → forward slash
/// - Already-POSIX inputs (`/c/foo`) pass through unchanged
///
/// Returns an owned `String` because the conversion is non-trivial in
/// the typical case; a borrowed-only API would force the caller to
/// allocate anyway.
#[must_use]
pub fn windows_path_to_posix_path(input: &str) -> String {
    #[cfg(not(target_os = "windows"))]
    {
        input.to_string()
    }

    #[cfg(target_os = "windows")]
    {
        // Already POSIX-shaped — passthrough.
        if input.starts_with('/') {
            return input.to_string();
        }

        let bytes = input.as_bytes();
        // Drive-letter prefix: `<letter>:` followed by `\` or `/` or end.
        let drive_prefix = bytes.len() >= 2
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && (bytes.len() == 2 || bytes[2] == b'\\' || bytes[2] == b'/');

        let body = if drive_prefix {
            // Replace `C:` with `/c` (lowercase drive letter — matches MSYS2).
            let drive = (bytes[0] as char).to_ascii_lowercase();
            let rest = &input[2..];
            format!("/{drive}{rest}")
        } else {
            input.to_string()
        };

        // Backslash → forward slash.
        body.replace('\\', "/")
    }
}
