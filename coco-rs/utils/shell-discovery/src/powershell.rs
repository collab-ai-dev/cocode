//! PowerShell discovery and argv construction.

use std::path::PathBuf;
use tokio::sync::OnceCell;

/// Cached lookup for `pwsh` (v6+, cross-platform) then `powershell` (Windows-only).
///
/// Returns `None` if neither is on `PATH`. Cached for the process
/// lifetime — subsequent calls hit the OnceCell directly. TS uses an
/// in-process Promise cache; OnceCell + async init is the Rust idiom.
pub async fn cached_powershell_path() -> Option<PathBuf> {
    static CACHE: OnceCell<Option<PathBuf>> = OnceCell::const_new();
    CACHE
        .get_or_init(|| async {
            // pwsh (PowerShell v6+) is cross-platform — preferred when present.
            if let Ok(p) = which::which("pwsh") {
                return Some(p);
            }
            // powershell.exe is Windows-only (Windows PowerShell 5.1).
            #[cfg(target_os = "windows")]
            if let Ok(p) = which::which("powershell") {
                return Some(p);
            }
            None
        })
        .await
        .clone()
}

/// Build the argv tail for invoking PowerShell.
///
/// Produces `-NoProfile -NonInteractive -Command <cmd>` so user profile scripts
/// don't run, the shell never blocks for input, and the command body
/// is interpreted (not treated as a script path).
#[must_use]
pub fn build_powershell_args(cmd: &str) -> Vec<String> {
    vec![
        "-NoProfile".to_string(),
        "-NonInteractive".to_string(),
        "-Command".to_string(),
        cmd.to_string(),
    ]
}
