//! `TaskStatusSource` impl on [`crate::running::TaskManager`].
//!
//! Fires post-compaction so the model rediscovers background agents
//! whose `<task-notification>` envelope was wiped from the
//! `CommandQueue` by compaction. Both **still-running** agents (so the
//! model doesn't spawn a duplicate) AND **terminal** agents whose
//! results haven't been retrieved must be re-injected — compaction
//! cleared the queue that delivered them inline.
//!
//! ## Filter
//!
//! Skip conditions, applied in order:
//!
//! 1. **Not a BgAgent** — `task_status` reminders are only generated for
//!    backgrounded agent tasks. Shell / dream / teammate / remote
//!    tasks don't carry the "don't spawn a duplicate" affordance.
//! 2. **`status == Pending`** — never started; nothing to report.
//! 3. **`retrieved`** — `TaskOutputTool` already served this task's
//!    output; the model knows what happened.
//! 4. **Same `task_id` as the caller** — fork mode + nested spawns;
//!    an agent doesn't warn itself about itself.

use async_trait::async_trait;
use coco_system_reminder::TaskRunStatus;
use coco_system_reminder::TaskStatusSnapshot;
use coco_system_reminder::TaskStatusSource;
use coco_types::TaskStatus;
use coco_types::TaskType;
use tracing::debug;

use crate::running::TaskManager;
use crate::running::task_type_wire_name;

#[async_trait]
impl TaskStatusSource for TaskManager {
    async fn collect(
        &self,
        agent_id: Option<&str>,
        just_compacted: bool,
    ) -> Vec<TaskStatusSnapshot> {
        if !just_compacted {
            return Vec::new();
        }
        let states = self.list().await;
        let total = states.len();
        let mut kept = 0usize;
        let mut skipped_type = 0usize;
        let mut skipped_pending = 0usize;
        let mut skipped_retrieved = 0usize;
        let mut skipped_self = 0usize;
        let mut snapshots = Vec::with_capacity(states.len());
        for t in states {
            if t.task_type() != TaskType::BgAgent {
                skipped_type += 1;
                continue;
            }
            if t.status == TaskStatus::Pending {
                skipped_pending += 1;
                continue;
            }
            let extras = t.bg_agent_extras().cloned().unwrap_or_default();
            if extras.retrieved {
                skipped_retrieved += 1;
                continue;
            }
            // BgAgent's task id IS its agent id (`a<16hex>`); the caller
            // self-filter compares against the row id directly.
            if let Some(caller_id) = agent_id
                && t.id.as_str() == caller_id
            {
                skipped_self += 1;
                continue;
            }
            kept += 1;
            // Build the `delta_summary`. For terminal-error tasks the
            // recorded `error` text takes precedence; fall through to
            // the progress summary or synthetic counter sentence
            // otherwise.
            let delta_summary = if t.status.is_terminal()
                && let Some(err) = extras.error.as_deref()
                && !err.is_empty()
            {
                Some(err.to_string())
            } else {
                extras.progress.as_ref().map(|p| {
                    p.summary.clone().unwrap_or_else(|| {
                        let mut parts = Vec::new();
                        if let Some(last) = p.last_tool_name.as_deref() {
                            parts.push(format!("last action: {last}"));
                        }
                        if p.tool_use_count > 0 {
                            parts.push(format!("tool uses: {}", p.tool_use_count));
                        }
                        if p.total_tokens > 0 {
                            parts.push(format!("tokens: {}", p.total_tokens));
                        }
                        parts.join(", ")
                    })
                })
            };
            let task_type = t.task_type();
            let output_file_path = t.output_file.clone();
            snapshots.push(TaskStatusSnapshot {
                task_id: t.id,
                description: t.description,
                status: map_status(t.status),
                task_type: task_type_wire_name(task_type).to_string(),
                delta_summary,
                output_file_path,
            });
        }
        debug!(
            target: "coco::task_reminder",
            total,
            kept,
            skipped_type,
            skipped_pending,
            skipped_retrieved,
            skipped_self,
            caller_agent_id = ?agent_id,
            "task_status reminder snapshot built (post-compact, TS-aligned filter)"
        );
        snapshots
    }
}

/// Map the 5-variant `TaskStatus` to the 4-variant `TaskRunStatus`
/// the reminder generator dispatches on (`Pending` collapses into
/// `Running` because the filter above already rejects `Pending`).
fn map_status(s: TaskStatus) -> TaskRunStatus {
    match s {
        TaskStatus::Completed => TaskRunStatus::Completed,
        TaskStatus::Failed => TaskRunStatus::Failed,
        TaskStatus::Killed => TaskRunStatus::Killed,
        TaskStatus::Pending | TaskStatus::Running => TaskRunStatus::Running,
    }
}

#[cfg(test)]
#[path = "reminder_source.test.rs"]
mod tests;
