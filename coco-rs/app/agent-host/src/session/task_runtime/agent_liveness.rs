//! Phase-aware liveness watchdog for local background agents.
//!
//! Event producers report coarse execution phases through `TaskHandle`; the
//! task manager stores only a monotonic runtime heartbeat. This module owns
//! policy and cancellation because `TaskRuntime` owns the agent lifecycle.

use std::time::Duration;

use coco_config::AgentLivenessConfig;
use coco_tasks::TaskManager;
use coco_tool_runtime::AgentExecutionPhase;
use coco_types::TaskKilledBy;
use tokio_util::sync::CancellationToken;
use tracing::{trace, warn};

/// Resolved deadlines for one watched agent. Every field is optional because
/// `0` (and, for the absolute cap, an unset value) means "no limit" — an agent
/// that is still emitting output is working, and the operator decides whether
/// to cap that at all.
#[derive(Debug, Clone, Copy)]
struct AgentLivenessPolicy {
    model_warning_after: Option<Duration>,
    model_timeout_after: Option<Duration>,
    tool_warning_after: Option<Duration>,
    tool_timeout_after: Option<Duration>,
    absolute_timeout: Option<Duration>,
}

fn limit(seconds: i64) -> Option<Duration> {
    u64::try_from(seconds)
        .ok()
        .filter(|s| *s > 0)
        .map(Duration::from_secs)
}

impl From<&AgentLivenessConfig> for AgentLivenessPolicy {
    fn from(config: &AgentLivenessConfig) -> Self {
        Self {
            model_warning_after: limit(config.model_warning_after_secs),
            model_timeout_after: limit(config.model_timeout_after_secs),
            tool_warning_after: limit(config.tool_warning_after_secs),
            tool_timeout_after: limit(config.tool_timeout_after_secs),
            absolute_timeout: config.absolute_timeout_secs.and_then(limit),
        }
    }
}

impl AgentLivenessPolicy {
    fn inactivity_limits(self, phase: AgentExecutionPhase) -> (Option<Duration>, Option<Duration>) {
        match phase {
            AgentExecutionPhase::AwaitingModel => {
                (self.model_warning_after, self.model_timeout_after)
            }
            AgentExecutionPhase::RunningTool => (self.tool_warning_after, self.tool_timeout_after),
        }
    }

    fn watches_anything(self) -> bool {
        self.model_warning_after
            .or(self.model_timeout_after)
            .or(self.tool_warning_after)
            .or(self.tool_timeout_after)
            .or(self.absolute_timeout)
            .is_some()
    }
}

pub(super) fn spawn_agent_liveness_watchdog(
    task_id: String,
    manager: std::sync::Arc<TaskManager>,
    cancel: CancellationToken,
    config: &AgentLivenessConfig,
) {
    let policy = AgentLivenessPolicy::from(config);
    if !policy.watches_anything() {
        return;
    }
    tokio::spawn(watch_agent_liveness(task_id, manager, cancel, policy));
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
    let started_at = tokio::time::Instant::now();
    let absolute_deadline = policy.absolute_timeout.map(|limit| started_at + limit);
    let mut warned_sequence: Option<u64> = None;

    loop {
        let snapshot = *liveness_rx.borrow_and_update();
        let (warning_after, timeout_after) = policy.inactivity_limits(snapshot.phase);
        let warning_deadline = warning_after.map(|after| snapshot.last_progress + after);
        let timeout_deadline = timeout_after.map(|after| snapshot.last_progress + after);
        let next_inactivity_deadline = if warned_sequence == Some(snapshot.sequence) {
            timeout_deadline
        } else {
            // No warning limit configured: wait straight for the timeout.
            warning_deadline.or(timeout_deadline)
        };
        // Nothing left to wait for in this phase — park until the heartbeat
        // changes phase or the agent finishes.
        let Some(next_deadline) = earliest(absolute_deadline, next_inactivity_deadline) else {
            tokio::select! {
                biased;
                () = cancel.cancelled() => return,
                changed = liveness_rx.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    continue;
                }
            }
        };

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
                if absolute_deadline.is_some_and(|deadline| now >= deadline) {
                    warn!(
                        target: "coco::task_runtime::agent_liveness",
                        task_id = %task_id,
                        ?snapshot.phase,
                        "agent exceeded absolute runtime limit; cancelling"
                    );
                    cancel_stalled_agent(&manager, &task_id).await;
                    return;
                }
                if timeout_deadline.is_some_and(|deadline| now >= deadline) {
                    warn!(
                        target: "coco::task_runtime::agent_liveness",
                        task_id = %task_id,
                        ?snapshot.phase,
                        inactive_ms = timeout_after.map(duration_ms),
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
                    inactive_ms = warning_after.map(duration_ms),
                    "agent has not reported progress"
                );
            }
        }
    }
}

/// Earliest of two optional deadlines.
fn earliest(
    left: Option<tokio::time::Instant>,
    right: Option<tokio::time::Instant>,
) -> Option<tokio::time::Instant> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (deadline, None) | (None, deadline) => deadline,
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
