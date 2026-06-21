//! Shell binary discovery — PowerShell, Git Bash, Windows path conversion.
//!
//! Used by `coco-hooks` (Command handler with `shell: "powershell" | "bash"`)
//! and any future consumer that needs to spawn a shell on Windows. The
//! crate sits at L0 (no internal deps) so it stays reusable across
//! `core/tools/BashTool` and `exec/shell`.
//!
//! # Platform strategy
//!
//! - PowerShell discovery is **cross-platform** — `pwsh` v6+ runs on
//!   Linux/macOS too, so a hook with `shell: "powershell"` works
//!   everywhere `pwsh` is on PATH. `powershell.exe` (Windows-only)
//!   is the fallback.
//! - Git Bash discovery is **Windows-only** — every other platform's
//!   default shell is bash-compatible, so the function returns `None`.
//! - Windows-path → POSIX conversion is **Windows-only** — bash on
//!   Windows (Git Bash) expects `/c/Users/foo`, not `C:\Users\foo`.

mod git_bash;
mod powershell;
mod windows_paths;

pub use git_bash::find_git_bash_path;
pub use powershell::build_powershell_args;
pub use powershell::cached_powershell_path;
pub use windows_paths::windows_path_to_posix_path;

#[cfg(test)]
#[path = "lib.test.rs"]
mod tests;
