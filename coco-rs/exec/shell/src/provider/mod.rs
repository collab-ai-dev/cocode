//! Provider abstraction for shell-specific command building.
//!
//! The provider is the **only** thing that knows about per-shell quirks:
//! snapshot sourcing, session-env injection, extglob disabling, alias
//! expansion, `pwd -P` tracking, base64-encoded PowerShell commands,
//! sandbox `TMPDIR` overrides, `COCO_SHELL_PREFIX` wrapping. The executor
//! (`crate::executor::ShellExecutor`) is just a spawn / wait / cancel /
//! timeout / sandbox-wrap loop on top.
//!
//! Two implementations ship:
//! - [`bash::BashProvider`] for bash / zsh / sh (full pipeline)
//! - [`powershell::PowerShellProvider`] for pwsh / powershell (UTF-16-LE
//!   base64-encoded command path)

pub mod bash;
pub mod powershell;

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use async_trait::async_trait;

use crate::shell_types::ShellType;

pub use bash::BashProvider;
pub use powershell::PowerShellProvider;

/// Per-command options threaded into [`ShellProvider::build_exec_command`]
/// and [`ShellProvider::env_overrides`].
#[derive(Debug, Clone, Default)]
pub struct BuildExecOpts {
    /// Unique-per-command id, used to name the CWD-tracking temp file.
    /// The executor maintains an atomic counter.
    pub id: u64,
    /// Per-command sandbox tmpdir, if the command will run sandboxed.
    /// Set by the executor based on `ExecOptions.sandbox`.
    pub sandbox_tmp_dir: Option<PathBuf>,
    /// True when this command will be wrapped with platform sandbox
    /// enforcement. Providers use this to decide:
    /// - bash: where the cwd-tracking file is created (must be writable
    ///   inside the sandbox).
    /// - powershell: which command form (base64 vs raw) to emit.
    pub use_sandbox: bool,
}

/// Output of [`ShellProvider::build_exec_command`].
#[derive(Debug, Clone)]
pub struct BuiltCommand {
    /// Fully-assembled shell command string. Pass directly to the shell
    /// binary via its `-c` / `-Command` argument.
    pub command_string: String,
    /// Filesystem path the inner command writes the post-execution CWD
    /// to via `pwd -P` (bash) or `Out-File` (pwsh). The executor reads
    /// this file after the child exits, then unlinks it.
    pub cwd_file_path: PathBuf,
}

/// Shell-specific command assembly + spawn-args + env overrides.
///
/// Implementations are usually `Arc`-shared across all tool calls in a
/// session — they hold the snapshot watch receiver, session-env reader,
/// and `/env` store, all of which are session-scoped state.
///
/// `Debug` is required by the parent `QueryEngineConfig` derive.
#[async_trait]
pub trait ShellProvider: Send + Sync + std::fmt::Debug {
    /// Shell flavor (drives spawn-arg shape, login-shell decision, …).
    fn shell_type(&self) -> &ShellType;

    /// Absolute path to the shell binary (`/bin/bash`, `/usr/bin/pwsh`, …).
    fn shell_path(&self) -> &Path;

    /// Build the full command string + CWD-tracking file path.
    async fn build_exec_command(&self, command: &str, opts: &BuildExecOpts) -> BuiltCommand;

    /// Argv to pass after the shell binary (`["-c", cmd]` or
    /// `["-c", "-l", cmd]` depending on snapshot availability for bash;
    /// `["-NoProfile", "-NonInteractive", "-Command", cmd]` for pwsh).
    fn spawn_args(&self, command_string: &str) -> Vec<String>;

    /// Per-command env-var overrides (session-env vars from `/env`,
    /// sandbox `TMPDIR` / `TMPPREFIX`, future tmux socket override, …).
    /// Applied on top of the inherited process env.
    async fn env_overrides(&self, command: &str, opts: &BuildExecOpts) -> HashMap<String, String>;
}
