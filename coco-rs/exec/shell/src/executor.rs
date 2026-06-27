//! Shell command executor.
//!
//! Spawns a child shell with the command built by a
//! [`crate::provider::ShellProvider`], waits with timeout + cancel-token,
//! optionally wraps with platform sandbox enforcement, and reads back the
//! CWD via the provider-written file.
//!
//! This layer is intentionally thin — every shell-flavor concern (snapshot,
//! session-env, extglob, eval-quoting, pwd-tracking, encoding) lives in
//! the provider. Adding a new shell means adding a `ShellProvider` impl,
//! not editing this file.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use coco_config::ShellConfig;

use crate::provider::BashProvider;
use crate::provider::BuildExecOpts;
use crate::provider::BuiltCommand;
use crate::provider::ShellProvider;
use crate::result::CommandResult;
use crate::result::ExecOptions;
use crate::safety::SafetyResult;
use crate::shell_types::default_user_shell;
use crate::shell_types::shell_from_config;
use crate::snapshot::ShellSnapshot;
use crate::snapshot::SnapshotConfig;

/// Process-wide monotonic ID for per-command CWD-tracking filenames.
///
/// Avoids collisions when many `ShellExecutor`s share the same tmpdir
/// (e.g. concurrent agent teams). u64 is more than enough — even at 1M
/// commands/second it wraps in 500K years.
static COMMAND_ID: AtomicU64 = AtomicU64::new(1);

fn next_command_id() -> u64 {
    COMMAND_ID.fetch_add(1, Ordering::Relaxed)
}

/// Shell executor with CWD tracking, timeout, and environment snapshot support.
pub struct ShellExecutor {
    /// Current working directory.
    cwd: PathBuf,
    /// CWD at executor construction. Stays put even when commands `cd`
    /// elsewhere; threaded into `coco_sandbox::bare_repo_scrub_paths` so
    /// the post-command scrub mitigates planted bare-repo attacks
    /// (anthropics/claude-code#29316) for the original session root.
    original_cwd: PathBuf,
    /// Shell-specific command assembler. `Arc`-shared across all
    /// `ShellExecutor` instances in a session so they all see the same
    /// snapshot / session-env / shell-prefix state.
    provider: Arc<dyn ShellProvider>,
}

impl ShellExecutor {
    /// Auto-detect the shell from `$SHELL` + platform defaults. No
    /// session-scoped state (snapshot, session-env, shell-prefix).
    pub fn new(cwd: &Path) -> Self {
        let provider = Arc::new(BashProvider::from_shell(default_user_shell()));
        Self::with_provider(cwd, provider)
    }

    /// Honour `ShellConfig.default_shell` for the override but skip
    /// snapshot wiring (legacy callers + tests).
    pub fn new_with_config(cwd: &Path, shell_config: &ShellConfig) -> Self {
        let provider = Arc::new(BashProvider::from_shell(shell_from_config(shell_config)));
        Self::with_provider(cwd, provider)
    }

    /// Construct with a pre-built provider — the entry point for
    /// session-bootstrap code that wires in the snapshot watch, session-env
    /// reader, and `/env` store.
    pub fn with_provider(cwd: &Path, provider: Arc<dyn ShellProvider>) -> Self {
        Self {
            cwd: cwd.to_path_buf(),
            original_cwd: cwd.to_path_buf(),
            provider,
        }
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub fn set_cwd(&mut self, cwd: PathBuf) {
        self.cwd = cwd;
    }

    /// Borrow the underlying provider (e.g. for `start_snapshotting` on
    /// the embedded [`Shell`] — only relevant for `BashProvider`).
    pub fn provider(&self) -> &Arc<dyn ShellProvider> {
        &self.provider
    }

    /// Convenience for callers that haven't yet been refactored onto the
    /// provider path: kick off snapshot capture in a background task and
    /// install the receiver onto a freshly-built provider.
    ///
    /// When `shell_config.disable_snapshot` is true (via settings.json or
    /// `COCO_DISABLE_SHELL_SNAPSHOT`), this is a no-op.
    pub fn start_snapshotting(&mut self, coco_home: PathBuf, session_id: &str) {
        // Build a fresh Shell so we own a `&mut` for the watch sender.
        let mut shell = default_user_shell();
        let config = SnapshotConfig::new(&coco_home);
        ShellSnapshot::start_snapshotting(config, session_id, &mut shell);
        self.provider = Arc::new(BashProvider::from_shell(shell));
    }

    /// Execute a shell command with timeout and CWD tracking.
    pub async fn execute(
        &mut self,
        command: &str,
        options: &ExecOptions,
    ) -> anyhow::Result<CommandResult> {
        let effective_cwd = options
            .cwd_override
            .as_deref()
            .unwrap_or(&self.cwd)
            .to_path_buf();
        let original_cwd = self.original_cwd.clone();

        // Allocate the per-command sandbox tmpdir up-front (when
        // sandbox is active for this command). Holding the TempDir
        // here means it auto-cleans when this function returns. The
        // path is passed to the provider (for cwd-file + TMPDIR) AND
        // to the platform wrap (for bwrap `--bind` / Seatbelt
        // file-write subpath).
        let sandbox_tmp_dir: Option<tempfile::TempDir> = if should_use_sandbox(options, command) {
            coco_sandbox::SandboxState::allocate_command_tmp_dir()
        } else {
            None
        };
        let sandbox_tmp_path = sandbox_tmp_dir.as_ref().map(|d| d.path().to_path_buf());

        let mut built = self
            .build_command(command, options, sandbox_tmp_path.clone())
            .await;
        // Linux netns bridge: prepend the inner socat listeners (forwarding the
        // netns-local proxy ports to the bind-mounted host UDS) so the sandboxed
        // command's egress reaches the host proxy. No-op on macOS / no bridge.
        if let Some(prefix) = sandbox_inner_bridge_prefix(options, command) {
            built.command_string = format!("{prefix}{}", built.command_string);
        }
        let merged_env = self
            .merge_env(command, options, sandbox_tmp_path.clone())
            .await;
        let spawn_args = self.provider.spawn_args(&built.command_string);

        let mut cmd = tokio::process::Command::new(self.provider.shell_path());
        cmd.args(&spawn_args);
        cmd.current_dir(&effective_cwd);
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        for (key, value) in merged_env {
            cmd.env(key, value);
        }

        apply_sandbox_wrap(&mut cmd, command, options, sandbox_tmp_path.as_deref())?;

        let child = cmd.kill_on_drop(true).spawn()?;
        let timeout_ms = options.timeout_ms.unwrap_or(120_000);
        let timeout_duration = std::time::Duration::from_millis(timeout_ms as u64);

        let wait_future = child.wait_with_output();
        tokio::pin!(wait_future);

        let output = if let Some(cancel) = &options.cancel {
            tokio::select! {
                biased;
                () = cancel.cancelled() => {
                    scrub_bare_repo_after_command(options, &effective_cwd, &original_cwd);
                    cleanup_cwd_file(&built.cwd_file_path);
                    return Ok(CommandResult {
                        exit_code: -1,
                        stdout: String::new(),
                        stdout_bytes: None,
                        stderr: "Command interrupted".to_string(),
                        stderr_bytes: None,
                        new_cwd: None,
                        timed_out: false,
                        interrupted: true,
                    });
                }
                result = tokio::time::timeout(timeout_duration, &mut wait_future) => {
                    match result {
                        Ok(Ok(output)) => output,
                        Ok(Err(e)) => {
                            scrub_bare_repo_after_command(options, &effective_cwd, &original_cwd);
                            cleanup_cwd_file(&built.cwd_file_path);
                            return Err(e.into());
                        }
                        Err(_) => {
                            scrub_bare_repo_after_command(options, &effective_cwd, &original_cwd);
                            cleanup_cwd_file(&built.cwd_file_path);
                            return Ok(CommandResult {
                                exit_code: -1,
                                stdout: String::new(),
                                stdout_bytes: None,
                                stderr: format!("Command timed out after {timeout_ms}ms"),
                                stderr_bytes: None,
                                new_cwd: None,
                                timed_out: true,
                                interrupted: false,
                            });
                        }
                    }
                }
            }
        } else {
            match tokio::time::timeout(timeout_duration, wait_future).await {
                Ok(Ok(output)) => output,
                Ok(Err(e)) => {
                    scrub_bare_repo_after_command(options, &effective_cwd, &original_cwd);
                    cleanup_cwd_file(&built.cwd_file_path);
                    return Err(e.into());
                }
                Err(_) => {
                    scrub_bare_repo_after_command(options, &effective_cwd, &original_cwd);
                    cleanup_cwd_file(&built.cwd_file_path);
                    return Ok(CommandResult {
                        exit_code: -1,
                        stdout: String::new(),
                        stdout_bytes: None,
                        stderr: format!("Command timed out after {timeout_ms}ms"),
                        stderr_bytes: None,
                        new_cwd: None,
                        timed_out: true,
                        interrupted: false,
                    });
                }
            }
        };

        record_seccomp_violation_if_killed(options, command, &output.status).await;

        let stdout_raw = output.stdout;
        let stderr_raw = output.stderr;
        let stdout = String::from_utf8_lossy(&stdout_raw).to_string();
        let stderr = String::from_utf8_lossy(&stderr_raw).to_string();

        let new_cwd = if options.prevent_cwd_changes {
            None
        } else {
            read_cwd_file(&built.cwd_file_path)
        };
        cleanup_cwd_file(&built.cwd_file_path);

        if let Some(ref new) = new_cwd {
            self.cwd = new.clone();
        }

        scrub_bare_repo_after_command(options, &effective_cwd, &original_cwd);

        Ok(CommandResult {
            exit_code: output.status.code().unwrap_or(-1),
            stdout,
            stdout_bytes: Some(stdout_raw),
            stderr,
            stderr_bytes: Some(stderr_raw),
            new_cwd,
            timed_out: false,
            interrupted: false,
        })
    }

    /// Check command safety before execution.
    ///
    /// Destructive commands are NOT hard-denied — destructive detection is an
    /// informational advisory. A destructive command surfaces its note as the
    /// approval reason (escalate-to-ask), never a block.
    pub fn check_safety(&self, command: &str) -> SafetyResult {
        if crate::read_only::is_read_only_command(command) {
            return SafetyResult::Safe;
        }

        let reason = crate::destructive::get_destructive_warning(command)
            .unwrap_or_else(|| "command may have side effects".into());
        SafetyResult::RequiresApproval { reason }
    }

    /// Execute a command with streaming progress via a callback.
    pub async fn execute_with_progress<F>(
        &mut self,
        command: &str,
        options: &ExecOptions,
        mut on_progress: F,
    ) -> anyhow::Result<CommandResult>
    where
        F: FnMut(ShellProgress) + Send + 'static,
    {
        use std::sync::atomic::AtomicI64;
        use std::sync::atomic::Ordering;
        use tokio::io::AsyncReadExt;

        let effective_cwd = options
            .cwd_override
            .as_deref()
            .unwrap_or(&self.cwd)
            .to_path_buf();
        let original_cwd = self.original_cwd.clone();

        let sandbox_tmp_dir: Option<tempfile::TempDir> = if should_use_sandbox(options, command) {
            coco_sandbox::SandboxState::allocate_command_tmp_dir()
        } else {
            None
        };
        let sandbox_tmp_path = sandbox_tmp_dir.as_ref().map(|d| d.path().to_path_buf());

        let mut built = self
            .build_command(command, options, sandbox_tmp_path.clone())
            .await;
        // Linux netns bridge: prepend the inner socat listeners (forwarding the
        // netns-local proxy ports to the bind-mounted host UDS) so the sandboxed
        // command's egress reaches the host proxy. No-op on macOS / no bridge.
        if let Some(prefix) = sandbox_inner_bridge_prefix(options, command) {
            built.command_string = format!("{prefix}{}", built.command_string);
        }
        let merged_env = self
            .merge_env(command, options, sandbox_tmp_path.clone())
            .await;
        let spawn_args = self.provider.spawn_args(&built.command_string);

        let mut cmd = tokio::process::Command::new(self.provider.shell_path());
        cmd.args(&spawn_args);
        cmd.current_dir(&effective_cwd);
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        for (key, value) in merged_env {
            cmd.env(key, value);
        }

        apply_sandbox_wrap(&mut cmd, command, options, sandbox_tmp_path.as_deref())?;

        let mut child = cmd.kill_on_drop(true).spawn()?;

        let stdout_bytes = Arc::new(AtomicI64::new(0));
        let stderr_bytes = Arc::new(AtomicI64::new(0));

        let stdout_pipe = child.stdout.take();
        let stderr_pipe = child.stderr.take();

        let timeout_ms = options.timeout_ms.unwrap_or(120_000);
        let timeout_duration = std::time::Duration::from_millis(timeout_ms as u64);
        let start = std::time::Instant::now();

        let stdout_counter = stdout_bytes.clone();
        let stdout_handle = tokio::spawn(async move {
            let mut collected: Vec<u8> = Vec::new();
            if let Some(mut pipe) = stdout_pipe {
                let mut buf = vec![0u8; 8192];
                loop {
                    match pipe.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => {
                            collected.extend_from_slice(&buf[..n]);
                            stdout_counter.fetch_add(n as i64, Ordering::Relaxed);
                        }
                        Err(_) => break,
                    }
                }
            }
            collected
        });

        let stderr_counter = stderr_bytes.clone();
        let stderr_handle = tokio::spawn(async move {
            let mut collected = String::new();
            if let Some(mut pipe) = stderr_pipe {
                let mut buf = vec![0u8; 8192];
                loop {
                    match pipe.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => {
                            let chunk = String::from_utf8_lossy(&buf[..n]);
                            collected.push_str(&chunk);
                            stderr_counter.fetch_add(n as i64, Ordering::Relaxed);
                        }
                        Err(_) => break,
                    }
                }
            }
            collected
        });

        let progress_interval = std::time::Duration::from_secs(1);
        let cancel = options.cancel.clone();
        let wait_result = loop {
            tokio::select! {
                biased;
                () = async {
                    if let Some(tok) = &cancel {
                        tok.cancelled().await
                    } else {
                        std::future::pending::<()>().await
                    }
                } => {
                    let _ = child.kill().await;
                    let stdout_partial_bytes = stdout_handle.await.unwrap_or_default();
                    let stdout_partial = String::from_utf8_lossy(&stdout_partial_bytes).to_string();
                    let stderr_partial = stderr_handle.await.unwrap_or_default();
                    scrub_bare_repo_after_command(options, &effective_cwd, &original_cwd);
                    cleanup_cwd_file(&built.cwd_file_path);
                    return Ok(CommandResult {
                        exit_code: -1,
                        stdout: stdout_partial,
                        stdout_bytes: Some(stdout_partial_bytes),
                        stderr: if stderr_partial.is_empty() {
                            "Command interrupted".to_string()
                        } else {
                            format!("{stderr_partial}\nCommand interrupted")
                        },
                        // Streaming path collects stderr as String only —
                        // PowerShell (the only stderr-bytes consumer)
                        // uses the non-streaming path.
                        stderr_bytes: None,
                        new_cwd: None,
                        timed_out: false,
                        interrupted: true,
                    });
                }
                result = child.wait() => {
                    break result.map(Some);
                }
                () = tokio::time::sleep(progress_interval) => {
                    let elapsed = start.elapsed();
                    if elapsed > timeout_duration {
                        let _ = child.kill().await;
                        scrub_bare_repo_after_command(options, &effective_cwd, &original_cwd);
                        cleanup_cwd_file(&built.cwd_file_path);
                        return Ok(CommandResult {
                            exit_code: -1,
                            stdout: String::new(),
                            stdout_bytes: None,
                            stderr: format!("Command timed out after {timeout_ms}ms"),
                            stderr_bytes: None,
                            new_cwd: None,
                            timed_out: true,
                            interrupted: false,
                        });
                    }
                    let total = stdout_bytes.load(Ordering::Relaxed)
                        + stderr_bytes.load(Ordering::Relaxed);
                    on_progress(ShellProgress {
                        elapsed_seconds: elapsed.as_secs_f64(),
                        total_bytes: total,
                    });
                }
            }
        };

        let status = wait_result?;
        let stdout_raw = stdout_handle.await.unwrap_or_default();
        let stdout = String::from_utf8_lossy(&stdout_raw).to_string();
        let stderr = stderr_handle.await.unwrap_or_default();

        let new_cwd = if options.prevent_cwd_changes {
            None
        } else {
            read_cwd_file(&built.cwd_file_path)
        };
        cleanup_cwd_file(&built.cwd_file_path);

        if let Some(ref new) = new_cwd {
            self.cwd = new.clone();
        }

        let exit_code = status.and_then(|s| s.code()).unwrap_or(-1);

        if let Some(s) = &status {
            record_seccomp_violation_if_killed(options, command, s).await;
        }

        scrub_bare_repo_after_command(options, &effective_cwd, &original_cwd);

        Ok(CommandResult {
            exit_code,
            stdout,
            stdout_bytes: Some(stdout_raw),
            stderr,
            stderr_bytes: None,
            new_cwd,
            timed_out: false,
            interrupted: false,
        })
    }

    async fn build_command(
        &self,
        command: &str,
        _options: &ExecOptions,
        sandbox_tmp_dir: Option<PathBuf>,
    ) -> BuiltCommand {
        let use_sandbox = sandbox_tmp_dir.is_some();
        let opts = BuildExecOpts {
            id: next_command_id(),
            sandbox_tmp_dir,
            use_sandbox,
        };
        self.provider.build_exec_command(command, &opts).await
    }

    async fn merge_env(
        &self,
        command: &str,
        options: &ExecOptions,
        sandbox_tmp_dir: Option<PathBuf>,
    ) -> std::collections::HashMap<String, String> {
        let use_sandbox = sandbox_tmp_dir.is_some();
        let opts = BuildExecOpts {
            id: 0, // env_overrides doesn't depend on id
            sandbox_tmp_dir,
            use_sandbox,
        };
        let mut env = options.extra_env.clone();
        // Provider overrides applied AFTER caller's extra_env so sandbox
        // isolation (TMPDIR / TMPPREFIX) wins over `/env TMPDIR=…`.
        for (k, v) in self.provider.env_overrides(command, &opts).await {
            env.insert(k, v);
        }
        env
    }
}

fn should_use_sandbox(options: &ExecOptions, command: &str) -> bool {
    let Some(state) = &options.sandbox else {
        return false;
    };
    state
        .command_snapshot(command, options.sandbox_bypass)
        .should_wrap
}

/// Record a Linux seccomp SIGSYS kill as a sandbox violation so the model sees
/// it in the `<sandbox_violations>` annotation. No-op off Linux / no sandbox.
async fn record_seccomp_violation_if_killed(
    options: &ExecOptions,
    command: &str,
    status: &std::process::ExitStatus,
) {
    #[cfg(target_os = "linux")]
    if let Some(state) = &options.sandbox
        && coco_sandbox::is_seccomp_violation(status)
    {
        let tag = coco_sandbox::generate_command_tag(command, state.session_tag());
        state
            .record_violation(coco_sandbox::seccomp_violation(Some(tag)))
            .await;
    }
    #[cfg(not(target_os = "linux"))]
    let _ = (options, command, status);
}

/// Linux netns-bridge shell prefix for a sandboxed command, derived from the
/// live sandbox snapshot. `None` on macOS / when no bridge is active.
fn sandbox_inner_bridge_prefix(options: &ExecOptions, command: &str) -> Option<String> {
    options.sandbox.as_ref().and_then(|s| {
        s.command_snapshot(command, options.sandbox_bypass)
            .inner_command_prefix
    })
}

fn read_cwd_file(path: &Path) -> Option<PathBuf> {
    let contents = std::fs::read_to_string(path).ok()?;
    let trimmed = contents.trim();
    if trimmed.is_empty() {
        return None;
    }
    let buf = PathBuf::from(trimmed);
    if buf.is_absolute() { Some(buf) } else { None }
}

fn cleanup_cwd_file(path: &Path) {
    if let Err(err) = std::fs::remove_file(path)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        tracing::debug!("Failed to remove cwd file {}: {err:?}", path.display());
    }
}

/// Mutate `cmd` to wrap the spawn with platform sandbox enforcement.
///
/// No-op if `options.sandbox` is `None`, or if the sandbox is inactive, or if
/// the command is excluded from sandboxing per
/// [`coco_sandbox::SandboxState::command_snapshot`].
///
/// When `sandbox_tmp_dir` is `Some`, that path is passed to the platform
/// wrap so it can be bind-mounted (bwrap) or carve-outed (Seatbelt) as
/// writable inside the sandbox — letting the inner shell's
/// `pwd -P >| <tmpdir>/cwd-N` actually write where the parent can read.
fn apply_sandbox_wrap(
    cmd: &mut tokio::process::Command,
    command: &str,
    options: &ExecOptions,
    sandbox_tmp_dir: Option<&std::path::Path>,
) -> anyhow::Result<()> {
    let Some(state) = &options.sandbox else {
        return Ok(());
    };
    let binds: Vec<PathBuf> = sandbox_tmp_dir
        .map(|p| vec![p.to_path_buf()])
        .unwrap_or_default();
    state
        .try_wrap_command_with_binds(command, options.sandbox_bypass, &binds, cmd)
        .map_err(|e| anyhow::anyhow!("sandbox wrap failed: {e}"))?;
    Ok(())
}

/// Best-effort post-command scrub of planted bare-repo files in `cwd` /
/// `original_cwd`. Mitigation for anthropics/claude-code#29316.
fn scrub_bare_repo_after_command(options: &ExecOptions, cwd: &Path, original_cwd: &Path) {
    if options.sandbox.is_none() {
        return;
    }
    let paths = coco_sandbox::bare_repo_scrub_paths(cwd, original_cwd);
    if !paths.is_empty() {
        coco_sandbox::scrub_bare_repo_files(&paths);
    }
}

/// Progress update during streaming shell execution.
#[derive(Debug, Clone)]
pub struct ShellProgress {
    /// Elapsed time in seconds since command started.
    pub elapsed_seconds: f64,
    /// Total bytes of combined stdout+stderr output so far.
    pub total_bytes: i64,
}

#[cfg(test)]
#[path = "executor.test.rs"]
mod tests;
