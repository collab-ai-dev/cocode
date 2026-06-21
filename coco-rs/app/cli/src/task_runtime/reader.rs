//! `TaskReader` trait implementation — read-only inspection of task
//! state and on-disk output.

use std::sync::Arc;

use coco_tool_runtime::{TaskOutputDelta, TerminalOutputs, TerminalSignal};
use coco_types::{TaskStateBase, TaskStatus, TaskType};
use tokio::sync::Notify;
use tracing::trace;

use super::{TaskRuntime, boxed_msg};
use crate::disk_task_output::DEFAULT_MAX_READ_BYTES;

impl TaskRuntime {
    pub(super) async fn get_task_status_impl(
        &self,
        task_id: &str,
    ) -> Result<TaskStateBase, coco_error::BoxedError> {
        self.manager.get(task_id).await.ok_or_else(|| {
            boxed_msg(
                format!("No running task found with ID: {task_id}"),
                coco_error::StatusCode::FileNotFound,
            )
        })
    }

    pub(super) async fn get_task_output_delta_impl(
        &self,
        task_id: &str,
        from_offset: i64,
    ) -> Result<TaskOutputDelta, coco_error::BoxedError> {
        let Some(state) = self.manager.get(task_id).await else {
            return Err(boxed_msg(
                format!("No running task found with ID: {task_id}"),
                coco_error::StatusCode::FileNotFound,
            ));
        };
        let Some(dto) = self.disk.get(task_id).await else {
            return Ok(TaskOutputDelta {
                content: String::new(),
                new_offset: from_offset,
                is_complete: state.status.is_terminal(),
            });
        };
        let _ = dto.flush().await;
        let (content, new_offset) = match dto.read_delta(from_offset, DEFAULT_MAX_READ_BYTES).await
        {
            Ok(pair) => pair,
            Err(_) => (String::new(), from_offset),
        };
        let is_complete = state.status.is_terminal();
        if is_complete && state.task_type() == TaskType::BgAgent {
            self.manager.mark_retrieved(task_id).await;
            trace!(
                target: "coco::task_runtime",
                task_id,
                "marked LocalAgent task as retrieved"
            );
        }
        trace!(
            target: "coco::task_runtime",
            task_id,
            from_offset,
            new_offset,
            delta_bytes = content.len(),
            is_complete,
            "served task output delta"
        );
        Ok(TaskOutputDelta {
            content,
            new_offset,
            is_complete,
        })
    }

    pub(super) async fn list_tasks_impl(&self) -> Vec<TaskStateBase> {
        self.manager.list().await
    }

    pub(super) async fn subscribe_terminal_impl(&self, task_id: &str) -> Option<TerminalSignal> {
        self.manager
            .subscribe_terminal(task_id)
            .await
            .map(TerminalSignal::new)
    }

    pub(super) async fn detach_handle_impl(&self, task_id: &str) -> Option<Arc<Notify>> {
        self.manager.detach_handle(task_id).await
    }

    pub(super) async fn read_terminal_outputs_impl(
        &self,
        task_id: &str,
    ) -> Result<TerminalOutputs, coco_error::BoxedError> {
        let Some(state) = self.manager.get(task_id).await else {
            return Err(boxed_msg(
                format!("No task found with ID: {task_id}"),
                coco_error::StatusCode::FileNotFound,
            ));
        };
        let stdout = if let Some(dto) = self.disk.get(task_id).await {
            let _ = dto.flush().await;
            dto.read_tail(DEFAULT_MAX_READ_BYTES)
                .await
                .unwrap_or_default()
        } else {
            String::new()
        };
        let interrupted = matches!(state.status, TaskStatus::Killed);
        // W3: shell driver writes exit_code into the per-task
        // `OnceLock` from `apply_shell_terminal_state`. Agent tasks
        // and shell `Cancelled` / `TimedOut` / `SpawnFailed` outcomes
        // leave it unset, yielding `None`.
        let exit_code = self.manager.exit_code(task_id).await;
        Ok(TerminalOutputs {
            stdout,
            stderr: String::new(),
            exit_code,
            interrupted,
        })
    }
}
