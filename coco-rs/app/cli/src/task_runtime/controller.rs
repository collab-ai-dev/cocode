//! `TaskController` trait implementation — kill + detach.

use coco_tool_runtime::DetachOutcome;
use coco_types::TaskKilledBy;
use tracing::{debug, info, instrument};

use super::{TaskRuntime, boxed_msg};

impl TaskRuntime {
    /// W2: signal that a foreground awaiter should detach and let
    /// the task continue in the background. Idempotent — second and
    /// subsequent calls are no-ops (returns
    /// [`DetachOutcome::AlreadyDetached`]).
    ///
    /// Mechanics:
    /// 1. CAS the per-task `detached: AtomicBool` to true. If already
    ///    set, return [`DetachOutcome::AlreadyDetached`] immediately.
    /// 2. Mark `BgAgentExtras.is_backgrounded() = true` so the TUI
    ///    panel filter can hide the task from the fg list.
    /// 3. Call `detach.notify_one()` — wakes the fg `tool.execute`
    ///    `select!` arm awaiting `.notified()`.
    ///
    /// Returns [`DetachOutcome::Unknown`] for unknown task ids.
    #[instrument(level = "info", skip(self), fields(task_id = %task_id))]
    pub async fn signal_detach(&self, task_id: &str) -> DetachOutcome {
        let outcome = self.manager.signal_detach(task_id).await;
        if matches!(outcome, DetachOutcome::Unknown) {
            debug!(
                target: "coco::task_runtime",
                task_id,
                "signal_detach: unknown task id"
            );
        } else if matches!(outcome, DetachOutcome::AlreadyDetached) {
            debug!(target: "coco::task_runtime", task_id, "signal_detach: already detached");
        } else {
            info!(
                target: "coco::task_runtime",
                task_id,
                "signal_detach fired; fg awaiter will receive detach notification"
            );
        }
        outcome
    }
}

impl TaskRuntime {
    /// Kill a running task by firing its cancel token. See trait-level
    /// docs on [`coco_tool_runtime::TaskHandle::kill_task`] for the
    /// double-notification rationale.
    #[instrument(level = "info", skip(self), fields(task_id = %task_id))]
    pub(super) async fn kill_task_impl(&self, task_id: &str) -> Result<(), coco_error::BoxedError> {
        self.kill_task_impl_with_actor(task_id, TaskKilledBy::User)
            .await
    }

    #[instrument(level = "info", skip(self), fields(task_id = %task_id, killed_by = %killed_by.as_str()))]
    pub(super) async fn kill_task_impl_with_actor(
        &self,
        task_id: &str,
        killed_by: TaskKilledBy,
    ) -> Result<(), coco_error::BoxedError> {
        if let Err(e) = self.manager.kill_running_by(task_id, killed_by).await {
            let msg = match e {
                coco_tasks::KillTaskError::NotFound => {
                    format!("No running task found with ID: {task_id}")
                }
                coco_tasks::KillTaskError::NotRunning => {
                    format!("Task is not running: {task_id}")
                }
            };
            return Err(boxed_msg(msg, coco_error::StatusCode::FileNotFound));
        }
        info!(
            target: "coco::task_runtime",
            task_id,
            killed_by = killed_by.as_str(),
            "kill_task fired cancel token; driver will finalize state + push notification"
        );
        Ok(())
    }
}
