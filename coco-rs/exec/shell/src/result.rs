use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Result of a shell command execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandResult {
    pub exit_code: i32,
    pub stdout: String,
    /// Raw pre-UTF-8-lossy stdout bytes, populated by the executor for
    /// binary-aware consumers like BashTool's image-detection path.
    /// `None` when the executor short-circuits before reading stdout
    /// (e.g., interrupted before any output).
    ///
    /// Why both `stdout` and `stdout_bytes`?
    /// - `stdout` is the canonical string consumed by the model and
    ///   the truncation pipeline; lossy conversion is fine for text.
    /// - Binary stdout (e.g., `cat image.png`) gets mangled by the
    ///   UTF-8 conversion (each invalid byte becomes `\u{FFFD}`),
    ///   destroying the magic-byte signature. `stdout_bytes` keeps the
    ///   pre-lossy bytes so detectors like `is_likely_image_bytes` can
    ///   inspect the actual data.
    ///
    /// `serde(skip)` because the bytes are redundant for text content
    /// and not part of the wire protocol.
    #[serde(skip)]
    pub stdout_bytes: Option<Vec<u8>>,
    pub stderr: String,
    /// Raw pre-UTF-8-lossy stderr bytes, populated by the executor.
    /// Symmetric with `stdout_bytes` — PowerShell on Windows can emit
    /// UTF-16 LE/BE with a BOM on stderr too, so consumers that need
    /// to decode the original encoding (e.g. PowerShellTool) read
    /// these bytes instead of the lossy `stderr` String.
    #[serde(skip)]
    pub stderr_bytes: Option<Vec<u8>>,
    /// New CWD if command changed directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_cwd: Option<PathBuf>,
    /// Whether the command timed out.
    #[serde(default)]
    pub timed_out: bool,
    /// Whether the command was interrupted by an external cancel signal.
    /// Distinct from `timed_out`: `interrupted` means the user pressed
    /// Ctrl+C / sent cancel token / aborted the session, rather than
    /// the timeout watchdog firing.
    #[serde(default)]
    pub interrupted: bool,
}

/// Options for shell command execution.
#[derive(Debug, Clone)]
pub struct ExecOptions {
    /// Timeout in milliseconds.
    pub timeout_ms: Option<i64>,
    /// Prevent CWD changes.
    pub prevent_cwd_changes: bool,
    /// Optional sandbox runtime state. When `Some` and the command isn't
    /// excluded by the sandbox settings, the executor wraps the command
    /// with platform enforcement (bwrap/Seatbelt) before spawning. `None`
    /// runs unsandboxed — used by tests and contexts where sandbox is
    /// disabled.
    pub sandbox: Option<Arc<coco_sandbox::SandboxState>>,
    /// Sandbox bypass requested via `dangerouslyDisableSandbox` parameter.
    /// Only meaningful when `sandbox` is `Some` and
    /// `SandboxSettings.allow_unsandboxed_commands` is `true`.
    pub sandbox_bypass: coco_sandbox::SandboxBypass,
    /// Extra environment variables.
    pub extra_env: HashMap<String, String>,
    /// CWD override.
    pub cwd_override: Option<PathBuf>,
    /// External cancel token. When fired, the executor drops the child
    /// future (which kills the child via `kill_on_drop(true)`) and
    /// returns a `CommandResult` with `interrupted = true`. R6-T17.
    pub cancel: Option<CancellationToken>,
}

impl Default for ExecOptions {
    fn default() -> Self {
        Self {
            timeout_ms: Some(120_000), // 2 minutes default
            prevent_cwd_changes: false,
            sandbox: None,
            sandbox_bypass: coco_sandbox::SandboxBypass::No,
            extra_env: HashMap::new(),
            cwd_override: None,
            cancel: None,
        }
    }
}
