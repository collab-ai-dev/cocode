//! `task_status` generator.
//!
//! Rendering varies by status:
//!
//! - `Killed`: brief stopped note with actor attribution when available.
//! - `Running`: anti-duplicate warning with type-specific output and
//!   interaction affordances.
//! - `Completed` / `Failed`: outcome summary with result recovery path.
//!
//! Multiple statuses emitted this turn are joined with `\n\n`.

use async_trait::async_trait;
use coco_types::ToolName;

use crate::error::Result;
use crate::generator::AttachmentGenerator;
use crate::generator::GeneratorContext;
use crate::generator::TaskRunStatus;
use crate::generator::TaskStatusSnapshot;
use crate::types::AttachmentType;
use crate::types::SystemReminder;
use coco_config::SystemReminderConfig;

#[derive(Debug, Default)]
pub struct TaskStatusGenerator;

#[async_trait]
impl AttachmentGenerator for TaskStatusGenerator {
    fn name(&self) -> &str {
        "TaskStatusGenerator"
    }

    fn attachment_type(&self) -> AttachmentType {
        AttachmentType::TaskStatus
    }

    fn is_enabled(&self, config: &SystemReminderConfig) -> bool {
        config.attachments.task_status
    }

    async fn generate(&self, ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        if ctx.task_statuses.is_empty() {
            return Ok(None);
        }
        let parts: Vec<String> = ctx.task_statuses.iter().map(render_one).collect();
        Ok(Some(SystemReminder::new(
            AttachmentType::TaskStatus,
            parts.join("\n\n"),
        )))
    }
}

fn render_one(t: &TaskStatusSnapshot) -> String {
    let send_message = ToolName::SendMessage.as_str();
    match t.status {
        TaskRunStatus::Killed => format!(
            "Task \"{desc}\" ({id}){}.",
            killed_suffix(t.killed_by),
            desc = t.description,
            id = t.task_id
        ),
        TaskRunStatus::Running => match t.task_type {
            coco_types::TaskType::BgAgent => {
                let mut parts = vec![format!(
                    "Background agent \"{desc}\" ({id}) is still running.",
                    desc = t.description,
                    id = t.task_id
                )];
                if let Some(s) = t.progress_summary.as_deref() {
                    parts.push(format!("Progress: {s}"));
                }
                parts.push(format!(
                    "Do NOT spawn a duplicate. You will be notified when it completes; use {send_message} only if you need to communicate with it while it runs."
                ));
                parts.join(" ")
            }
            coco_types::TaskType::Shell => render_file_backed_running("Background command", t),
            coco_types::TaskType::LocalWorkflow => render_file_backed_running("Local workflow", t),
            coco_types::TaskType::Teammate => render_file_backed_running("Teammate task", t),
            coco_types::TaskType::Dream => render_file_backed_running("Dream task", t),
            coco_types::TaskType::RemoteTeammate => {
                render_file_backed_running("Remote teammate task", t)
            }
        },
        TaskRunStatus::Completed | TaskRunStatus::Failed => {
            // Format: `Task {id} (type: ...) (status: ...) (description: ...)
            // [Read the output file... | Result output path is unavailable.]`
            // joined by single spaces.
            let display_status = match t.status {
                TaskRunStatus::Completed => "completed",
                TaskRunStatus::Failed => "failed",
                _ => unreachable!("outer match restricts to Completed|Failed"),
            };
            let mut parts = vec![
                format!("Task {id}", id = t.task_id),
                format!("(type: {tt})", tt = t.task_type.wire_name()),
                format!("(status: {display_status})"),
                format!("(description: {desc})", desc = t.description),
            ];
            if let Some(p) = t.output_file_path.as_deref() {
                parts.push(format!("Read the output file to restore the result: {p}"));
            } else {
                parts.push("Result output path is unavailable.".to_string());
            }
            parts.join(" ")
        }
    }
}

fn render_file_backed_running(label: &str, t: &TaskStatusSnapshot) -> String {
    let mut parts = vec![format!(
        "{label} \"{desc}\" ({id}) is still running.",
        desc = t.description,
        id = t.task_id
    )];
    parts.push("Do NOT spawn a duplicate.".to_string());
    parts.join(" ")
}

fn killed_suffix(killed_by: Option<coco_types::TaskKilledBy>) -> &'static str {
    match killed_by {
        Some(coco_types::TaskKilledBy::User) => " was stopped by user",
        Some(coco_types::TaskKilledBy::Parent) => " was stopped by the agent",
        Some(coco_types::TaskKilledBy::System) => " was stopped by system",
        None => " was stopped",
    }
}

#[cfg(test)]
#[path = "task_status.test.rs"]
mod tests;
