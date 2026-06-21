//! Disk-backed schedule store.
//!
//! Durable tasks persist to `<cwd>/.coco/scheduled_tasks.json`; session tasks
//! (`durable = false`) live in memory and die with the process. Reads degrade
//! gracefully (missing / corrupt file → empty list; tasks with an invalid cron
//! string are dropped). The runtime-only `durable` / `agent_id` fields are
//! stripped on write (serde-skip), so the on-disk shape stays
//! `{ id, cron, prompt, createdAt, lastFiredAt?, recurring?, permanent? }`.

use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::sync::RwLock;

use crate::schedule_store::CronTask;
use crate::schedule_store::ScheduleStore;
use crate::schedule_store::TriggerEntry;
use crate::schedule_store::new_cron_task;
use crate::schedule_store::not_found;

#[derive(Debug, Default, Serialize, Deserialize)]
struct CronFile {
    #[serde(default)]
    tasks: Vec<CronTask>,
}

/// Disk-backed cron store. Construct with the resolved cron-file path
/// (`<cwd>/.coco/scheduled_tasks.json`).
#[derive(Debug)]
pub struct DiskBackedScheduleStore {
    cron_file_path: PathBuf,
    session_tasks: RwLock<Vec<CronTask>>,
    triggers: RwLock<HashMap<String, TriggerEntry>>,
}

fn boxed(message: String) -> coco_error::BoxedError {
    Box::new(coco_error::PlainError::new(
        message,
        coco_error::StatusCode::Internal,
    ))
}

impl DiskBackedScheduleStore {
    pub fn new(cron_file_path: PathBuf) -> Self {
        Self {
            cron_file_path,
            session_tasks: RwLock::new(Vec::new()),
            triggers: RwLock::new(HashMap::new()),
        }
    }

    /// File-backed tasks. Missing/corrupt file → `[]`; tasks whose cron no
    /// longer parses are dropped.
    async fn read_file_tasks(&self) -> Vec<CronTask> {
        let Ok((raw, _enc, _le)) =
            coco_file_encoding::read_with_format_async(&self.cron_file_path).await
        else {
            return Vec::new();
        };
        let file: CronFile = serde_json::from_str(&raw).unwrap_or_default();
        file.tasks
            .into_iter()
            .filter(|t| coco_cron::is_valid_cron_expression(&t.cron))
            .collect()
    }

    /// Overwrite the file (creating `.coco/`). `durable` / `agent_id` are
    /// serde-skipped, so they never reach disk. Empty list writes `{"tasks":[]}`.
    async fn write_file_tasks(&self, tasks: &[CronTask]) -> Result<(), coco_error::BoxedError> {
        if let Some(parent) = self.cron_file_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| boxed(format!("create {}: {e}", parent.display())))?;
        }
        let file = CronFile {
            tasks: tasks.to_vec(),
        };
        let json = serde_json::to_string_pretty(&file).map_err(|e| boxed(e.to_string()))? + "\n";
        coco_file_encoding::write_with_format_async(
            &self.cron_file_path,
            &json,
            coco_file_encoding::Encoding::Utf8,
            coco_file_encoding::LineEnding::Lf,
        )
        .await
        .map_err(|e| boxed(format!("write {}: {e}", self.cron_file_path.display())))
    }
}

#[async_trait::async_trait]
impl ScheduleStore for DiskBackedScheduleStore {
    async fn add_cron_task(
        &self,
        cron: &str,
        prompt: &str,
        recurring: bool,
        durable: bool,
        agent_id: Option<&str>,
    ) -> Result<CronTask, coco_error::BoxedError> {
        let task = new_cron_task(cron, prompt, recurring, durable, agent_id);
        if durable {
            let mut tasks = self.read_file_tasks().await;
            tasks.push(task.clone());
            self.write_file_tasks(&tasks).await?;
        } else {
            self.session_tasks.write().await.push(task.clone());
        }
        Ok(task)
    }

    async fn remove_cron_tasks(&self, ids: &[&str]) -> Result<(), coco_error::BoxedError> {
        // Sweep the session store first; then the file.
        self.session_tasks
            .write()
            .await
            .retain(|t| !ids.contains(&t.id.as_str()));
        let tasks = self.read_file_tasks().await;
        let remaining: Vec<CronTask> = tasks
            .iter()
            .filter(|t| !ids.contains(&t.id.as_str()))
            .cloned()
            .collect();
        if remaining.len() != tasks.len() {
            self.write_file_tasks(&remaining).await?;
        }
        Ok(())
    }

    async fn list_all_cron_tasks(&self) -> Result<Vec<CronTask>, coco_error::BoxedError> {
        let mut out = self.read_file_tasks().await;
        out.extend(self.session_tasks.read().await.iter().cloned());
        Ok(out)
    }

    async fn mark_cron_tasks_fired(
        &self,
        ids: &[&str],
        fired_at: i64,
    ) -> Result<(), coco_error::BoxedError> {
        // Session tasks (in-memory) — keep last_fired_at accurate for listing.
        {
            let mut session = self.session_tasks.write().await;
            for t in session.iter_mut() {
                if ids.contains(&t.id.as_str()) {
                    t.last_fired_at = Some(fired_at);
                }
            }
        }
        // File tasks — persist so first-sight anchoring survives restarts.
        let mut tasks = self.read_file_tasks().await;
        let mut changed = false;
        for t in tasks.iter_mut() {
            if ids.contains(&t.id.as_str()) {
                t.last_fired_at = Some(fired_at);
                changed = true;
            }
        }
        if changed {
            self.write_file_tasks(&tasks).await?;
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
        _body: serde_json::Value,
    ) -> Result<TriggerEntry, coco_error::BoxedError> {
        self.get_trigger(id).await
    }

    async fn run_trigger(&self, id: &str) -> Result<String, coco_error::BoxedError> {
        let trigger = self.get_trigger(id).await?;
        Ok(format!("Triggered {}", trigger.name))
    }
}

#[cfg(test)]
#[path = "disk_backed_schedule_store.test.rs"]
mod tests;
