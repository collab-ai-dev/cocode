//! Cross-platform path translation between Windows and POSIX forms.
//!
//! LRU caching elided — these conversions are cheap pure string ops and
//! the hot path runs once per shell command.
//!
//! Used by the bash provider when shelling out under Git Bash on Windows:
//! the inner bash command needs POSIX paths (`/c/Users/foo`) but the
//! outer Rust process needs native Windows paths (`C:\Users\foo`) for
//! `std::fs` operations.

/// Convert a Windows path to its POSIX equivalent for Git Bash / MSYS2 /
/// Cygwin.
///
/// - UNC paths: `\\server\share` → `//server/share`
/// - Drive letters: `C:\Users\foo` → `/c/Users/foo`
/// - Already POSIX / relative: just flip backslashes to forward slashes.
pub fn windows_path_to_posix_path(windows_path: &str) -> String {
    if windows_path.starts_with(r"\\") {
        return windows_path.replace('\\', "/");
    }
    // Drive letter: `<letter>:[\\/]…`
    let bytes = windows_path.as_bytes();
    if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'/' || bytes[2] == b'\\')
    {
        let drive = (bytes[0] as char).to_ascii_lowercase();
        let mut out = String::with_capacity(windows_path.len() + 1);
        out.push('/');
        out.push(drive);
        out.push_str(&windows_path[2..].replace('\\', "/"));
        return out;
    }
    windows_path.replace('\\', "/")
}

/// Convert a POSIX-form path back to native Windows form.
///
/// - UNC: `//server/share` → `\\server\share`
/// - Cygwin: `/cygdrive/c/...` → `C:\...`
/// - MSYS2 / Git Bash: `/c/...` → `C:\...`
/// - Already Windows / relative: just flip slashes.
pub fn posix_path_to_windows_path(posix_path: &str) -> String {
    if posix_path.starts_with("//") {
        return posix_path.replace('/', "\\");
    }
    // /cygdrive/X[/...] form.
    let cygdrive = "/cygdrive/";
    if let Some(rest) = posix_path.strip_prefix(cygdrive)
        && let Some(first) = rest.chars().next()
        && first.is_ascii_alphabetic()
    {
        let after_letter = &rest[1..];
        let drive = first.to_ascii_uppercase();
        let body = if after_letter.is_empty() {
            "\\".to_string()
        } else {
            after_letter.replace('/', "\\")
        };
        return format!("{drive}:{body}");
    }
    // /X[/...] form.
    let bytes = posix_path.as_bytes();
    if bytes.len() >= 2
        && bytes[0] == b'/'
        && bytes[1].is_ascii_alphabetic()
        && (bytes.len() == 2 || bytes[2] == b'/')
    {
        let drive = (bytes[1] as char).to_ascii_uppercase();
        let body_start = 2;
        let body = if body_start >= posix_path.len() {
            "\\".to_string()
        } else {
            posix_path[body_start..].replace('/', "\\")
        };
        return format!("{drive}:{body}");
    }
    posix_path.replace('/', "\\")
}

#[cfg(test)]
#[path = "windows_paths.test.rs"]
mod tests;
