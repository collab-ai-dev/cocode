//! `task_reminder` generator (V2 task-list nudge).
//!
//! Emitted when the V2 task mutation tools (`TaskCreate` / `TaskUpdate`)
//! haven't been used recently and the V2 feature is active.
//!
//! Gate chain (all must pass, in order):
//!
//! 1. `ctx.is_task_v2_enabled`
//! 2. `Brief` tool is **not** present — when Brief is the primary I/O
//!    channel TaskUpdate becomes a side channel and nudging conflicts.
//! 3. `TaskUpdate` tool is present in `ctx.tools`.
//! 4. `turns_since_last_task_tool >= 10`
//! 5. `turns_since_last_task_reminder >= 10`
//!
//! Content is the reminder body, optionally followed by a
//! newline-separated list of current tasks (`#{id}. [{status}] {subject}`).

use async_trait::async_trait;
use coco_types::TaskRecord;
use coco_types::ToolName;

use crate::error::Result;
use crate::generator::AttachmentGenerator;
use crate::generator::GeneratorContext;
use crate::types::AttachmentType;
use crate::types::SystemReminder;
use coco_config::SystemReminderConfig;

/// Same 10/10 pair as the V1 todo reminder (V2 is a strict superset).
const TURNS_SINCE_TASK_TOOL: i32 = 10;
const TURNS_BETWEEN_REMINDERS: i32 = 10;

/// Tools gating the reminder. Canonical wire strings come from the
/// [`ToolName`] enum's `as_str()`, so a rename flows through automatically.
const REQUIRED_TOOL: ToolName = ToolName::TaskUpdate;
const SUPPRESS_TOOL: ToolName = ToolName::SendUserMessage;

/// Reminder body — `${TASK_CREATE_TOOL_NAME}` / `${TASK_UPDATE_TOOL_NAME}`
/// are resolved through the typed [`ToolName`] enum so a future rename
/// flows through automatically.
fn task_reminder_body() -> String {
    let task_create = ToolName::TaskCreate.as_str();
    let task_update = ToolName::TaskUpdate.as_str();
    format!(
        "The task tools haven't been used recently. If you're working on tasks that would benefit from tracking progress, consider using {task_create} to add new tasks and {task_update} to update task status (set to in_progress when starting, completed when done). Also consider cleaning up the task list if it has become stale. Only use these if relevant to the current work. This is just a gentle reminder - ignore if not applicable. Make sure that you NEVER mention this reminder to the user"
    )
}

/// Nudge the agent to use V2 task tools.
#[derive(Debug, Default)]
pub struct TaskRemindersGenerator;

#[async_trait]
impl AttachmentGenerator for TaskRemindersGenerator {
    fn name(&self) -> &str {
        "TaskRemindersGenerator"
    }

    fn attachment_type(&self) -> AttachmentType {
        AttachmentType::TaskReminder
    }

    fn is_enabled(&self, config: &SystemReminderConfig) -> bool {
        config.attachments.task_reminder
    }

    async fn generate(&self, ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        if !ctx.is_task_v2_enabled {
            return Ok(None);
        }

        if tools_contain(&ctx.tools, SUPPRESS_TOOL) {
            return Ok(None);
        }

        if !tools_contain(&ctx.tools, REQUIRED_TOOL) {
            return Ok(None);
        }

        if ctx.turns_since_last_task_tool < TURNS_SINCE_TASK_TOOL
            || ctx.turns_since_last_task_reminder < TURNS_BETWEEN_REMINDERS
        {
            return Ok(None);
        }

        let content = render_task_reminder_body(&ctx.plan_tasks);
        Ok(Some(SystemReminder::new(
            AttachmentType::TaskReminder,
            content,
        )))
    }
}

/// True when `tools` (raw wire-strings including MCP / custom names)
/// contains the wire form of `builtin`.
fn tools_contain(tools: &[String], builtin: ToolName) -> bool {
    let target = builtin.as_str();
    tools.iter().any(|t| t == target)
}

fn render_task_reminder_body(tasks: &[TaskRecord]) -> String {
    // Base body always ends with `\n`; the optional list suffix adds
    // `\n\n` (3 newlines total before "Here").
    let mut out = format!("{}\n", task_reminder_body());
    if tasks.is_empty() {
        return out;
    }
    // Format: `#${id}. [${status}] ${subject}` joined by `\n`.
    let items = tasks
        .iter()
        .map(|t| {
            format!(
                "#{id}. [{s}] {subj}",
                id = t.id,
                s = t.status.as_str(),
                subj = t.subject
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    out.push_str("\n\nHere are the existing tasks:\n\n");
    out.push_str(&items);
    out
}

#[cfg(test)]
#[path = "task_reminders.test.rs"]
mod tests;
