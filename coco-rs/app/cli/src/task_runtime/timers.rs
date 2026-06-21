//! Background-task timers — progress emission, auto-detach for fg
//! shell tasks, auto-background for fg agent tasks.
//!
//! Each timer:
//! - is `tokio::spawn`ed at task registration / spawn time
//! - races a `drain_done` / `cancel` token against a `sleep` future
//!   (`biased` so cancellation always wins ties)
//! - self-terminates idempotently via the per-task `detached` /
//!   terminal-state checks
//!

use std::sync::Arc;
use std::time::Duration;

use coco_tasks::TaskManager;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::disk_task_output::DiskTaskOutput;

/// W3: per-task progress emitter. Polls `dto.size()` every
/// `throttle_ms`, builds a `bash_progress` payload, and sends through
/// the caller's `ProgressSender`. Self-terminates when `drain_done`
/// fires.
pub(super) fn spawn_progress_timer(
    task_id: String,
    tool_use_id: String,
    throttle_ms: u64,
    dto: DiskTaskOutput,
    progress_tx: coco_tool_runtime::ProgressSender,
    drain_done: CancellationToken,
) {
    tokio::spawn(async move {
        let start = std::time::Instant::now();
        let mut ticker = tokio::time::interval(Duration::from_millis(throttle_ms));
        // Skip the immediate first tick — only emit AFTER the first
        // throttle interval has elapsed.
        ticker.tick().await;
        loop {
            tokio::select! {
                biased;
                () = drain_done.cancelled() => break,
                _ = ticker.tick() => {
                    let total_bytes = dto.size().await;
                    let elapsed_seconds = start.elapsed().as_secs();
                    let payload = serde_json::json!({
                        "type": "bash_progress",
                        "status": "running",
                        "elapsedTimeSeconds": elapsed_seconds,
                        "totalBytes": total_bytes,
                        "taskId": task_id,
                    });
                    // Best-effort send: receiver closed = drop quietly.
                    let _ = progress_tx.send(coco_tool_runtime::ToolProgress {
                        tool_use_id: tool_use_id.clone(),
                        parent_tool_use_id: None,
                        data: payload,
                    });
                }
            }
        }
    });
}

/// Source of an auto-detach event — drives both the log target and
/// the structured field name in the trace event so per-context grep
/// keeps working after the helper was extracted.
#[derive(Debug, Clone, Copy)]
enum DetachReason {
    /// Shell foreground task hit `ASSISTANT_BLOCKING_BUDGET_MS`.
    ShellAutoDetach,
    /// Agent foreground task hit `autoBackgroundMs`.
    AgentAutoBackground,
}

/// Shared body for both auto-detach timers — fires the per-task
/// detach signal exactly once. Bails when:
///   - the task is unknown to the task manager (e.g. removed before
///     the timer fired)
///   - the task has reached a terminal state
///   - the `detached` CAS already flipped (idempotent — another caller
///     of `signal_detach` won the race)
///
/// On success: flips `BgAgentExtras.is_backgrounded() = true`,
/// notifies the per-task `Notify`, and emits a per-reason info log.
/// Returns immediately on every bail-out path.
async fn fire_detach(
    task_id: &str,
    manager: &Arc<TaskManager>,
    reason: DetachReason,
    timeout_ms: u64,
) {
    if !manager.signal_detach(task_id).await.is_first() {
        return;
    }
    // `target:` must be a const string per tracing's macro; dispatch
    // on the reason variant so each path keeps its own grep-able
    // target. The `kind` field labels the reason in the structured
    // event for log aggregators that filter by field rather than target.
    match reason {
        DetachReason::ShellAutoDetach => {
            info!(
                target: "coco::task_runtime::shell",
                task_id,
                auto_detach_ms = timeout_ms,
                kind = "shell_auto_detach",
                "auto-detach timer fired; fg awaiter (if any) will receive detach"
            );
        }
        DetachReason::AgentAutoBackground => {
            info!(
                target: "coco::task_runtime::agent",
                task_id,
                auto_background_ms = timeout_ms,
                kind = "agent_auto_background",
                "auto-background timer fired; fg awaiter (if any) will detach"
            );
        }
    }
}

/// W3: per-task auto-detach timer. Fires after `auto_detach_ms` of
/// execution. Implements auto-background for foreground shell tasks
/// after the blocking budget (15 s by default). Self-terminates on
/// `drain_done` (the task already finished).
pub(super) fn spawn_auto_detach_timer(
    task_id: String,
    auto_detach_ms: u64,
    manager: Arc<TaskManager>,
    drain_done: CancellationToken,
) {
    tokio::spawn(async move {
        tokio::select! {
            biased;
            () = drain_done.cancelled() => return,
            () = tokio::time::sleep(Duration::from_millis(auto_detach_ms)) => {}
        }
        fire_detach(
            &task_id,
            &manager,
            DetachReason::ShellAutoDetach,
            auto_detach_ms,
        )
        .await;
    });
}

/// Auto-background timer for foreground AgentTool spawns. Fires after
/// `auto_background_ms` of execution. Resolves the background signal
/// for the given agent after the configured ms so the fg `select!`
/// arm wakes. Self-terminates on the task's cancel token.
pub(super) fn spawn_agent_auto_background_timer(
    task_id: String,
    auto_background_ms: u64,
    manager: Arc<TaskManager>,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        tokio::select! {
            biased;
            () = cancel.cancelled() => return,
            () = tokio::time::sleep(Duration::from_millis(auto_background_ms)) => {}
        }
        fire_detach(
            &task_id,
            &manager,
            DetachReason::AgentAutoBackground,
            auto_background_ms,
        )
        .await;
    });
}
