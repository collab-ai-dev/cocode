//! `TaskStatusSource` implementation for per-turn task reminders.

use async_trait::async_trait;
use coco_system_reminder::{TaskRunStatus, TaskStatusSnapshot, TaskStatusSource};
use coco_types::{TaskStateBase, TaskStatus, TaskType};
use tracing::debug;

use super::TaskRuntime;
use crate::disk_task_output::DEFAULT_MAX_READ_BYTES;

#[async_trait]
impl TaskStatusSource for TaskRuntime {
    async fn collect(
        &self,
        agent_id: Option<&str>,
        just_compacted: bool,
    ) -> Vec<TaskStatusSnapshot> {
        let states = self.manager.list().await;
        let mut snapshots = Vec::new();
        let mut skipped_remote = 0usize;
        let mut skipped_pending = 0usize;
        let mut skipped_self = 0usize;
        let mut skipped_terminal = 0usize;
        let mut offset_bookkeeping = 0usize;
        let mut skipped_not_local_agent = 0usize;

        for state in states {
            if state.task_type() == TaskType::RemoteTeammate {
                skipped_remote += 1;
                continue;
            }
            if state.status == TaskStatus::Pending {
                skipped_pending += 1;
                continue;
            }
            if agent_id.is_some_and(|caller| state.id == caller) {
                skipped_self += 1;
                continue;
            }

            match state.status {
                TaskStatus::Running => {
                    if just_compacted {
                        if is_unretrieved_bg_agent(&state) {
                            snapshots.push(snapshot_from_state(state));
                        } else {
                            skipped_not_local_agent += 1;
                        }
                    } else if self.advance_running_output_offset(&state).await {
                        offset_bookkeeping += 1;
                    }
                }
                TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Killed => {
                    if just_compacted && is_unretrieved_bg_agent(&state) {
                        snapshots.push(snapshot_from_state(state));
                    } else {
                        skipped_terminal += 1;
                    }
                }
                TaskStatus::Pending => unreachable!("pending handled above"),
            }
        }

        debug!(
            target: "coco::task_reminder",
            kept = snapshots.len(),
            skipped_remote,
            skipped_pending,
            skipped_self,
            skipped_terminal,
            offset_bookkeeping,
            skipped_not_local_agent,
            caller_agent_id = ?agent_id,
            just_compacted,
            "task_status reminder snapshot built"
        );
        snapshots
    }
}

impl TaskRuntime {
    async fn advance_running_output_offset(&self, state: &TaskStateBase) -> bool {
        if !running_output_is_model_facing(state.task_type()) {
            return false;
        }
        let observed_offset = state.output_offset;
        let new_offset = self.read_running_output_offset(state).await;
        if new_offset != observed_offset {
            return self
                .manager
                .advance_output_offset_if_running(&state.id, observed_offset, new_offset)
                .await;
        }
        false
    }

    async fn read_running_output_offset(&self, state: &TaskStateBase) -> i64 {
        let Some(path) = state.output_file.as_deref().filter(|s| !s.is_empty()) else {
            return state.output_offset;
        };
        match self
            .disk
            .read_delta_at_path(
                &state.id,
                std::path::Path::new(path),
                state.output_offset,
                DEFAULT_MAX_READ_BYTES,
            )
            .await
        {
            Ok((_content, new_offset)) => new_offset,
            Err(err) => {
                debug!(
                    target: "coco::task_reminder",
                    task_id = %state.id,
                    error = %err,
                    "failed to read task output delta"
                );
                state.output_offset
            }
        }
    }
}

fn snapshot_from_state(state: TaskStateBase) -> TaskStatusSnapshot {
    let task_type = state.task_type();
    let progress_summary = state
        .progress()
        .and_then(|progress| progress.summary.clone());
    TaskStatusSnapshot {
        task_id: state.id,
        description: state.description,
        status: map_status(state.status),
        killed_by: state.killed_by,
        task_type,
        progress_summary,
        output_file_path: state.output_file.filter(|path| !path.is_empty()),
    }
}

fn is_unretrieved_bg_agent(state: &TaskStateBase) -> bool {
    state.task_type() == TaskType::BgAgent
        && !state
            .bg_agent_extras()
            .map(|extras| extras.retrieved)
            .unwrap_or(false)
}

fn running_output_is_model_facing(task_type: TaskType) -> bool {
    matches!(
        task_type,
        TaskType::Shell | TaskType::LocalWorkflow | TaskType::Teammate | TaskType::Dream
    )
}

fn map_status(status: TaskStatus) -> TaskRunStatus {
    match status {
        TaskStatus::Running | TaskStatus::Pending => TaskRunStatus::Running,
        TaskStatus::Completed => TaskRunStatus::Completed,
        TaskStatus::Failed => TaskRunStatus::Failed,
        TaskStatus::Killed => TaskRunStatus::Killed,
    }
}
