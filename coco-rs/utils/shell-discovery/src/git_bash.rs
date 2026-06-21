//! Git Bash binary discovery on Windows.
//!
//! Walks PATH and standard install locations. Non-Windows platforms
//! always return `None` — `/bin/sh` works directly without discovery.

#[cfg(target_os = "windows")]
use std::path::Path;
use std::path::PathBuf;

/// Find a Git Bash binary on Windows. Searches PATH first, then the
/// standard install locations (`Program Files`, `Program Files (x86)`,
/// `%LOCALAPPDATA%\Programs\Git`).
///
/// On non-Windows platforms always returns `None` — the caller should
/// fall back to `/bin/sh` directly.
#[cfg(target_os = "windows")]
#[must_use]
pub fn find_git_bash_path() -> Option<PathBuf> {
    // 1. PATH lookup — covers users who put Git's bin on PATH.
    if let Ok(p) = which::which("bash") {
        return Some(p);
    }

    // 2. Standard Git for Windows install locations.
    let candidates: [&str; 2] = [
        r"C:\Program Files\Git\bin\bash.exe",
        r"C:\Program Files (x86)\Git\bin\bash.exe",
    ];
    for candidate in candidates {
        if Path::new(candidate).exists() {
            return Some(PathBuf::from(candidate));
        }
    }

    // 3. Per-user install (LOCALAPPDATA).
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        let p = Path::new(&local).join(r"Programs\Git\bin\bash.exe");
        if p.exists() {
            return Some(p);
        }
    }

    None
}

/// Stub for non-Windows platforms — always returns `None`. Callers
/// should fall back to spawning `sh` directly.
#[cfg(not(target_os = "windows"))]
#[must_use]
pub fn find_git_bash_path() -> Option<PathBuf> {
    None
}
