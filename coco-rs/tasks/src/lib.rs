//! Task subsystem for coco-rs.
//!
//! This crate owns **three distinct kinds of task state**. They're deliberately
//! separate — a running background shell task, a persistent plan-item stored on
//! disk, and an ephemeral conversation checklist are different things with
//! different lifecycles.
//!
//! | Module | Purpose |
//! |--------|---------|
//! | [`running`] | Running background tasks (shell/agent) with lifecycle events for the SDK NDJSON stream. |
//! | [`task_list`] | Durable plan items, persisted per task-list-id on disk with file locking. Shared across a team. |
//! | [`todos`] | Ephemeral per-agent TodoWrite (V1) checklist. |
//!
//! V1 (TodoWrite) and V2 (Task* tools) are gated by the CLI layer via
//! `is_todo_v2_enabled()` — they're never both active. Running tasks
//! are orthogonal and always on.

mod error;
pub mod handle_impls;
pub mod notification;
pub mod reminder_source;
pub mod running;
pub mod stall;
pub mod task_list;
pub mod todos;

pub use error::Result;
pub use error::TasksError;

// Re-export the canonical surface so callers don't have to pierce the
// module tree for every type.
pub use notification::{
    NoOpNotificationSink, NotificationKind, NotificationSink, NotificationSinkRef,
    TaskNotification, TaskUsage, TerminalStatus, Worktree, render as render_notification,
};
pub use running::KillTaskError;
pub use running::PANEL_GRACE_MS;
pub use running::TaskCreateRequest;
pub use running::TaskManager;
pub use running::TeammateTaskCreateRequest;
pub use running::TeammateTaskUpdate;
pub use stall::{
    STALL_CHECK_INTERVAL_MS, STALL_TAIL_BYTES, STALL_THRESHOLD_MS, matches_interactive_prompt,
};
pub use task_list::ClaimResult;
pub use task_list::Task;
pub use task_list::TaskListStore;
pub use task_list::TaskStatus;
pub use task_list::TaskUpdate;
pub use task_list::resolve_task_list_id;
pub use todos::TodoItem;
pub use todos::TodoStore;
