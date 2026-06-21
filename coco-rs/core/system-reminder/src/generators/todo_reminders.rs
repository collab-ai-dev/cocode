//! `todo_reminder` generator.
//!
//! Gate chain (all must pass):
//!
//! 1. `TodoWrite` tool is present in `ctx.tools`.
//! 2. `Brief` tool is **not** present — when Brief is the primary I/O
//!    channel TodoWrite becomes a side channel and nudging conflicts.
//! 3. `turns_since_last_todo_write >= 10`.
//! 4. `turns_since_last_todo_reminder >= 10`.
//!
//! Content is the reminder body, optionally followed by a bracketed list
//! of the agent's current todos if any exist.

use async_trait::async_trait;
use coco_types::TodoRecord;
use coco_types::ToolName;

use crate::error::Result;
use crate::generator::AttachmentGenerator;
use crate::generator::GeneratorContext;
use crate::types::AttachmentType;
use crate::types::SystemReminder;
use coco_config::SystemReminderConfig;

/// Tools that gate the reminder. The canonical wire strings come from the
/// [`ToolName`] enum's `as_str()`, so renaming a variant flows through
/// automatically and there are no hand-written magic strings here.
const TRIGGER_TOOL: ToolName = ToolName::TodoWrite;
const SUPPRESS_TOOL: ToolName = ToolName::SendUserMessage;

const TURNS_SINCE_WRITE: i32 = 10;
const TURNS_BETWEEN_REMINDERS: i32 = 10;

/// Reminder body (sans trailing `\n` which is injected explicitly).
const TODO_REMINDER_BODY: &str = "The TodoWrite tool hasn't been used recently. If you're working on tasks that would benefit from tracking progress, consider using the TodoWrite tool to track progress. Also consider cleaning up the todo list if has become stale and no longer matches what you are working on. Only use it if it's relevant to the current work. This is just a gentle reminder - ignore if not applicable. Make sure that you NEVER mention this reminder to the user";

/// Nudge the agent to use `TodoWrite` after a long silence.
#[derive(Debug, Default)]
pub struct TodoRemindersGenerator;

#[async_trait]
impl AttachmentGenerator for TodoRemindersGenerator {
    fn name(&self) -> &str {
        "TodoRemindersGenerator"
    }

    fn attachment_type(&self) -> AttachmentType {
        AttachmentType::TodoReminder
    }

    fn is_enabled(&self, config: &SystemReminderConfig) -> bool {
        config.attachments.todo_reminder
    }

    async fn generate(&self, ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        // V2 takes precedence: when V2 is enabled, `TaskRemindersGenerator`
        // owns this turn.
        if ctx.is_task_v2_enabled {
            return Ok(None);
        }

        if !tools_contain(&ctx.tools, TRIGGER_TOOL) {
            return Ok(None);
        }

        if tools_contain(&ctx.tools, SUPPRESS_TOOL) {
            return Ok(None);
        }

        if ctx.turns_since_last_todo_write < TURNS_SINCE_WRITE
            || ctx.turns_since_last_todo_reminder < TURNS_BETWEEN_REMINDERS
        {
            return Ok(None);
        }

        let content = render_todo_reminder_body(&ctx.todos);
        Ok(Some(SystemReminder::new(
            AttachmentType::TodoReminder,
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

fn render_todo_reminder_body(todos: &[TodoRecord]) -> String {
    // Base body always ends with `\n`; the optional list suffix adds
    // `\n\n` (3 newlines total before "Here").
    let mut out = format!("{TODO_REMINDER_BODY}\n");
    if todos.is_empty() {
        return out;
    }
    // `${index + 1}. [${status}] ${content}` joined by `\n`, wrapped in
    // `[…]`. Preserve the outer brackets exactly — they're user-visible.
    let items = todos
        .iter()
        .enumerate()
        .map(|(i, t)| format!("{n}. [{s}] {c}", n = i + 1, s = t.status, c = t.content))
        .collect::<Vec<_>>()
        .join("\n");
    out.push_str("\n\nHere are the existing contents of your todo list:\n\n[");
    out.push_str(&items);
    out.push(']');
    out
}

#[cfg(test)]
#[path = "todo_reminders.test.rs"]
mod tests;
