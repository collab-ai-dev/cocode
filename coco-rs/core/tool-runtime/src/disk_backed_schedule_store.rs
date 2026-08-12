//! Disk-backed schedule store.
//!
//! Durable tasks persist to `project config dir/scheduled_tasks.json`; session tasks
//! (`durable = false`) live in memory and die with the process. A missing file
//! is empty, while malformed durable state fails closed so a later mutation
//! cannot silently erase it. Invalid cron rows also fail closed rather than
//! disappearing during the next mutation. The runtime-only `durable` /
//! `agent_id` fields are
//! stripped on write (serde-skip), so the on-disk shape stays
//! `{ id, cron, prompt, createdAt, lastFiredAt?, recurring?, permanent? }`.

use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::fs::File;
use std::fs::OpenOptions;
use std::path::Path;
use std::path::PathBuf;
use tokio::sync::Mutex;
use tokio::sync::RwLock;

use crate::schedule_store::CronPayload;
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
/// (`project config dir/scheduled_tasks.json`).
#[derive(Debug)]
pub struct DiskBackedScheduleStore {
    cron_file_path: PathBuf,
    lock_file_path: PathBuf,
    mutation_lock: Mutex<()>,
    session_tasks: RwLock<Vec<CronTask>>,
    triggers: RwLock<HashMap<String, TriggerEntry>>,
}

fn boxed(message: String) -> coco_error::BoxedError {
    Box::new(coco_error::PlainError::new(
        message,
        coco_error::StatusCode::Internal,
    ))
}

fn open_lock_file(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("schedule lock `{}` is not a regular file", path.display()),
        ));
    }
    Ok(file)
}

impl DiskBackedScheduleStore {
    pub fn new(cron_file_path: PathBuf) -> Self {
        let lock_file_path = cron_file_path.with_extension("json.lock");
        Self {
            cron_file_path,
            lock_file_path,
            mutation_lock: Mutex::new(()),
            session_tasks: RwLock::new(Vec::new()),
            triggers: RwLock::new(HashMap::new()),
        }
    }

    /// File-backed tasks. A missing file is empty; corruption fails closed.
    async fn read_file_tasks(&self) -> Result<Vec<CronTask>, coco_error::BoxedError> {
        const MAX_SCHEDULE_BYTES: usize = 8 * 1024 * 1024;
        let bytes = match coco_utils_common::read_regular_async(&self.cron_file_path).await {
            Ok(bytes) if bytes.len() <= MAX_SCHEDULE_BYTES => bytes,
            Ok(_) => {
                return Err(boxed(format!(
                    "read {}: schedule exceeds {MAX_SCHEDULE_BYTES} bytes",
                    self.cron_file_path.display()
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(boxed(format!(
                    "read {}: {error}",
                    self.cron_file_path.display()
                )));
            }
        };
        let raw = coco_file_encoding::detect_encoding(&bytes)
            .decode(&bytes)
            .map_err(|error| {
                boxed(format!(
                    "decode {}: {error}; refusing to overwrite a corrupt schedule",
                    self.cron_file_path.display()
                ))
            })?;
        let file: CronFile = serde_json::from_str(&raw).map_err(|error| {
            boxed(format!(
                "parse {}: {error}; refusing to overwrite a corrupt schedule",
                self.cron_file_path.display()
            ))
        })?;
        if let Some(invalid) = file
            .tasks
            .iter()
            .find(|task| !coco_cron::is_valid_cron_expression(&task.cron))
        {
            return Err(boxed(format!(
                "parse {}: task '{}' has invalid cron expression; refusing to overwrite a corrupt schedule",
                self.cron_file_path.display(),
                invalid.id
            )));
        }
        Ok(file.tasks)
    }

    /// Overwrite the file (creating `project config dir/`). `durable` / `agent_id` are
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
        let path = self.cron_file_path.clone();
        tokio::task::spawn_blocking(move || coco_utils_common::replace_regular_atomic(&path, json))
            .await
            .map_err(|error| boxed(format!("schedule writer join failed: {error}")))?
            .map(|_| ())
            .map_err(|error| boxed(format!("write {}: {error}", self.cron_file_path.display())))
    }

    async fn acquire_file_lease(&self) -> Result<File, coco_error::BoxedError> {
        let path = self.lock_file_path.clone();
        tokio::task::spawn_blocking(move || {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            if std::fs::symlink_metadata(&path)
                .is_ok_and(|metadata| metadata.file_type().is_symlink())
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("schedule lock `{}` is a symlink", path.display()),
                ));
            }
            let file = open_lock_file(&path)?;
            fs2::FileExt::lock_exclusive(&file)?;
            Ok(file)
        })
        .await
        .map_err(|error| boxed(format!("schedule lease join failed: {error}")))?
        .map_err(|error| {
            boxed(format!(
                "acquire {}: {error}",
                self.lock_file_path.display()
            ))
        })
    }
}

#[async_trait::async_trait]
impl ScheduleStore for DiskBackedScheduleStore {
    async fn add_cron_task(
        &self,
        cron: &str,
        payload: CronPayload,
        recurring: bool,
        durable: bool,
        agent_id: Option<&str>,
    ) -> Result<CronTask, coco_error::BoxedError> {
        let task = new_cron_task(cron, payload, recurring, durable, agent_id);
        if durable {
            let _local = self.mutation_lock.lock().await;
            let _lease = self.acquire_file_lease().await?;
            let mut tasks = self.read_file_tasks().await?;
            tasks.push(task.clone());
            self.write_file_tasks(&tasks).await?;
        } else {
            self.session_tasks.write().await.push(task.clone());
        }
        Ok(task)
    }

    async fn remove_cron_tasks(&self, ids: &[&str]) -> Result<(), coco_error::BoxedError> {
        let _local = self.mutation_lock.lock().await;
        let _lease = self.acquire_file_lease().await?;
        let tasks = self.read_file_tasks().await?;
        let remaining: Vec<CronTask> = tasks
            .iter()
            .filter(|t| !ids.contains(&t.id.as_str()))
            .cloned()
            .collect();
        if remaining.len() != tasks.len() {
            self.write_file_tasks(&remaining).await?;
        }
        self.session_tasks
            .write()
            .await
            .retain(|task| !ids.contains(&task.id.as_str()));
        Ok(())
    }

    async fn list_all_cron_tasks(&self) -> Result<Vec<CronTask>, coco_error::BoxedError> {
        let mut out = self.read_file_tasks().await?;
        out.extend(self.session_tasks.read().await.iter().cloned());
        Ok(out)
    }

    async fn mark_cron_tasks_fired(
        &self,
        ids: &[&str],
        fired_at: i64,
    ) -> Result<(), coco_error::BoxedError> {
        let _local = self.mutation_lock.lock().await;
        let _lease = self.acquire_file_lease().await?;
        // Persist file tasks first so an I/O error cannot leave the in-memory
        // half changed while the method reports failure.
        let mut tasks = self.read_file_tasks().await?;
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
        let mut session = self.session_tasks.write().await;
        for task in session.iter_mut() {
            if ids.contains(&task.id.as_str()) {
                task.last_fired_at = Some(fired_at);
            }
        }
        Ok(())
    }

    async fn claim_cron_task(
        &self,
        expected: &CronTask,
        fired_at: i64,
        remove_after_claim: bool,
    ) -> Result<Option<CronTask>, coco_error::BoxedError> {
        if expected.durable == Some(false) {
            let mut tasks = self.session_tasks.write().await;
            let Some(index) = tasks.iter().position(|task| task == expected) else {
                return Ok(None);
            };
            let claimed = tasks[index].clone();
            if remove_after_claim {
                tasks.remove(index);
            } else {
                tasks[index].last_fired_at = Some(fired_at);
            }
            return Ok(Some(claimed));
        }

        let _local = self.mutation_lock.lock().await;
        let _lease = self.acquire_file_lease().await?;

        let mut tasks = self.read_file_tasks().await?;
        let Some(index) = tasks.iter().position(|task| task == expected) else {
            return Ok(None);
        };
        let claimed = tasks[index].clone();
        if remove_after_claim {
            tasks.remove(index);
        } else {
            tasks[index].last_fired_at = Some(fired_at);
        }
        self.write_file_tasks(&tasks).await?;
        Ok(Some(claimed))
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
