//! Phase-aware liveness watchdog for local background agents.
//!
//! Event producers report coarse execution phases through `TaskHandle`; the
//! task manager stores only a monotonic runtime heartbeat. This module owns
//! policy and cancellation because `TaskRuntime` owns the agent lifecycle.

use std::time::Duration;

use coco_tasks::TaskManager;
use coco_tool_runtime::AgentExecutionPhase;
use coco_types::TaskKilledBy;
use tokio_util::sync::CancellationToken;
use tracing::{trace, warn};

#[derive(Debug, Clone, Copy)]
struct AgentLivenessPolicy {
    model_warning_after: Duration,
    model_timeout_after: Duration,
    tool_warning_after: Duration,
    tool_timeout_after: Duration,
    absolute_timeout: Duration,
}

impl Default for AgentLivenessPolicy {
    fn default() -> Self {
        Self {
            model_warning_after: Duration::from_secs(2 * 60),
            model_timeout_after: Duration::from_secs(15 * 60),
            tool_warning_after: Duration::from_secs(10 * 60),
            tool_timeout_after: Duration::from_secs(2 * 60 * 60),
            absolute_timeout: Duration::from_secs(6 * 60 * 60),
        }
    }
}

impl AgentLivenessPolicy {
    fn inactivity_limits(self, phase: AgentExecutionPhase) -> (Duration, Duration) {
        match phase {
            AgentExecutionPhase::AwaitingModel => {
                (self.model_warning_after, self.model_timeout_after)
            }
            AgentExecutionPhase::RunningTool => (self.tool_warning_after, self.tool_timeout_after),
        }
    }
}

pub(super) fn spawn_agent_liveness_watchdog(
    task_id: String,
    manager: std::sync::Arc<TaskManager>,
    cancel: CancellationToken,
) {
    tokio::spawn(watch_agent_liveness(
        task_id,
        manager,
        cancel,
        AgentLivenessPolicy::default(),
    ));
}

async fn watch_agent_liveness(
    task_id: String,
    manager: std::sync::Arc<TaskManager>,
    cancel: CancellationToken,
    policy: AgentLivenessPolicy,
) {
    let Some(mut liveness_rx) = manager.subscribe_agent_liveness(&task_id).await else {
        return;
    };
    let absolute_deadline = tokio::time::Instant::now() + policy.absolute_timeout;
    let mut warned_sequence: Option<u64> = None;

    loop {
        let snapshot = *liveness_rx.borrow_and_update();
        let (warning_after, timeout_after) = policy.inactivity_limits(snapshot.phase);
        let warning_deadline = snapshot.last_progress + warning_after;
        let timeout_deadline = snapshot.last_progress + timeout_after;
        let next_inactivity_deadline = if warned_sequence == Some(snapshot.sequence) {
            timeout_deadline
        } else {
            warning_deadline
        };
        let next_deadline = absolute_deadline.min(next_inactivity_deadline);

        tokio::select! {
            biased;
            () = cancel.cancelled() => {
                trace!(
                    target: "coco::task_runtime::agent_liveness",
                    task_id = %task_id,
                    "agent liveness watchdog exiting (cancelled)"
                );
                return;
            }
            changed = liveness_rx.changed() => {
                if changed.is_err() {
                    return;
                }
            }
            () = tokio::time::sleep_until(next_deadline) => {
                let now = tokio::time::Instant::now();
                if now >= absolute_deadline {
                    warn!(
                        target: "coco::task_runtime::agent_liveness",
                        task_id = %task_id,
                        ?snapshot.phase,
                        "agent exceeded absolute runtime limit; cancelling"
                    );
                    cancel_stalled_agent(&manager, &task_id).await;
                    return;
                }
                if now >= timeout_deadline {
                    warn!(
                        target: "coco::task_runtime::agent_liveness",
                        task_id = %task_id,
                        ?snapshot.phase,
                        inactive_ms = duration_ms(timeout_after),
                        "agent made no progress before phase timeout; cancelling"
                    );
                    cancel_stalled_agent(&manager, &task_id).await;
                    return;
                }
                warned_sequence = Some(snapshot.sequence);
                warn!(
                    target: "coco::task_runtime::agent_liveness",
                    task_id = %task_id,
                    ?snapshot.phase,
                    inactive_ms = duration_ms(warning_after),
                    "agent has not reported progress"
                );
            }
        }
    }
}

fn duration_ms(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

async fn cancel_stalled_agent(manager: &TaskManager, task_id: &str) {
    if let Err(error) = manager.kill_running_by(task_id, TaskKilledBy::System).await {
        trace!(
            target: "coco::task_runtime::agent_liveness",
            task_id,
            %error,
            "agent was already terminal when watchdog attempted cancellation"
        );
    }
}

#[cfg(test)]
#[path = "agent_liveness.test.rs"]
mod tests;
