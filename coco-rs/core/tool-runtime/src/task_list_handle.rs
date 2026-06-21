//! Persistent task-list + per-agent todo-list handle traits.
//!
//! Follows the same callback-decoupling pattern as
//! [`crate::agent_handle::AgentHandle`], [`crate::mailbox_handle::MailboxHandle`],
//! etc. The concrete implementations wrap `coco_tasks::TaskListStore` +
//! `coco_tasks::TodoStore` in the app layer; `coco-tool` stays dep-free.
//!
//! Data types (`TaskRecord`, `TodoRecord`, `TaskListStatus`, etc.) live
//! in `coco-types` so `ToolAppState` can carry typed snapshots without
//! a reverse dep into `coco-tool`. We re-export here for discoverability.

pub use coco_types::ExpandedView;
pub use coco_types::TaskClaimOutcome;
pub use coco_types::TaskListStatus;
pub use coco_types::TaskRecord;
pub use coco_types::TaskRecordUpdate;
pub use coco_types::TodoRecord;

use std::collections::HashMap;
use std::sync::Arc;

/// Shared verification-nudge gate used by both V1 `TodoWrite` and V2
/// `TaskUpdate`.
///
/// Returns `true` when the model should be nudged to spawn a
/// verification agent: ≥3 items, all supposedly finished, none matches
/// `/verif/i`. Callers pre-check "main-thread" and "all-done" so this
/// function stays pure.
pub fn check_verification_nudge(items: &[&str]) -> bool {
    if items.len() < 3 {
        return false;
    }
    !items
        .iter()
        .any(|s| s.to_ascii_lowercase().contains("verif"))
}

/// Access to the durable, disk-backed plan-item store.
///
/// **Single store per session**, shared across tools + subagents +
/// teammates. Implementations serialize cross-thread access via file
/// locks (`.lock` + per-task lock) using `proper-lockfile` semantics.
#[async_trait::async_trait]
pub trait TaskListHandle: Send + Sync {
    async fn create_task(
        &self,
        subject: String,
        description: String,
        active_form: Option<String>,
        metadata: Option<HashMap<String, serde_json::Value>>,
    ) -> Result<TaskRecord, coco_error::BoxedError>;

    async fn get_task(&self, task_id: &str) -> Result<Option<TaskRecord>, coco_error::BoxedError>;

    async fn list_tasks(&self) -> Result<Vec<TaskRecord>, coco_error::BoxedError>;

    async fn update_task(
        &self,
        task_id: &str,
        updates: TaskRecordUpdate,
    ) -> Result<Option<TaskRecord>, coco_error::BoxedError>;

    async fn delete_task(&self, task_id: &str) -> Result<bool, coco_error::BoxedError>;

    async fn block_task(&self, from_id: &str, to_id: &str) -> Result<bool, coco_error::BoxedError>;

    async fn claim_task(
        &self,
        task_id: &str,
        claimant: &str,
        check_agent_busy: bool,
    ) -> Result<TaskClaimOutcome, coco_error::BoxedError>;

    /// Should we emit a verification-agent nudge after this update?
    /// Implementations check "main-thread, all completed, ≥3 tasks,
    /// none match `/verif/i`".
    async fn should_nudge_verification(&self, just_completed: bool, is_main_thread: bool) -> bool;

    /// Fire a "tasks changed" notification without mutating any task.
    /// Default no-op for in-memory / no-op stores; the disk-backed store
    /// overrides it to emit its change broadcast. Used by team teardown,
    /// which removes the whole task-list directory out-of-band (so no
    /// task op runs to fire the usual notification).
    async fn notify_change(&self) {}

    /// Clear ownership of every task owned by a stopping teammate,
    /// resetting each to its pending state so another teammate can
    /// claim it. Returns the `(task_id, subject)` pairs that were
    /// unassigned. Default no-op for in-memory / no-op stores; the
    /// disk-backed store overrides it. Invoked from the leader's
    /// `ShutdownApproved` inbox handler.
    async fn unassign_teammate_tasks(
        &self,
        _teammate_id: &str,
        _teammate_name: &str,
    ) -> Result<Vec<(String, String)>, coco_error::BoxedError> {
        Ok(Vec::new())
    }
}

pub type TaskListHandleRef = Arc<dyn TaskListHandle>;

/// Session-level router for the active task list.
///
/// Team creation needs to switch the leader from the session task list to
/// the team task list immediately. The concrete app layer owns how a
/// task-list id maps to storage; tools and coordinator only need this
/// typed callback.
#[async_trait::async_trait]
pub trait TeamTaskListRouter: Send + Sync {
    async fn route_team_task_list(
        &self,
        task_list_id: &str,
    ) -> Result<TaskListHandleRef, coco_error::BoxedError>;

    async fn clear_team_task_list_route(&self) -> Result<(), coco_error::BoxedError>;
}

pub type TeamTaskListRouterRef = Arc<dyn TeamTaskListRouter>;

#[derive(Debug, Clone)]
pub struct NoOpTeamTaskListRouter;

#[async_trait::async_trait]
impl TeamTaskListRouter for NoOpTeamTaskListRouter {
    async fn route_team_task_list(
        &self,
        _task_list_id: &str,
    ) -> Result<TaskListHandleRef, coco_error::BoxedError> {
        Err(Box::new(coco_error::PlainError::new(
            "team task-list routing is not available in this context",
            coco_error::StatusCode::Internal,
        )))
    }

    async fn clear_team_task_list_route(&self) -> Result<(), coco_error::BoxedError> {
        Ok(())
    }
}

/// In-memory implementation for tests. Real sessions wire up the
/// disk-backed `coco_tasks::TaskListStore` instead.
///
/// Matches the semantic of the persistent store closely enough for
/// unit testing: sequential integer IDs, cascade on delete, atomic
/// claim with busy-check guard, metadata merge with null deletion.
pub struct InMemoryTaskListHandle {
    inner: std::sync::Mutex<InMemoryState>,
}

struct InMemoryState {
    tasks: HashMap<String, TaskRecord>,
    next_id: i64,
    high_water_mark: i64,
}

impl InMemoryTaskListHandle {
    pub fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(InMemoryState {
                tasks: HashMap::new(),
                next_id: 1,
                high_water_mark: 0,
            }),
        }
    }
}

impl Default for InMemoryTaskListHandle {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl TaskListHandle for InMemoryTaskListHandle {
    async fn create_task(
        &self,
        subject: String,
        description: String,
        active_form: Option<String>,
        metadata: Option<HashMap<String, serde_json::Value>>,
    ) -> Result<TaskRecord, coco_error::BoxedError> {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Advance next_id past HWM so deleted ids don't get reassigned.
        if guard.high_water_mark >= guard.next_id {
            guard.next_id = guard.high_water_mark + 1;
        }
        let id = guard.next_id.to_string();
        guard.next_id += 1;
        let task = TaskRecord {
            id: id.clone(),
            subject,
            description,
            active_form,
            owner: None,
            status: TaskListStatus::Pending,
            blocks: Vec::new(),
            blocked_by: Vec::new(),
            metadata,
        };
        guard.tasks.insert(id, task.clone());
        Ok(task)
    }

    async fn get_task(&self, task_id: &str) -> Result<Option<TaskRecord>, coco_error::BoxedError> {
        Ok(self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .tasks
            .get(task_id)
            .cloned())
    }

    async fn list_tasks(&self) -> Result<Vec<TaskRecord>, coco_error::BoxedError> {
        Ok(self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .tasks
            .values()
            .cloned()
            .collect())
    }

    async fn update_task(
        &self,
        task_id: &str,
        updates: TaskRecordUpdate,
    ) -> Result<Option<TaskRecord>, coco_error::BoxedError> {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(task) = guard.tasks.get_mut(task_id) else {
            return Ok(None);
        };
        if let Some(v) = updates.subject {
            task.subject = v;
        }
        if let Some(v) = updates.description {
            task.description = v;
        }
        if let Some(v) = updates.active_form {
            task.active_form = Some(v);
        }
        if let Some(v) = updates.owner {
            task.owner = Some(v);
        }
        if let Some(v) = updates.status {
            task.status = v;
        }
        if let Some(merge) = updates.metadata_merge {
            let mut base = task.metadata.clone().unwrap_or_default();
            for (k, v) in merge {
                if v.is_null() {
                    base.remove(&k);
                } else {
                    base.insert(k, v);
                }
            }
            task.metadata = if base.is_empty() { None } else { Some(base) };
        }
        Ok(Some(task.clone()))
    }

    async fn delete_task(&self, task_id: &str) -> Result<bool, coco_error::BoxedError> {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Bump HWM before deletion.
        if let Ok(numeric) = task_id.parse::<i64>()
            && numeric > guard.high_water_mark
        {
            guard.high_water_mark = numeric;
        }
        let removed = guard.tasks.remove(task_id).is_some();
        if removed {
            // Cascade: strip the id from siblings.
            for sibling in guard.tasks.values_mut() {
                sibling.blocks.retain(|b| b != task_id);
                sibling.blocked_by.retain(|b| b != task_id);
            }
        }
        Ok(removed)
    }

    async fn block_task(&self, from_id: &str, to_id: &str) -> Result<bool, coco_error::BoxedError> {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !guard.tasks.contains_key(from_id) || !guard.tasks.contains_key(to_id) {
            return Ok(false);
        }
        let mut changed = false;
        if let Some(t) = guard.tasks.get_mut(from_id)
            && !t.blocks.iter().any(|b| b == to_id)
        {
            t.blocks.push(to_id.to_string());
            changed = true;
        }
        if let Some(t) = guard.tasks.get_mut(to_id)
            && !t.blocked_by.iter().any(|b| b == from_id)
        {
            t.blocked_by.push(from_id.to_string());
            changed = true;
        }
        Ok(changed)
    }

    async fn claim_task(
        &self,
        task_id: &str,
        claimant: &str,
        check_agent_busy: bool,
    ) -> Result<TaskClaimOutcome, coco_error::BoxedError> {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(task) = guard.tasks.get(task_id).cloned() else {
            return Ok(TaskClaimOutcome::TaskNotFound);
        };
        if let Some(owner) = &task.owner
            && owner != claimant
        {
            return Ok(TaskClaimOutcome::AlreadyClaimed(task));
        }
        if task.status == TaskListStatus::Completed {
            return Ok(TaskClaimOutcome::AlreadyResolved(task));
        }
        let unresolved: std::collections::HashSet<String> = guard
            .tasks
            .values()
            .filter(|t| t.status != TaskListStatus::Completed)
            .map(|t| t.id.clone())
            .collect();
        let blocked_by: Vec<String> = task
            .blocked_by
            .iter()
            .filter(|id| unresolved.contains(*id))
            .cloned()
            .collect();
        if !blocked_by.is_empty() {
            return Ok(TaskClaimOutcome::Blocked {
                task,
                blocked_by_tasks: blocked_by,
            });
        }
        if check_agent_busy {
            let busy_with: Vec<String> = guard
                .tasks
                .values()
                .filter(|t| {
                    t.status != TaskListStatus::Completed
                        && t.owner.as_deref() == Some(claimant)
                        && t.id != task_id
                })
                .map(|t| t.id.clone())
                .collect();
            if !busy_with.is_empty() {
                return Ok(TaskClaimOutcome::AgentBusy {
                    task,
                    busy_with_tasks: busy_with,
                });
            }
        }
        if let Some(t) = guard.tasks.get_mut(task_id) {
            t.owner = Some(claimant.to_string());
            return Ok(TaskClaimOutcome::Success(t.clone()));
        }
        Ok(TaskClaimOutcome::TaskNotFound)
    }

    async fn should_nudge_verification(&self, just_completed: bool, is_main_thread: bool) -> bool {
        if !is_main_thread || !just_completed {
            return false;
        }
        let guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tasks: Vec<&TaskRecord> = guard.tasks.values().collect();
        if tasks.len() < 3 {
            return false;
        }
        let all_done = tasks.iter().all(|t| t.status == TaskListStatus::Completed);
        if !all_done {
            return false;
        }
        !tasks
            .iter()
            .any(|t| t.subject.to_ascii_lowercase().contains("verif"))
    }
}

/// No-op implementation for non-swarm sessions lacking a store.
/// Returns `Err` on writes so misconfigured tool use surfaces loudly.
pub struct NoOpTaskListHandle;

#[async_trait::async_trait]
impl TaskListHandle for NoOpTaskListHandle {
    async fn create_task(
        &self,
        _subject: String,
        _description: String,
        _active_form: Option<String>,
        _metadata: Option<HashMap<String, serde_json::Value>>,
    ) -> Result<TaskRecord, coco_error::BoxedError> {
        Err(Box::new(coco_error::PlainError::new(
            "task-list handle not configured (no persistent store)",
            coco_error::StatusCode::Internal,
        )))
    }
    async fn get_task(&self, _task_id: &str) -> Result<Option<TaskRecord>, coco_error::BoxedError> {
        Ok(None)
    }
    async fn list_tasks(&self) -> Result<Vec<TaskRecord>, coco_error::BoxedError> {
        Ok(Vec::new())
    }
    async fn update_task(
        &self,
        _task_id: &str,
        _updates: TaskRecordUpdate,
    ) -> Result<Option<TaskRecord>, coco_error::BoxedError> {
        Ok(None)
    }
    async fn delete_task(&self, _task_id: &str) -> Result<bool, coco_error::BoxedError> {
        Ok(false)
    }
    async fn block_task(
        &self,
        _from_id: &str,
        _to_id: &str,
    ) -> Result<bool, coco_error::BoxedError> {
        Ok(false)
    }
    async fn claim_task(
        &self,
        _task_id: &str,
        _claimant: &str,
        _check_agent_busy: bool,
    ) -> Result<TaskClaimOutcome, coco_error::BoxedError> {
        Ok(TaskClaimOutcome::TaskNotFound)
    }
    async fn should_nudge_verification(
        &self,
        _just_completed: bool,
        _is_main_thread: bool,
    ) -> bool {
        false
    }
}

// ── TodoListHandle (V1) ───────────────────────────────────────────────

/// Access to the per-agent ephemeral todo store (V1).
///
/// Key convention: `agent_id.clone().unwrap_or_else(|| session_id)`.
/// Key convention: `agent_id.clone().unwrap_or_else(|| session_id)`.
#[async_trait::async_trait]
pub trait TodoListHandle: Send + Sync {
    async fn read(&self, key: &str) -> Vec<TodoRecord>;
    async fn write(&self, key: &str, items: Vec<TodoRecord>);
}

pub type TodoListHandleRef = Arc<dyn TodoListHandle>;

/// Default in-memory implementation. Suitable for the main process —
/// teammate processes get their own instance; the V1 todo store is
/// per-process.
pub struct InMemoryTodoListHandle {
    inner: std::sync::Mutex<HashMap<String, Vec<TodoRecord>>>,
}

impl InMemoryTodoListHandle {
    pub fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(HashMap::new()),
        }
    }
}

impl Default for InMemoryTodoListHandle {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl TodoListHandle for InMemoryTodoListHandle {
    async fn read(&self, key: &str) -> Vec<TodoRecord> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(key)
            .cloned()
            .unwrap_or_default()
    }

    async fn write(&self, key: &str, items: Vec<TodoRecord>) {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if items.is_empty() {
            guard.remove(key);
        } else {
            guard.insert(key.to_string(), items);
        }
    }
}
