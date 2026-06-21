//! Shell-task spawning, driver, and terminal-state composition.

use std::time::Duration;

use coco_tasks::{
    NotificationKind, NotificationSink, TaskCreateRequest, TaskManager, TaskNotification,
    TerminalStatus,
};
use coco_tool_runtime::BackgroundShellKind;
use coco_tool_runtime::BackgroundShellRequest;
use coco_types::{TaskStatus, TaskType};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio_util::sync::CancellationToken;
use tracing::{info, instrument, warn};

use super::TaskRuntime;
use super::stall;
use super::timers::{spawn_auto_detach_timer, spawn_progress_timer};
use crate::disk_task_output::DiskTaskOutput;

static BACKGROUND_SHELL_COMMAND_ID: AtomicU64 = AtomicU64::new(1);

impl TaskRuntime {
    #[instrument(
        level = "info",
        skip(self, request),
        fields(
            command_preview = %command_preview(&request.command),
            timeout_ms = ?request.timeout_ms,
            agent_id = ?request.issuing_agent,
        )
    )]
    pub(super) async fn spawn_shell_task_impl(
        &self,
        request: BackgroundShellRequest,
    ) -> Result<String, coco_error::BoxedError> {
        let task_id = coco_types::generate_task_id(TaskType::Shell);
        let dto = self.disk.get_or_create(&task_id).await;
        let output_path = dto.path().display().to_string();
        let cancel = CancellationToken::new();
        // Shell tasks reach this path only via `BashTool` with
        // `run_in_background=true` — by definition backgrounded. Stash
        // the typed shell extras (kind / command / agent_id) on the
        // canonical row so introspection has the same shape TS exposes.
        let shell_extras = coco_types::ShellExtras {
            kind: None,
            command: request.command.clone(),
            issuing_agent: request.issuing_agent.clone(),
            exit_code: None,
            is_backgrounded: true,
        };
        let assigned = self
            .manager
            .create_task(TaskCreateRequest {
                task_id: task_id.clone(),
                task_type: TaskType::Shell,
                description: request.description.clone(),
                output_file: Some(output_path.clone()),
                tool_use_id: request.tool_use_id.clone(),
                is_backgrounded: true,
                status: TaskStatus::Running,
                cancel: cancel.clone(),
                invoking_agent: request.issuing_agent.clone(),
                shell_extras: Some(shell_extras),
            })
            .await;
        debug_assert_eq!(assigned, task_id);
        info!(
            target: "coco::task_runtime::shell",
            task_id = %task_id,
            description = %request.description,
            output_file = %output_path,
            "background shell task spawned"
        );

        let manager = self.manager.clone();
        let sink = self.notification_sink.clone();
        let driver_task_id = task_id.clone();
        let driver_description = request.description.clone();
        let driver_tool_use_id = request.tool_use_id.clone();
        let driver_agent_id = request.issuing_agent.clone();
        let driver_output_path = output_path.clone();
        let timeout_ms = request.timeout_ms.unwrap_or(120_000);

        let dto_for_driver = dto.clone();
        let cancel_for_driver = cancel.clone();
        // W3: rename `stall_cancel` → `drain_done`. Fired by the driver
        // tokio task after `run_shell_task` returns; stops the stall
        // watchdog, progress timer, and auto-detach timer in one
        // coordinated signal.
        let drain_done = CancellationToken::new();
        tokio::spawn(stall::watchdog(
            task_id.clone(),
            request.description.clone(),
            request.tool_use_id.clone(),
            request.issuing_agent.clone(),
            output_path.clone(),
            dto.clone(),
            sink.clone(),
            drain_done.clone(),
        ));

        // W3: progress timer — emits `bash_progress` events through
        // `progress_tx` every `progress_throttle_ms` (~1s) while the
        // task runs. The unified fg/bg path lets fg `tool.execute`
        // observe progress via the same `ctx.progress_tx` channel it
        // always used.
        if let Some(progress_tx) = request.progress_tx.clone() {
            spawn_progress_timer(
                task_id.clone(),
                request.tool_use_id.clone().unwrap_or_default(),
                request.progress_throttle_ms.max(100),
                dto.clone(),
                progress_tx,
                drain_done.clone(),
            );
        }

        // W3: auto-detach timer — fires `signal_detach(task_id)` after
        // `auto_detach_ms` of fg execution (15 s blocking budget).
        // Stops when the task terminates (`drain_done` fires). Bails
        // when the task is already terminal at fire time.
        if let Some(ms) = request.auto_detach_ms {
            spawn_auto_detach_timer(
                task_id.clone(),
                ms,
                self.manager.clone(),
                drain_done.clone(),
            );
        }

        let drain_done_for_driver = drain_done;
        let request_for_driver = request.clone();
        tokio::spawn(async move {
            let outcome = run_shell_task(
                request_for_driver,
                timeout_ms,
                cancel_for_driver,
                dto_for_driver,
            )
            .await;
            drain_done_for_driver.cancel();
            apply_shell_terminal_state(
                &manager,
                &driver_task_id,
                &driver_description,
                driver_tool_use_id.as_deref(),
                driver_agent_id.as_deref(),
                &driver_output_path,
                sink.as_ref(),
                outcome,
            )
            .await;
        });

        Ok(task_id)
    }
}

/// Result of one shell-task execution. Carries enough information
/// for `apply_shell_terminal_state` to compose the summary string
/// and status.
enum WaitOutcome {
    Exited { code: i32 },
    TimedOut { budget_ms: i64 },
    Cancelled,
    SpawnFailed,
}

/// Subset of [`WaitOutcome`] the terminal-apply step needs.
/// `stderr_tail` lives separately so it can be threaded through
/// without polluting the lifecycle enum.
struct ShellOutcome {
    wait: WaitOutcome,
}

/// Spawn the child process directly (bypassing `coco_shell::ShellExecutor`
/// — the BashTool security pipeline already cleared the command at
/// the foreground entry point in `bash.rs::execute`). Streams stdout
/// + stderr to the per-task disk file in real time so the stall
/// watchdog observes growth.
///
/// W6: applies sandbox wrap (`bwrap` / Seatbelt) when `sandbox_state`
/// is `Some` and the command isn't excluded by the sandbox settings.
/// Mirrors `coco_shell::executor::apply_sandbox_wrap` so the
/// TaskRuntime unified path doesn't lose the sandbox guarantee that
/// the legacy `ShellExecutor` foreground path provided.
#[instrument(
    level = "debug",
    skip(cancel, dto, request),
    fields(command_preview = %command_preview(&request.command), timeout_ms, kill_on_timeout = request.kill_on_timeout, sandboxed = request.sandbox_state.is_some())
)]
async fn run_shell_task(
    request: BackgroundShellRequest,
    timeout_ms: i64,
    cancel: CancellationToken,
    dto: DiskTaskOutput,
) -> ShellOutcome {
    use tokio::io::AsyncReadExt;
    use tokio::process::Command;

    let command = request.command.as_str();
    let sandbox_tmp_dir = match &request.sandbox_state {
        Some(state)
            if state
                .command_snapshot(command, request.sandbox_bypass)
                .should_wrap =>
        {
            coco_sandbox::SandboxState::allocate_command_tmp_dir()
        }
        _ => None,
    };
    let sandbox_tmp_path = sandbox_tmp_dir.as_ref().map(|dir| dir.path().to_path_buf());

    let (program, args, env_overrides): (std::path::PathBuf, Vec<String>, Vec<(String, String)>) =
        match request.shell_kind.clone() {
            BackgroundShellKind::DefaultPlatformShell => {
                #[cfg(windows)]
                {
                    (
                        std::path::PathBuf::from("cmd.exe"),
                        vec!["/C".to_string(), command.to_string()],
                        Vec::new(),
                    )
                }
                #[cfg(not(windows))]
                {
                    (
                        std::path::PathBuf::from("/bin/bash"),
                        vec!["-c".to_string(), command.to_string()],
                        Vec::new(),
                    )
                }
            }
            BackgroundShellKind::Provider(provider) => {
                let use_sandbox = sandbox_tmp_path.is_some();
                let opts = coco_shell::BuildExecOpts {
                    id: BACKGROUND_SHELL_COMMAND_ID.fetch_add(1, Ordering::Relaxed),
                    sandbox_tmp_dir: sandbox_tmp_path.clone(),
                    use_sandbox,
                };
                let built = provider.build_exec_command(command, &opts).await;
                let args = provider.spawn_args(&built.command_string);
                let env_overrides = provider
                    .env_overrides(command, &opts)
                    .await
                    .into_iter()
                    .collect();
                (provider.shell_path().to_path_buf(), args, env_overrides)
            }
        };

    let mut cmd = Command::new(program);
    cmd.args(&args);
    for (key, value) in env_overrides {
        cmd.env(key, value);
    }
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    // W6: sandbox wrap. `try_wrap_command_with_binds` mutates `cmd`
    // in place to swap the program/args with the platform-specific
    // wrapper (bwrap on Linux, Seatbelt sandbox-exec on macOS).
    // No-op when sandbox is None / inactive / command excluded.
    if let Some(state) = &request.sandbox_state
        && let Err(e) = state.try_wrap_command_with_binds(
            command,
            request.sandbox_bypass,
            &sandbox_tmp_path.iter().cloned().collect::<Vec<_>>(),
            &mut cmd,
        )
    {
        warn!(
            target: "coco::task_runtime::shell",
            error = %e,
            "sandbox wrap failed; spawning unsandboxed"
        );
    }
    let mut child = match cmd.kill_on_drop(true).spawn() {
        Ok(c) => c,
        Err(e) => {
            warn!(
                target: "coco::task_runtime::shell",
                error = %e,
                "failed to spawn child process"
            );
            let msg = format!("\n[failed to spawn shell child: {e}]\n");
            dto.append(&msg);
            let _ = dto.flush().await;
            return ShellOutcome {
                wait: WaitOutcome::SpawnFailed,
            };
        }
    };

    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    let dto_stdout = dto.clone();
    let stdout_handle = tokio::spawn(async move {
        if let Some(mut pipe) = stdout_pipe {
            let mut buf = vec![0u8; 8192];
            loop {
                match pipe.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        let chunk = String::from_utf8_lossy(&buf[..n]);
                        dto_stdout.append(&chunk);
                    }
                    Err(_) => break,
                }
            }
        }
    });

    let dto_stderr = dto.clone();
    let stderr_handle = tokio::spawn(async move {
        let mut tail = String::new();
        if let Some(mut pipe) = stderr_pipe {
            let mut buf = vec![0u8; 8192];
            loop {
                match pipe.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        let chunk = String::from_utf8_lossy(&buf[..n]).into_owned();
                        dto_stderr.append(&chunk);
                        tail.push_str(&chunk);
                        if tail.len() > 4096 {
                            let cut = tail.len() - 4096;
                            tail.drain(..cut);
                        }
                    }
                    Err(_) => break,
                }
            }
        }
        tail
    });

    let timeout_duration = Duration::from_millis(timeout_ms.max(0) as u64);
    let outcome = tokio::select! {
        biased;
        () = cancel.cancelled() => {
            let _ = child.kill().await;
            WaitOutcome::Cancelled
        }
        result = child.wait() => match result {
            Ok(status) => WaitOutcome::Exited { code: status.code().unwrap_or(-1) },
            Err(e) => {
                warn!(target: "coco::task_runtime::shell", error = %e, "child wait failed");
                WaitOutcome::SpawnFailed
            }
        },
        // When `kill_on_timeout` is false (auto-backgroundable fg command),
        // this arm is disabled: the timeout no longer kills the child. The fg
        // awaiter is released separately by the auto-detach timer (which fires
        // at the same `timeout_ms`), and the child runs to natural exit in the
        // background — TS `shouldAutoBackground` parity.
        () = tokio::time::sleep(timeout_duration), if request.kill_on_timeout => {
            let _ = child.kill().await;
            WaitOutcome::TimedOut { budget_ms: timeout_ms }
        }
    };

    let _ = stdout_handle.await;
    // stderr tail is already on disk; the join result is intentionally
    // dropped — the terminal-apply step reads task state from
    // TaskManager, not from in-memory tails.
    let _ = stderr_handle.await;
    let _ = dto.flush().await;

    ShellOutcome { wait: outcome }
}

/// Final lifecycle update for a shell task: flip status,
/// broadcast on the watch, persist exit_code into the per-task
/// `OnceLock` (W3: so `read_terminal_outputs` can return it to the
/// fg `tool.execute` caller), and push the terminal notification.
#[allow(clippy::too_many_arguments)]
async fn apply_shell_terminal_state(
    manager: &TaskManager,
    task_id: &str,
    description: &str,
    tool_use_id: Option<&str>,
    agent_id: Option<&str>,
    output_path: &str,
    sink: &dyn NotificationSink,
    outcome: ShellOutcome,
) {
    let (status, terminal, exit_code) = match outcome.wait {
        WaitOutcome::Exited { code: 0 } => {
            (TaskStatus::Completed, TerminalStatus::Completed, Some(0))
        }
        WaitOutcome::Exited { code } => {
            // Non-zero exit is treated as failure.
            (TaskStatus::Failed, TerminalStatus::Failed, Some(code))
        }
        WaitOutcome::TimedOut { budget_ms } => {
            // Timeout is surfaced as Failed for the model, with a
            // clearer log line showing the budget.
            warn!(
                target: "coco::task_runtime::shell",
                task_id,
                budget_ms,
                "shell task exceeded budget"
            );
            (TaskStatus::Failed, TerminalStatus::Failed, None)
        }
        WaitOutcome::Cancelled => (TaskStatus::Killed, TerminalStatus::Killed, None),
        WaitOutcome::SpawnFailed => (TaskStatus::Failed, TerminalStatus::Failed, None),
    };
    // W3: persist exit_code BEFORE update_status, so any awaiter
    // racing the terminal signal sees a consistent snapshot.
    if let Some(code) = exit_code {
        manager.set_exit_code(task_id, code).await;
    }
    if manager.transition_terminal(task_id, status).await.is_none() {
        return;
    }
    if !manager.mark_notified_once(task_id).await {
        return;
    }

    let n = TaskNotification {
        task_id: task_id.to_string(),
        tool_use_id: tool_use_id.map(String::from),
        agent_id: agent_id.map(String::from),
        output_file: output_path.to_string(),
        description: description.to_string(),
        kind: NotificationKind::ShellTerminal {
            status: terminal,
            exit_code,
        },
    };
    sink.push(n).await;
    info!(
        target: "coco::task_runtime::shell",
        task_id,
        status = ?status,
        "background shell task terminal"
    );
}

/// Short identifier shown in `tracing` spans for shell commands.
/// Avoids logging the entire command which can be hundreds of
/// characters with heredocs / pipes.
fn command_preview(cmd: &str) -> String {
    truncate_for_label(cmd, 60)
}

fn truncate_for_label(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}
