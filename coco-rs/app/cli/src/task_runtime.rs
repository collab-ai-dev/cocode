//! Production background-task runtime.
//!
//! `TaskRuntime` implements four traits that the tool layer + the
//! coordinator both consume:
//!
//! - [`coco_tool_runtime::TaskReader`] — `TaskGet` / `TaskList` /
//!   `TaskOutput` read paths. Implemented in [`reader`].
//! - [`coco_tool_runtime::TaskController`] — `TaskStop`. Implemented
//!   in [`controller`].
//! - [`coco_tool_runtime::ShellTaskSpawner`] — Bash / PowerShell
//!   `run_in_background`. Implemented in [`shell`].
//! - [`coco_tool_runtime::AgentTaskRegistry`] — `SwarmAgentHandle`'s
//!   background AgentTool dispatch. Implemented in [`agent`].
//!
//! ## Where each concern lives
//!
//! - Lifecycle state — [`coco_tasks::TaskManager`].
//! - Disk-backed output — [`crate::disk_task_output::DiskOutputs`].
//! - Per-task cancel + terminal-status broadcast — runtime-only
//!   controls owned by [`coco_tasks::TaskManager`] in the same row
//!   as lifecycle state.
//! - Notification XML construction + push — done in [`agent`] / [`shell`]
//!   using [`coco_tasks::notification`] primitives. The sink
//!   (`Arc<dyn NotificationSink>`) is always wired; tests use
//!   `NoOpNotificationSink` (default), production wires
//!   [`crate::command_queue_sink::CommandQueueNotificationSink`].
//! - Stall watchdog — [`stall::watchdog`] spawned per shell task (no
//!   agent-task watchdog).
//! - Auto-background / auto-detach / progress timers — [`timers`].
//!
mod agent;
mod controller;
mod reader;
mod shell;
mod stall;
mod timers;

use std::sync::Arc;

use coco_tasks::{NoOpNotificationSink, NotificationSinkRef, TaskManager};
use tracing::{debug, info};

use crate::disk_task_output::DiskOutputs;

/// Production task runtime.
///
/// Cheap to clone (every field is `Arc`). Construction happens once
/// per session in CLI bootstrap; the same `Arc<Self>` flows into the
/// engine (read/control) and into `SwarmAgentHandle` (registration).
pub struct TaskRuntime {
    pub(in crate::task_runtime) manager: Arc<TaskManager>,
    pub(in crate::task_runtime) disk: Arc<DiskOutputs>,
    /// Always wired. `NoOpNotificationSink` is the default when no
    /// producer attaches — terminal events are silently dropped,
    /// matching TS sessions that run without a turn loop (headless
    /// jobs / `--bare` SDK). Production attaches the
    /// `CommandQueueNotificationSink`.
    pub(in crate::task_runtime) notification_sink: NotificationSinkRef,
}

impl TaskRuntime {
    /// Test-friendly constructor — temp dir, no-op notification
    /// sink. Production callers use [`Self::with_session_dir`] +
    /// [`Self::with_notification_sink`].
    pub fn new(manager: Arc<TaskManager>) -> Self {
        let temp =
            std::env::temp_dir().join(format!("coco-task-rt-{}", uuid::Uuid::new_v4().simple()));
        Self::with_session_dir(manager, temp)
    }

    /// Production constructor. `session_dir` is the per-session
    /// root for on-disk task output files (typically
    /// `<config_home>/cache/tasks/<session_id>`). Notification sink
    /// defaults to no-op until [`Self::with_notification_sink`]
    /// attaches one.
    pub fn with_session_dir(manager: Arc<TaskManager>, session_dir: std::path::PathBuf) -> Self {
        debug!(
            target: "coco::task_runtime",
            session_dir = %session_dir.display(),
            "constructing TaskRuntime"
        );
        Self {
            manager,
            disk: Arc::new(DiskOutputs::new(session_dir)),
            notification_sink: Arc::new(NoOpNotificationSink),
        }
    }

    /// Attach the notification sink. After this call, every terminal
    /// transition pushes a `<task-notification>` envelope through the
    /// sink.
    pub fn with_notification_sink(mut self, sink: NotificationSinkRef) -> Self {
        info!(
            target: "coco::task_runtime",
            "task-notification sink attached"
        );
        self.notification_sink = sink;
        self
    }

    /// Read access to the inner `TaskManager`.
    pub fn manager(&self) -> &Arc<TaskManager> {
        &self.manager
    }
}

/// Build a [`coco_error::BoxedError`] from a message + status code.
/// Shared across the four trait impls — kept here so each submodule
/// uses the same wrapping. Visible only inside `task_runtime::*`.
pub(in crate::task_runtime) fn boxed_msg(
    msg: impl Into<String>,
    code: coco_error::StatusCode,
) -> coco_error::BoxedError {
    Box::new(coco_error::PlainError::new(msg, code))
}

#[cfg(test)]
#[path = "task_runtime.test.rs"]
mod tests;
