//! Schedule store — cron-task + remote-trigger persistence.
//!
//! Cron-task (`CronTask` + add/remove/list/markFired) and the trigger surface
//! used by `RemoteTriggerTool`. Injected
//! into tools via `ToolUseContext.schedules`, same pattern as `SideQuery` /
//! `McpHandle`. The disk-backed implementation lives in
//! [`crate::disk_backed_schedule_store`].

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// A scheduled prompt. The on-disk
/// JSON uses camelCase (`createdAt`, `lastFiredAt`); the runtime-only `durable`
/// and `agent_id` fields are never serialized.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CronTask {
    pub id: String,
    /// 5-field cron string (local time), validated on write + re-validated on read.
    pub cron: String,
    /// Prompt enqueued when the task fires.
    pub prompt: String,
    /// Epoch ms when created — anchor for missed-task detection.
    pub created_at: i64,
    /// Epoch ms of the most recent fire (recurring only); lets next-fire
    /// computation survive restarts. Never set for one-shots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_fired_at: Option<i64>,
    /// When `Some(true)`, reschedule after firing instead of deleting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurring: Option<bool>,
    /// When `Some(true)`, exempt from recurring auto-expiry (system tasks only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permanent: Option<bool>,
    /// Runtime-only: `Some(false)` = session-scoped (never written to disk).
    /// File-backed tasks leave this `None`. Stripped on write.
    #[serde(skip)]
    pub durable: Option<bool>,
    /// Runtime-only: set when created by an in-process teammate. Never on disk.
    #[serde(skip)]
    pub agent_id: Option<String>,
}

impl CronTask {
    /// `true` when the task reschedules after firing (missing/false = one-shot).
    pub fn is_recurring(&self) -> bool {
        self.recurring.unwrap_or(false)
    }
}

/// A remote trigger entry (CCR-backed; see `RemoteTriggerTool`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerEntry {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

/// Current wall-clock in epoch ms.
pub(crate) fn now_epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 8-hex-char id derived from the first 8 chars of a `randomUUID()`.
pub(crate) fn short_task_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..8].to_string()
}

/// Build a fresh `CronTask` with `created_at = now`. Shared by the in-memory and
/// disk stores so id/timestamp generation stays identical.
pub(crate) fn new_cron_task(
    cron: &str,
    prompt: &str,
    recurring: bool,
    durable: bool,
    agent_id: Option<&str>,
) -> CronTask {
    CronTask {
        id: short_task_id(),
        cron: cron.to_string(),
        prompt: prompt.to_string(),
        created_at: now_epoch_ms(),
        last_fired_at: None,
        recurring: recurring.then_some(true),
        permanent: None,
        durable: Some(durable),
        agent_id: agent_id.map(str::to_string),
    }
}

/// Cron-task + trigger CRUD. The scheduler tick driver and the `Cron*` / `RemoteTrigger` tools
/// share one injected store.
#[async_trait::async_trait]
pub trait ScheduleStore: Send + Sync {
    // ── Cron tasks ──
    /// Append a task; returns the created record. `durable=false` → session
    /// (in-memory) only; `durable=true` → persisted to disk.
    async fn add_cron_task(
        &self,
        cron: &str,
        prompt: &str,
        recurring: bool,
        durable: bool,
        agent_id: Option<&str>,
    ) -> Result<CronTask, coco_error::BoxedError>;
    /// Remove tasks by id (no-op for ids that don't match).
    async fn remove_cron_tasks(&self, ids: &[&str]) -> Result<(), coco_error::BoxedError>;
    /// All tasks (file-backed + session), merged.
    async fn list_all_cron_tasks(&self) -> Result<Vec<CronTask>, coco_error::BoxedError>;
    /// Stamp `last_fired_at` on the given (file-backed) recurring tasks.
    async fn mark_cron_tasks_fired(
        &self,
        ids: &[&str],
        fired_at: i64,
    ) -> Result<(), coco_error::BoxedError>;

    // ── Remote triggers (RemoteTriggerTool — CCR; see its struct doc) ──
    async fn create_trigger(
        &self,
        name: &str,
        description: Option<&str>,
    ) -> Result<TriggerEntry, coco_error::BoxedError>;
    async fn list_triggers(&self) -> Result<Vec<TriggerEntry>, coco_error::BoxedError>;
    async fn get_trigger(&self, id: &str) -> Result<TriggerEntry, coco_error::BoxedError>;
    async fn update_trigger(
        &self,
        id: &str,
        body: Value,
    ) -> Result<TriggerEntry, coco_error::BoxedError>;
    async fn run_trigger(&self, id: &str) -> Result<String, coco_error::BoxedError>;
}

pub type ScheduleStoreRef = Arc<dyn ScheduleStore>;

/// Session-scoped in-memory store. All tasks live in memory (no disk), so it's
/// the natural fallback for sessions without a project dir + the test double.
#[derive(Debug, Default)]
pub struct InMemoryScheduleStore {
    tasks: RwLock<Vec<CronTask>>,
    triggers: RwLock<HashMap<String, TriggerEntry>>,
}

impl InMemoryScheduleStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait::async_trait]
impl ScheduleStore for InMemoryScheduleStore {
    async fn add_cron_task(
        &self,
        cron: &str,
        prompt: &str,
        recurring: bool,
        durable: bool,
        agent_id: Option<&str>,
    ) -> Result<CronTask, coco_error::BoxedError> {
        let task = new_cron_task(cron, prompt, recurring, durable, agent_id);
        self.tasks.write().await.push(task.clone());
        Ok(task)
    }

    async fn remove_cron_tasks(&self, ids: &[&str]) -> Result<(), coco_error::BoxedError> {
        self.tasks
            .write()
            .await
            .retain(|t| !ids.contains(&t.id.as_str()));
        Ok(())
    }

    async fn list_all_cron_tasks(&self) -> Result<Vec<CronTask>, coco_error::BoxedError> {
        Ok(self.tasks.read().await.clone())
    }

    async fn mark_cron_tasks_fired(
        &self,
        ids: &[&str],
        fired_at: i64,
    ) -> Result<(), coco_error::BoxedError> {
        let mut tasks = self.tasks.write().await;
        for t in tasks.iter_mut() {
            if ids.contains(&t.id.as_str()) {
                t.last_fired_at = Some(fired_at);
            }
        }
        Ok(())
    }

    async fn create_trigger(
        &self,
        name: &str,
        description: Option<&str>,
    ) -> Result<TriggerEntry, coco_error::BoxedError> {
        let entry = TriggerEntry {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.to_string(),
            description: description.map(str::to_string),
        };
        self.triggers
            .write()
            .await
            .insert(entry.id.clone(), entry.clone());
        Ok(entry)
    }

    async fn list_triggers(&self) -> Result<Vec<TriggerEntry>, coco_error::BoxedError> {
        Ok(self.triggers.read().await.values().cloned().collect())
    }

    async fn get_trigger(&self, id: &str) -> Result<TriggerEntry, coco_error::BoxedError> {
        self.triggers
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| not_found(format!("trigger '{id}' not found")))
    }

    async fn update_trigger(
        &self,
        id: &str,
        _body: Value,
    ) -> Result<TriggerEntry, coco_error::BoxedError> {
        self.get_trigger(id).await
    }

    async fn run_trigger(&self, id: &str) -> Result<String, coco_error::BoxedError> {
        let trigger = self.get_trigger(id).await?;
        Ok(format!("Triggered {}", trigger.name))
    }
}

pub(crate) fn not_found(message: String) -> coco_error::BoxedError {
    Box::new(coco_error::PlainError::new(
        message,
        coco_error::StatusCode::FileNotFound,
    ))
}

fn unavailable<T>() -> Result<T, coco_error::BoxedError> {
    Err(Box::new(coco_error::PlainError::new(
        "scheduling not available in this context",
        coco_error::StatusCode::Internal,
    )))
}

/// No-op store for contexts without scheduling (test double / headless helpers).
#[derive(Debug, Clone)]
pub struct NoOpScheduleStore;

#[async_trait::async_trait]
impl ScheduleStore for NoOpScheduleStore {
    async fn add_cron_task(
        &self,
        _cron: &str,
        _prompt: &str,
        _recurring: bool,
        _durable: bool,
        _agent_id: Option<&str>,
    ) -> Result<CronTask, coco_error::BoxedError> {
        unavailable()
    }
    async fn remove_cron_tasks(&self, _ids: &[&str]) -> Result<(), coco_error::BoxedError> {
        Ok(())
    }
    async fn list_all_cron_tasks(&self) -> Result<Vec<CronTask>, coco_error::BoxedError> {
        Ok(vec![])
    }
    async fn mark_cron_tasks_fired(
        &self,
        _ids: &[&str],
        _fired_at: i64,
    ) -> Result<(), coco_error::BoxedError> {
        Ok(())
    }
    async fn create_trigger(
        &self,
        _name: &str,
        _description: Option<&str>,
    ) -> Result<TriggerEntry, coco_error::BoxedError> {
        unavailable()
    }
    async fn list_triggers(&self) -> Result<Vec<TriggerEntry>, coco_error::BoxedError> {
        Ok(vec![])
    }
    async fn get_trigger(&self, id: &str) -> Result<TriggerEntry, coco_error::BoxedError> {
        Err(not_found(format!(
            "trigger '{id}' not found (scheduling not available)"
        )))
    }
    async fn update_trigger(
        &self,
        id: &str,
        _body: Value,
    ) -> Result<TriggerEntry, coco_error::BoxedError> {
        Err(not_found(format!(
            "trigger '{id}' not found (scheduling not available)"
        )))
    }
    async fn run_trigger(&self, id: &str) -> Result<String, coco_error::BoxedError> {
        Err(not_found(format!(
            "trigger '{id}' not found (scheduling not available)"
        )))
    }
}

#[cfg(test)]
#[path = "schedule_store.test.rs"]
mod tests;
