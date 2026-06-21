//! `task_status` generator.
//!
//! Rendering varies by status:
//!
//! - `Killed`: brief "stopped by the user" note.
//! - `Running`: anti-duplicate warning; optional delta summary and
//!   output-file pointer.
//! - `Completed` / `Failed`: outcome summary with optional delta.
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
    let task_output = ToolName::TaskOutput.as_str();
    match t.status {
        TaskRunStatus::Killed => format!(
            "Task \"{desc}\" ({id}) was stopped by the user.",
            desc = t.description,
            id = t.task_id
        ),
        TaskRunStatus::Running => {
            // Parts joined by space, with tool-name refs threaded through
            // the anti-duplicate line so the model knows the affordances
            // (`SendMessage` / `TaskOutput`) for steering / inspecting.
            let mut parts = vec![format!(
                "Background agent \"{desc}\" ({id}) is still running.",
                desc = t.description,
                id = t.task_id
            )];
            if let Some(s) = t.delta_summary.as_deref() {
                parts.push(format!("Progress: {s}"));
            }
            if let Some(p) = t.output_file_path.as_deref() {
                parts.push(format!(
                    "Do NOT spawn a duplicate. You will be notified when it completes. You can read partial output at {p} or send it a message with {send_message}."
                ));
            } else {
                parts.push(format!(
                    "Do NOT spawn a duplicate. You will be notified when it completes. You can check its progress with the {task_output} tool or send it a message with {send_message}."
                ));
            }
            parts.join(" ")
        }
        TaskRunStatus::Completed | TaskRunStatus::Failed => {
            // Format: `Task {id} (type: ...) (status: ...) (description: ...)
            // [Delta: ...] [Read the output file...
            //  | You can check its output using the {TASK_OUTPUT_TOOL_NAME} tool.]`
            // joined by single space.
            let display_status = match t.status {
                TaskRunStatus::Completed => "completed",
                TaskRunStatus::Failed => "failed",
                _ => unreachable!("outer match restricts to Completed|Failed"),
            };
            let mut parts = vec![
                format!("Task {id}", id = t.task_id),
                format!("(type: {tt})", tt = t.task_type),
                format!("(status: {display_status})"),
                format!("(description: {desc})", desc = t.description),
            ];
            if let Some(s) = t.delta_summary.as_deref() {
                parts.push(format!("Delta: {s}"));
            }
            if let Some(p) = t.output_file_path.as_deref() {
                parts.push(format!("Read the output file to retrieve the result: {p}"));
            } else {
                parts.push(format!(
                    "You can check its output using the {task_output} tool."
                ));
            }
            parts.join(" ")
        }
    }
}

#[cfg(test)]
#[path = "task_status.test.rs"]
mod tests;
