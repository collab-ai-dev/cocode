//! Contracts between the tool layer and the shell backend.
//!
//! `BashTool` needs to assemble a command and (optionally) compress its
//! output; both are behaviours the shell layer supplies. Putting the two
//! traits and their plain-data payloads here lets `coco-tool-runtime`
//! carry them on `ToolUseContext` without linking `coco-shell` — which
//! would otherwise drag `tree-sitter-bash`, the PTY layer, and the whole
//! sandbox stack onto every crate that merely defines or executes a tool.
//!
//! The alternative — declaring the traits in `coco-tool-runtime` next to
//! the other execution seams — would force `exec/shell` to depend on
//! `core/tool-runtime` and invert the layering. Contracts belong to the
//! foundation; the implementations stay in `coco-shell`.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

/// Shell flavour. Drives spawn-arg shape and the login-shell decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellType {
    Zsh,
    Bash,
    PowerShell,
    Sh,
    Cmd,
}

impl ShellType {
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Zsh => "zsh",
            Self::Bash => "bash",
            Self::PowerShell => "powershell",
            Self::Sh => "sh",
            Self::Cmd => "cmd",
        }
    }
}

/// Per-command inputs to [`ShellProvider::build_exec_command`].
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
#[async_trait::async_trait]
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

/// Result of a rewrite attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RewriteOutcome {
    /// The backend returned a non-empty, fully-accounted-for rewrite.
    /// Execute this string instead of the original.
    Rewritten(String),
    /// Execute the original command unchanged. The reason is recorded for
    /// metrics / tracing.
    Passthrough(PassthroughReason),
}

/// Why a command was not rewritten. Every variant terminates in "run the
/// original command" — none is an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassthroughReason {
    /// No backend binary detected on `$PATH` (or the configured path).
    BinaryMissing,
    /// Detected binary is older than the minimum supported rewrite contract.
    VersionTooOld,
    /// `run_in_background=true`: the backend buffers-then-prints, which would
    /// stall incremental `TaskOutput` streaming.
    Background,
    /// The sandbox will wrap this command, blocking the backend's history write.
    Sandboxed,
    /// First command word is in the coco-side exclude list.
    Excluded,
    /// No backend equivalent for this command.
    NoEquivalent,
    /// A *host* deny rule matched (informational only; coco ran its own
    /// permission engine before this).
    HostDeny,
    /// The rewrite probe exceeded its timeout and was killed.
    Timeout,
    /// The backend process could not be spawned.
    SpawnError,
    /// A rewritten segment could not be fully accounted for.
    ShapeMismatch,
}

impl PassthroughReason {
    /// Stable metric tag value (`coco.rtk.engine_total{reason=...}`).
    pub fn as_metric_str(self) -> &'static str {
        match self {
            Self::BinaryMissing => "binary_missing",
            Self::VersionTooOld => "version_too_old",
            Self::Background => "background",
            Self::Sandboxed => "sandboxed",
            Self::Excluded => "excluded",
            Self::NoEquivalent => "no_equivalent",
            Self::HostDeny => "host_deny",
            Self::Timeout => "timeout",
            Self::SpawnError => "spawn_error",
            Self::ShapeMismatch => "shape_mismatch",
        }
    }
}

/// Execution-site facts the skip conditions need. Computed by the Bash
/// tool from the *original* command before the rewrite runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RewriteSite {
    /// `BashInput.run_in_background`.
    pub background: bool,
    /// The sandbox snapshot decided it will wrap this command.
    pub sandboxed: bool,
}

/// The Bash output-compression seam. A backend acts at one or both of two
/// lifecycle points: a **pre-spawn rewrite** (rewrite the command so its
/// own output is already compact) and a **post-exec filter** (compress the
/// captured stdout of the unmodified command in-process). The trait exists
/// so `BashTool` depends on the seam and its two capability predicates
/// rather than on a concrete backend. The whole API is **infallible**: a
/// rewrite maps to [`RewriteOutcome::Passthrough`] and a filter to `None`,
/// so a broken backend only declines to compress.
#[async_trait::async_trait]
pub trait BashOutputRewriter: std::fmt::Debug + Send + Sync {
    /// Rewrite `command` for output compression, or decide to pass it through.
    async fn rewrite(&self, command: &str, site: RewriteSite) -> RewriteOutcome;

    /// Whether this backend performs a pre-spawn rewrite. When `false`,
    /// `BashTool` skips [`rewrite`](BashOutputRewriter::rewrite) and spawns the
    /// original command. **Required, no default:** a silent `true` would opt a
    /// post-exec-only backend into modifying the spawned command it never meant
    /// to touch — each backend must declare its tiers explicitly.
    fn does_pre_spawn_rewrite(&self) -> bool;

    /// Whether this backend performs post-exec filtering. When `true`,
    /// `BashTool` calls [`filter_output`](BashOutputRewriter::filter_output) on
    /// the captured stdout — but never when a pre-spawn rewrite already fired
    /// for the same call (no double filtering).
    fn does_post_exec_filter(&self) -> bool;

    /// Post-exec output compression. Given the original command, its exit code,
    /// and captured stdout, return compressed text or `None` to keep the raw
    /// output. Infallible — a filter panic degrades to `None`. Defaults to `None`
    /// for pre-spawn-only backends; it is only ever called when
    /// [`does_post_exec_filter`](BashOutputRewriter::does_post_exec_filter) is `true`.
    async fn filter_output(
        &self,
        _command: &str,
        _exit_code: i32,
        _stdout: &str,
    ) -> Option<String> {
        None
    }
}
