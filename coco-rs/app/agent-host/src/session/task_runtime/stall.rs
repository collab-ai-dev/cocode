//! Per-task stall watchdog loop.
//!
//! [`watchdog`] — for **shell** tasks only. Spawned by `spawn_shell_task`.
//! Every [`STALL_CHECK_INTERVAL_MS`] checks whether the on-disk output has
//! grown; if it's been frozen for [`STALL_THRESHOLD_MS`] AND the tail looks
//! like an interactive prompt, fires a stall notification and exits.
//!
//! There is intentionally **no** agent-task watchdog: agents have no stall
//! logic, and agents — having no stdin and never emitting prompts — would
//! only ever misfire the shell-shaped "interactive input" advice.

use std::time::Duration;

use coco_tasks::{
    NotificationKind, NotificationSinkRef, STALL_CHECK_INTERVAL_MS, STALL_TAIL_BYTES,
    STALL_THRESHOLD_MS, TaskNotification, matches_interactive_prompt,
};
use tokio_util::sync::CancellationToken;
use tracing::{info, instrument, trace};

use crate::disk_task_output::DiskTaskOutput;

/// Run the watchdog until either the cancel token fires (task ended)
/// or a stall is detected and notified (one-shot). The first
/// non-prompt tail check resets the growth timer to avoid spurious
/// re-checks every 5 s once a task has paused on a long compile.
#[instrument(
    level = "debug",
    skip(dto, sink, cancel),
    fields(task_id = %task_id, description = %description)
)]
#[allow(clippy::too_many_arguments)]
pub async fn watchdog(
    task_id: String,
    description: String,
    tool_use_id: Option<String>,
    agent_id: Option<String>,
    output_file: String,
    dto: DiskTaskOutput,
    sink: NotificationSinkRef,
    cancel: CancellationToken,
) {
    let interval = Duration::from_millis(STALL_CHECK_INTERVAL_MS);
    let threshold = Duration::from_millis(STALL_THRESHOLD_MS);
    let mut last_size: i64 = 0;
    let mut last_growth = tokio::time::Instant::now();
    let mut ticker = tokio::time::interval(interval);
    // Interval's first tick is immediate; the biased `cancel` arm
    // wins if the task already ended, so no skip needed.
    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => {
                trace!(
                    target: "coco::task_runtime::stall",
                    task_id = %task_id,
                    "stall watchdog exiting (cancelled)"
                );
                return;
            }
            _ = ticker.tick() => {
                let size = dto.size().await;
                if size > last_size {
                    last_size = size;
                    last_growth = tokio::time::Instant::now();
                    continue;
                }
                if last_growth.elapsed() < threshold {
                    continue;
                }
                let tail = match dto.read_tail(STALL_TAIL_BYTES).await {
                    Ok(t) => t,
                    Err(_) => continue,
                };
                if !matches_interactive_prompt(&tail) {
                    // Not a prompt — keep watching but reset the
                    // growth marker so next eval is THRESHOLD out.
                    last_growth = tokio::time::Instant::now();
                    continue;
                }
                info!(
                    target: "coco::task_runtime::stall",
                    task_id = %task_id,
                    tail_bytes = tail.len(),
                    "stall detected — pushing stall notification"
                );
                let n = TaskNotification {
                    task_id: task_id.clone(),
                    tool_use_id: tool_use_id.clone(),
                    agent_id: agent_id.clone(),
                    output_file: output_file.clone(),
                    description: description.clone(),
                    kind: NotificationKind::Stall { output_tail: tail },
                };
                sink.push(n).await;
                // One-shot per stall episode.
                return;
            }
        }
    }
}
