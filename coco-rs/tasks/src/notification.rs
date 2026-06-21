//! Push notifications for background-task lifecycle events.
//!
//! This module owns:
//!
//! - The canonical [`TaskNotification`] payload.
//! - The XML envelope renderer ([`render`]).
//! - The [`NotificationSink`] trait that lets app-layer crates plug
//!   in a real producer (e.g. `coco_query::CommandQueue`) without
//!   creating a dependency cycle.
//!
//! ## Why a dedicated module
//!
//! Before this module the XML builder lived in `coco-cli`. That's a
//! layering smell: the envelope shape is task-domain logic, not
//! CLI-bootstrap logic. Moving it here lets `TaskManager` (which
//! owns lifecycle state) call the sink directly — and keeps the
//! producer + the data shape co-located.
//!
//! The trait dependency direction stays clean: `coco-tasks` knows
//! nothing about `coco-query` / `coco-cli`. The app layer
//! implements [`NotificationSink`] in terms of its own machinery
//! (CommandQueue, SDK channels, etc.) and hands an `Arc<dyn
//! NotificationSink>` down at session bootstrap.

use std::sync::Arc;

use async_trait::async_trait;

/// One push-notification produced by a background task lifecycle
/// transition. Carries all fields emitted across shell + agent
/// variants — the [`render`] function picks the right shape.
#[derive(Debug, Clone)]
pub struct TaskNotification {
    /// Task id used in the `<task-id>` tag.
    pub task_id: String,
    /// `tool_use_id` of the tool invocation that spawned the task.
    /// Threaded into `<tool-use-id>` so model can route the
    /// completion back to the original tool call.
    pub tool_use_id: Option<String>,
    /// Agent id of the subagent that spawned the task. Routes the
    /// queued envelope back to that agent's command-queue filter so
    /// teammates only see their own tasks' completions.
    pub agent_id: Option<String>,
    /// On-disk output file path (`<output-file>` tag).
    pub output_file: String,
    /// Human-readable label embedded in the `<summary>` for shell
    /// tasks ("Background command \"$description\" completed") and
    /// agent tasks ("Agent \"$description\" completed").
    pub description: String,
    /// Per-variant payload. Drives the renderer.
    pub kind: NotificationKind,
}

/// Notification variants.
#[derive(Debug, Clone)]
pub enum NotificationKind {
    /// Shell task reached a terminal state. Summary line includes the
    /// `Background command "..."` prefix + exit code.
    ShellTerminal {
        status: TerminalStatus,
        exit_code: Option<i32>,
    },
    /// Agent task reached a terminal state. Envelope carries up to three
    /// optional sections — `<result>`, `<usage>`, `<worktree>`.
    AgentTerminal {
        status: TerminalStatus,
        /// Final response text from the subagent.
        /// Rendered as `<result>${finalMessage}</result>`.
        result: Option<String>,
        /// Token / tool-use / duration block rendered as `<usage>...</usage>`.
        usage: Option<TaskUsage>,
        /// Isolation worktree info rendered as `<worktree>...</worktree>`.
        worktree: Option<Worktree>,
        /// Summary differs per status (`Agent "..." completed` / `failed: ...`
        /// / `was stopped`). `None` triggers the default per-status text in
        /// [`render`].
        error: Option<String>,
    },
    /// Shell output appears frozen on an interactive prompt. Explicitly
    /// forbids `<status>` because print.ts treats `<status>` as a terminal
    /// signal and an unknown value falls through to 'completed', falsely
    /// closing the task for SDK consumers.
    Stall { output_tail: String },
}

/// Three terminal status values (minus the non-terminal `pending` /
/// `running`). Renders as the `<status>` tag content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalStatus {
    Completed,
    Failed,
    Killed,
}

impl TerminalStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Killed => "killed",
        }
    }
}

/// Usage block embedded in agent-task completion notifications.
/// Field shape for agent notification usage.
#[derive(Debug, Clone)]
pub struct TaskUsage {
    pub total_tokens: i64,
    pub tool_uses: i32,
    pub duration_ms: i64,
}

/// Worktree info for agent tasks spawned with `isolation: "worktree"`.
#[derive(Debug, Clone)]
pub struct Worktree {
    pub path: String,
    /// Optional — only emitted when set.
    pub branch: Option<String>,
}

/// Render the XML envelope for shell terminal, stall, and agent terminal
/// (with optional sections) variants.
pub fn render(n: &TaskNotification) -> String {
    match &n.kind {
        NotificationKind::ShellTerminal { status, exit_code } => {
            let mut summary = format!("Background command \"{}\"", n.description);
            match status {
                TerminalStatus::Completed => {
                    summary.push_str(" completed");
                    if let Some(code) = exit_code {
                        summary.push_str(&format!(" (exit code {code})"));
                    }
                }
                TerminalStatus::Failed => {
                    summary.push_str(" failed");
                    if let Some(code) = exit_code {
                        summary.push_str(&format!(" with exit code {code}"));
                    }
                }
                TerminalStatus::Killed => summary.push_str(" was stopped"),
            }
            render_terminal(n, *status, &summary, None, None, None)
        }
        NotificationKind::AgentTerminal {
            status,
            result,
            usage,
            worktree,
            error,
        } => {
            let summary = match status {
                TerminalStatus::Completed => {
                    format!("Agent \"{}\" completed", n.description)
                }
                TerminalStatus::Failed => {
                    let reason = error.as_deref().unwrap_or("Unknown error");
                    format!("Agent \"{}\" failed: {reason}", n.description)
                }
                TerminalStatus::Killed => format!("Agent \"{}\" was stopped", n.description),
            };
            render_terminal(
                n,
                *status,
                &summary,
                result.as_deref(),
                usage.as_ref(),
                worktree.as_ref(),
            )
        }
        NotificationKind::Stall { output_tail } => render_stall(n, output_tail),
    }
}

fn render_terminal(
    n: &TaskNotification,
    status: TerminalStatus,
    summary: &str,
    result: Option<&str>,
    usage: Option<&TaskUsage>,
    worktree: Option<&Worktree>,
) -> String {
    let mut xml = String::with_capacity(384);
    xml.push_str("<task-notification>\n");
    xml.push_str(&format!("<task-id>{}</task-id>\n", n.task_id));
    if let Some(tu) = &n.tool_use_id {
        xml.push_str(&format!("<tool-use-id>{tu}</tool-use-id>\n"));
    }
    xml.push_str(&format!("<output-file>{}</output-file>\n", n.output_file));
    xml.push_str(&format!("<status>{}</status>\n", status.as_str()));
    xml.push_str(&format!("<summary>{}</summary>", escape_xml(summary)));
    if let Some(text) = result {
        xml.push_str(&format!("\n<result>{}</result>", escape_xml(text)));
    }
    if let Some(u) = usage {
        xml.push_str(&format!(
            "\n<usage><total_tokens>{}</total_tokens><tool_uses>{}</tool_uses><duration_ms>{}</duration_ms></usage>",
            u.total_tokens, u.tool_uses, u.duration_ms
        ));
    }
    if let Some(w) = worktree {
        xml.push_str(&format!(
            "\n<worktree><worktreePath>{}</worktreePath>",
            escape_xml(&w.path)
        ));
        if let Some(branch) = &w.branch {
            xml.push_str(&format!(
                "<worktreeBranch>{}</worktreeBranch>",
                escape_xml(branch)
            ));
        }
        xml.push_str("</worktree>");
    }
    xml.push_str("\n</task-notification>");
    xml
}

fn render_stall(n: &TaskNotification, tail: &str) -> String {
    let mut xml = String::with_capacity(512);
    xml.push_str("<task-notification>\n");
    xml.push_str(&format!("<task-id>{}</task-id>\n", n.task_id));
    if let Some(tu) = &n.tool_use_id {
        xml.push_str(&format!("<tool-use-id>{tu}</tool-use-id>\n"));
    }
    xml.push_str(&format!("<output-file>{}</output-file>\n", n.output_file));
    let summary = format!(
        "Background command \"{}\" appears to be waiting for interactive input",
        n.description
    );
    xml.push_str(&format!("<summary>{}</summary>\n", escape_xml(&summary)));
    xml.push_str("</task-notification>\n");
    if !tail.is_empty() {
        xml.push_str("Last output:\n");
        xml.push_str(tail.trim_end());
        xml.push_str(
            "\n\nThe command is likely blocked on an interactive prompt. \
             Kill this task and re-run with piped input (e.g. `echo y | command`) \
             or a non-interactive flag if one exists.",
        );
    }
    xml
}

/// Minimal XML escape for the summary / result / worktree text (5-char set).
fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}

/// Sink that receives a fully-built [`TaskNotification`] and turns
/// it into a side effect in the host application (typically: push
/// onto `coco_query::CommandQueue` so the next agent turn sees it).
///
/// `coco-tasks` knows nothing about the queue; this trait inverts
/// the dependency so the producer (TaskManager) can fire
/// notifications without dragging `coco-query` into the tasks
/// crate's dep set.
#[async_trait]
pub trait NotificationSink: Send + Sync {
    async fn push(&self, notification: TaskNotification);
}

pub type NotificationSinkRef = Arc<dyn NotificationSink>;

/// Default for sessions that don't wire a producer (tests, headless
/// jobs without a turn loop). Notifications are dropped silently.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoOpNotificationSink;

#[async_trait]
impl NotificationSink for NoOpNotificationSink {
    async fn push(&self, _: TaskNotification) {}
}

#[cfg(test)]
#[path = "notification.test.rs"]
mod tests;
