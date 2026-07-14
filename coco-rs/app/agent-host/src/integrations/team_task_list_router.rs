use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use coco_tool_runtime::TaskClaimOutcome;
use coco_tool_runtime::TaskListHandle;
use coco_tool_runtime::TaskListHandleRef;
use coco_tool_runtime::TaskRecord;
use coco_tool_runtime::TaskRecordUpdate;
use coco_tool_runtime::TeamTaskListRouter;

pub struct RoutedTaskList {
    root: PathBuf,
    initial_id: String,
    current: tokio::sync::RwLock<TaskListHandleRef>,
}

impl RoutedTaskList {
    pub fn open(root: PathBuf, initial_id: String) -> anyhow::Result<Arc<Self>> {
        let current = coco_tasks::TaskListStore::open(&root, &initial_id)?;
        Ok(Arc::new(Self {
            root,
            initial_id,
            current: tokio::sync::RwLock::new(current as TaskListHandleRef),
        }))
    }

    async fn current(&self) -> TaskListHandleRef {
        self.current.read().await.clone()
    }

    async fn replace_with(
        &self,
        task_list_id: &str,
        reset: bool,
    ) -> Result<TaskListHandleRef, coco_error::BoxedError> {
        let store = coco_tasks::TaskListStore::open(&self.root, task_list_id)
            .map_err(|e| Box::new(e) as coco_error::BoxedError)?;
        if reset {
            store
                .reset()
                .await
                .map_err(|e| Box::new(e) as coco_error::BoxedError)?;
        }
        let handle = store as TaskListHandleRef;
        *self.current.write().await = handle.clone();
        Ok(handle)
    }
}

#[async_trait::async_trait]
impl TeamTaskListRouter for RoutedTaskList {
    async fn route_team_task_list(
        &self,
        task_list_id: &str,
    ) -> Result<TaskListHandleRef, coco_error::BoxedError> {
        self.replace_with(task_list_id, true).await
    }

    async fn clear_team_task_list_route(&self) -> Result<(), coco_error::BoxedError> {
        self.replace_with(&self.initial_id, false).await?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl TaskListHandle for RoutedTaskList {
    async fn create_task(
        &self,
        subject: String,
        description: String,
        active_form: Option<String>,
        metadata: Option<HashMap<String, serde_json::Value>>,
    ) -> Result<TaskRecord, coco_error::BoxedError> {
        self.current()
            .await
            .create_task(subject, description, active_form, metadata)
            .await
    }

    async fn get_task(&self, task_id: &str) -> Result<Option<TaskRecord>, coco_error::BoxedError> {
        self.current().await.get_task(task_id).await
    }

    async fn list_tasks(&self) -> Result<Vec<TaskRecord>, coco_error::BoxedError> {
        self.current().await.list_tasks().await
    }

    async fn update_task(
        &self,
        task_id: &str,
        updates: TaskRecordUpdate,
    ) -> Result<Option<TaskRecord>, coco_error::BoxedError> {
        self.current().await.update_task(task_id, updates).await
    }

    async fn delete_task(&self, task_id: &str) -> Result<bool, coco_error::BoxedError> {
        self.current().await.delete_task(task_id).await
    }

    async fn block_task(&self, from_id: &str, to_id: &str) -> Result<bool, coco_error::BoxedError> {
        self.current().await.block_task(from_id, to_id).await
    }

    async fn claim_task(
        &self,
        task_id: &str,
        claimant: &str,
        check_agent_busy: bool,
    ) -> Result<TaskClaimOutcome, coco_error::BoxedError> {
        self.current()
            .await
            .claim_task(task_id, claimant, check_agent_busy)
            .await
    }

    async fn should_nudge_verification(&self, just_completed: bool, is_main_thread: bool) -> bool {
        self.current()
            .await
            .should_nudge_verification(just_completed, is_main_thread)
            .await
    }

    async fn notify_change(&self) {
        self.current().await.notify_change().await;
    }
}
