//! Shared DTOs for the persistent task list (V2) and per-agent todos
//! (V1). Lives in `coco-types` so [`crate::app_state::ToolAppState`]
//! can carry typed snapshots without depending on the higher-level
//! handle/implementation crates (`coco-tool-runtime`, `coco-tasks`).

use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;

// ── Task (V2) DTOs ────────────────────────────────────────────────────

/// Task status wire format. **Distinct** from [`crate::TaskStatus`],
/// which is the 6-variant running-task lifecycle enum.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskListStatus {
    Pending,
    InProgress,
    Completed,
}

impl TaskListStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
        }
    }
}

/// A durable plan-item.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRecord {
    pub id: String,
    pub subject: String,
    #[serde(default)]
    pub description: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "activeForm"
    )]
    pub active_form: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    pub status: TaskListStatus,
    #[serde(default)]
    pub blocks: Vec<String>,
    #[serde(default, rename = "blockedBy")]
    pub blocked_by: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, serde_json::Value>>,
}

/// Partial update passed to a task-list handle's `update_task`.
#[derive(Debug, Clone, Default)]
pub struct TaskRecordUpdate {
    pub subject: Option<String>,
    pub description: Option<String>,
    pub active_form: Option<String>,
    pub owner: Option<String>,
    pub status: Option<TaskListStatus>,
    /// Merge these keys into `metadata`; `null` values delete a key.
    pub metadata_merge: Option<HashMap<String, serde_json::Value>>,
}

/// Outcome of a `claim_task` call.
#[derive(Debug, Clone)]
pub enum TaskClaimOutcome {
    Success(TaskRecord),
    TaskNotFound,
    AlreadyClaimed(TaskRecord),
    AlreadyResolved(TaskRecord),
    Blocked {
        task: TaskRecord,
        blocked_by_tasks: Vec<String>,
    },
    AgentBusy {
        task: TaskRecord,
        busy_with_tasks: Vec<String>,
    },
}

// ── Todo (V1) DTOs ────────────────────────────────────────────────────

/// A TodoWrite item (no `id` field, positional identity).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoRecord {
    /// Non-empty content string.
    #[cfg_attr(feature = "schema", schemars(length(min = 1)))]
    pub content: String,
    /// `status` is a plain `String` (TUI/store paths pre-date typing); the
    /// allowed values are declared on the schema.
    #[cfg_attr(feature = "schema", schemars(extend("enum" = ["pending", "in_progress", "completed"])))]
    pub status: String,
    /// Non-empty active form identifier.
    #[serde(rename = "activeForm")]
    #[cfg_attr(feature = "schema", schemars(length(min = 1)))]
    pub active_form: String,
}

// ── UI view state ─────────────────────────────────────────────────────

/// Which panel the TUI should have expanded in the task area.
///
/// **`Teammates` ≠ general subagents.** `Teammates` strictly shows agents
/// with persistent teammate identity (`agentId@teamName`, survives `/clear`,
/// mailbox-based). Async subagents spawned by the `Agent` tool render inline
/// in the transcript and in the `BackgroundTaskStatus` pill row — **not** here.
///
/// A subagent only appears in this view when the Agent tool was invoked with
/// `teamName` set, which routes through `spawnTeammate()` and transforms the
/// worker into a first-class teammate.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpandedView {
    #[default]
    None,
    Tasks,
    Teammates,
}

impl ExpandedView {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Tasks => "tasks",
            Self::Teammates => "teammates",
        }
    }
}
