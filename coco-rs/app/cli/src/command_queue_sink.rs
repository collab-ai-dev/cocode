//! Production [`coco_tasks::NotificationSink`] backed by the
//! session-scoped [`coco_query::CommandQueue`].
//!
//! ## Why this lives in app/cli (and not in coco-tasks)
//!
//! `coco-tasks` is layered below `coco-query` (where `CommandQueue`
//! lives). Pushing the sink impl up into `app/cli` keeps the
//! dependency direction acyclic. The producer side
//! (`TaskManager::*` / `TaskRuntime::*`) talks to the trait;
//! `app/cli` is the only place that knows about both the trait and
//! the queue, so the wiring lives here.
//!
//! ## Notification priority
//!
//! Translates [`coco_tasks::TaskNotification`] → [`QueuedCommand`]:
//! defaults priority to `'later'` for terminal events,
//! `'next'` for stalls.

use async_trait::async_trait;
use coco_query::command_queue::{CommandQueue, QueuePriority, QueuedCommand};
use coco_system_reminder::QueueOrigin;
use coco_tasks::{NotificationKind, NotificationSink, TaskNotification, render_notification};
use tracing::{debug, instrument};

/// Wraps a [`CommandQueue`] to satisfy [`NotificationSink`].
#[derive(Clone, Debug)]
pub struct CommandQueueNotificationSink {
    queue: CommandQueue,
}

impl CommandQueueNotificationSink {
    pub fn new(queue: CommandQueue) -> Self {
        Self { queue }
    }
}

#[async_trait]
impl NotificationSink for CommandQueueNotificationSink {
    #[instrument(
        level = "debug",
        skip(self, n),
        fields(task_id = %n.task_id, agent_id = ?n.agent_id, kind = kind_label(&n.kind))
    )]
    async fn push(&self, n: TaskNotification) {
        // Priority by notification kind: every non-stall call uses
        // 'later', stall uses 'next'.
        let priority = match &n.kind {
            NotificationKind::Stall { .. } => QueuePriority::Next,
            NotificationKind::ShellTerminal { .. } | NotificationKind::AgentTerminal { .. } => {
                QueuePriority::Later
            }
        };
        let agent_id = n.agent_id.clone();
        let envelope = render_notification(&n);
        let envelope_bytes = envelope.len();
        let mut cmd =
            QueuedCommand::new(envelope, priority).with_origin(QueueOrigin::TaskNotification);
        if let Some(id) = agent_id {
            cmd = cmd.with_agent(id);
        }
        self.queue.enqueue(cmd).await;
        debug!(
            target: "coco::task_notification",
            envelope_bytes,
            ?priority,
            "enqueued <task-notification>"
        );
    }
}

#[async_trait]
impl coco_hooks::AsyncRewakeSink for CommandQueueNotificationSink {
    async fn enqueue_rewake(&self, _command: String, message: String) {
        let cmd = QueuedCommand::new(message, QueuePriority::Later)
            .with_origin(QueueOrigin::TaskNotification);
        self.queue.enqueue(cmd).await;
        debug!(
            target: "coco::hook_rewake",
            "enqueued asyncRewake task notification"
        );
    }
}

fn kind_label(kind: &NotificationKind) -> &'static str {
    match kind {
        NotificationKind::ShellTerminal { .. } => "shell_terminal",
        NotificationKind::AgentTerminal { .. } => "agent_terminal",
        NotificationKind::Stall { .. } => "stall",
    }
}

#[cfg(test)]
#[path = "command_queue_sink.test.rs"]
mod tests;
