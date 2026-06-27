//! Implementations of `coco_tool_runtime::TaskListHandle` / `TodoListHandle`
//! over this crate's concrete stores. Kept in a separate module so
//! the pure data types don't need to know about the handle trait.

use coco_tool_runtime::TaskClaimOutcome;
use coco_tool_runtime::TaskListHandle;
use coco_tool_runtime::TaskListStatus;
use coco_tool_runtime::TaskRecord;
use coco_tool_runtime::TaskRecordUpdate;
use coco_tool_runtime::TodoListHandle;
use coco_tool_runtime::TodoRecord;
use std::collections::HashMap;

use crate::task_list::ClaimResult;
use crate::task_list::Task;
use crate::task_list::TaskListStore;
use crate::task_list::TaskStatus;
use crate::task_list::TaskUpdate;
use crate::todos::TodoItem;
use crate::todos::TodoStore;

fn status_to_wire(s: TaskStatus) -> TaskListStatus {
    match s {
        TaskStatus::Pending => TaskListStatus::Pending,
        TaskStatus::InProgress => TaskListStatus::InProgress,
        TaskStatus::Completed => TaskListStatus::Completed,
    }
}

fn status_from_wire(s: TaskListStatus) -> TaskStatus {
    match s {
        TaskListStatus::Pending => TaskStatus::Pending,
        TaskListStatus::InProgress => TaskStatus::InProgress,
        TaskListStatus::Completed => TaskStatus::Completed,
    }
}

fn task_to_record(t: Task) -> TaskRecord {
    TaskRecord {
        id: t.id,
        subject: t.subject,
        description: t.description,
        active_form: t.active_form,
        owner: t.owner,
        status: status_to_wire(t.status),
        blocks: t.blocks,
        blocked_by: t.blocked_by,
        metadata: t.metadata,
    }
}

fn update_from_wire(u: TaskRecordUpdate) -> TaskUpdate {
    TaskUpdate {
        subject: u.subject,
        description: u.description,
        active_form: u.active_form,
        owner: u.owner,
        status: u.status.map(status_from_wire),
        metadata_merge: u.metadata_merge,
    }
}

fn claim_to_outcome(c: ClaimResult) -> TaskClaimOutcome {
    match c {
        ClaimResult::Success(t) => TaskClaimOutcome::Success(task_to_record(t)),
        ClaimResult::TaskNotFound => TaskClaimOutcome::TaskNotFound,
        ClaimResult::AlreadyClaimed(t) => TaskClaimOutcome::AlreadyClaimed(task_to_record(t)),
        ClaimResult::AlreadyResolved(t) => TaskClaimOutcome::AlreadyResolved(task_to_record(t)),
        ClaimResult::Blocked {
            task,
            blocked_by_tasks,
        } => TaskClaimOutcome::Blocked {
            task: task_to_record(task),
            blocked_by_tasks,
        },
        ClaimResult::AgentBusy {
            task,
            busy_with_tasks,
        } => TaskClaimOutcome::AgentBusy {
            task: task_to_record(task),
            busy_with_tasks,
        },
    }
}

fn boxed(e: crate::TasksError) -> coco_error::BoxedError {
    Box::new(e)
}

#[async_trait::async_trait]
impl TaskListHandle for TaskListStore {
    async fn create_task(
        &self,
        subject: String,
        description: String,
        active_form: Option<String>,
        metadata: Option<HashMap<String, serde_json::Value>>,
    ) -> Result<TaskRecord, coco_error::BoxedError> {
        self.create_task(subject, description, active_form, metadata)
            .await
            .map(task_to_record)
            .map_err(boxed)
    }

    async fn get_task(&self, task_id: &str) -> Result<Option<TaskRecord>, coco_error::BoxedError> {
        self.get_task(task_id)
            .await
            .map(|o| o.map(task_to_record))
            .map_err(boxed)
    }

    async fn list_tasks(&self) -> Result<Vec<TaskRecord>, coco_error::BoxedError> {
        self.list_tasks()
            .await
            .map(|v| v.into_iter().map(task_to_record).collect())
            .map_err(boxed)
    }

    async fn update_task(
        &self,
        task_id: &str,
        updates: TaskRecordUpdate,
    ) -> Result<Option<TaskRecord>, coco_error::BoxedError> {
        self.update_task(task_id, update_from_wire(updates))
            .await
            .map(|o| o.map(task_to_record))
            .map_err(boxed)
    }

    async fn delete_task(&self, task_id: &str) -> Result<bool, coco_error::BoxedError> {
        self.delete_task(task_id).await.map_err(boxed)
    }

    async fn block_task(&self, from_id: &str, to_id: &str) -> Result<bool, coco_error::BoxedError> {
        self.block_task(from_id, to_id).await.map_err(boxed)
    }

    async fn claim_task(
        &self,
        task_id: &str,
        claimant: &str,
        check_agent_busy: bool,
    ) -> Result<TaskClaimOutcome, coco_error::BoxedError> {
        self.claim_task(task_id, claimant, check_agent_busy)
            .await
            .map(claim_to_outcome)
            .map_err(boxed)
    }

    async fn should_nudge_verification(&self, just_completed: bool, is_main_thread: bool) -> bool {
        self.should_nudge_verification_after_update(just_completed, is_main_thread)
            .await
    }

    async fn notify_change(&self) {
        TaskListStore::notify_change(self);
    }

    async fn unassign_teammate_tasks(
        &self,
        teammate_id: &str,
        teammate_name: &str,
    ) -> Result<Vec<(String, String)>, coco_error::BoxedError> {
        self.unassign_teammate_tasks(teammate_id, teammate_name)
            .await
            .map_err(boxed)
    }
}

#[async_trait::async_trait]
impl TodoListHandle for TodoStore {
    async fn read(&self, key: &str) -> Vec<TodoRecord> {
        self.read(key)
            .into_iter()
            .map(|i| TodoRecord {
                content: i.content,
                status: i.status,
                active_form: i.active_form,
            })
            .collect()
    }

    async fn write(&self, key: &str, items: Vec<TodoRecord>) {
        let converted = items
            .into_iter()
            .map(|r| TodoItem {
                content: r.content,
                status: r.status,
                active_form: r.active_form,
            })
            .collect();
        self.write(key, converted);
    }
}
