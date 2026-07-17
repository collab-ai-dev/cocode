# coco-tasks — Crate Plan

TS source: `tasks/` (8 files, 1.1K LOC), `Task.ts`, `utils/task/` (5 files, 1.2K LOC), `utils/plans.ts` (397 LOC), `utils/planModeV2.ts`

## Dependencies

```
coco-tasks depends on:
  - coco-types (TaskId, TaskStatus, TaskStateBase, AgentId)
  - tokio (process spawning, background execution)

coco-tasks does NOT depend on:
  - coco-tools (AgentTool spawns agents via callback, not direct dep)
  - coco-query, coco-inference, any app/ crate
```

## Design Note

Background task execution is a **v1 core primitive**. Both BashTool and AgentTool support
`run_in_background: true`. Coordinator (v2) builds ON TOP of this — no new execution mechanism.

Agents ARE tasks: `LocalAgentTaskState` is one variant of `TaskState`.
AgentTool (coco-tools) creates agents; coco-tasks tracks their lifecycle.
See `crate-coco-tools.md` AgentTool section for spawning architecture.

## Task State (union type — all variants v1 except InProcessTeammate)

```rust
/// Unified task type. Lives in AppState.tasks as HashMap<TaskId, TaskState>.
pub enum TaskState {
    LocalBash(LocalShellTaskState),        // v1: background bash
    LocalAgent(LocalAgentTaskState),       // v1: background/foreground agent
    RemoteAgent(RemoteAgentTaskState),     // v1: remote agent (CCR)
    LocalWorkflow(LocalWorkflowTaskState), // v1: background workflow
    MonitorMcp(MonitorMcpTaskState),       // v1: background MCP monitor
    Dream(DreamTaskState),                 // v1: background memory consolidation
    InProcessTeammate(InProcessTeammateTaskState), // v2: team member
}

// TaskStateBase is defined in coco-types (canonical owner, 11 fields).
// Re-exported here: pub use coco_types::TaskStateBase;
// Fields: id, task_type, status, description, tool_use_id, start_time,
//         end_time, total_paused_ms, output_file, output_offset, notified.

pub struct LocalShellTaskState {
    pub base: TaskStateBase,
    pub command: String,
    pub is_backgrounded: bool,  // false=foreground, true=backgrounded
    pub kind: ShellTaskKind,    // Bash, Monitor
    pub result: Option<ShellTaskResult>,
}

pub struct LocalAgentTaskState {
    pub base: TaskStateBase,
    pub agent_id: AgentId,
    pub prompt: String,
    pub agent_type: AgentTypeId,
    pub model: Option<String>,
    pub is_backgrounded: bool,
    pub pending_messages: Vec<String>,  // Via SendMessage
    pub messages: Option<Vec<Message>>, // Conversation transcript
    pub progress: Option<AgentProgress>,
    pub result: Option<AgentToolResult>,
    pub error: Option<String>,
    pub disk_loaded: bool,              // Sidechain JSONL loaded
    pub evict_after: Option<i64>,       // GC deadline timestamp
}
```

## Background Execution: Three Entry Points

```rust
/// 1. Explicit background (via run_in_background: true parameter):
///    BashTool/AgentTool sets isBackgrounded=true immediately.
///    Returns backgroundTaskId to caller; task runs independently.
///
/// 2. Auto-background (time-based):
///    BashTool: After ASSISTANT_BLOCKING_BUDGET_MS (15s), auto-backgrounds.
///    AgentTool: After autoBackgroundMs (configurable), auto-backgrounds.
///    Transition: foreground → background via backgroundTask().
///
/// 3. User-triggered (Ctrl+B):
///    User presses Ctrl+B while command/agent runs.
///    Calls backgroundAll() which flips isBackgrounded=true for all foreground tasks.
pub fn is_background_task(task: &TaskState) -> bool {
    matches!(task.status(), TaskStatus::Running | TaskStatus::Pending)
        && task.is_backgrounded()
}
```

## Task Output Persistence

```rust
/// Each task gets a unique output file: {session_temp_dir}/tasks/{task_id}.output
/// TaskOutput manages file-mode (bash stdout to file) and pipe-mode (hooks, buffered).
/// Output capped at 5GB (MAX_TASK_OUTPUT_BYTES).
pub struct TaskOutput {
    pub path: PathBuf,
    pub mode: OutputMode,  // File (fd-based), Pipe (buffered)
}

impl TaskOutput {
    pub fn start_polling(&mut self, interval: Duration);
    pub fn stop_polling(&mut self);
    pub fn read(&self, offset: i64) -> String;
}
```

## Task Notification (async completion → main agent)

```rust
/// When a background task completes, it enqueues a notification message
/// formatted as <task-notification> XML. The main agent receives this
/// in the next turn via the steering/attachment injection system (see crate-coco-query.md).
///
/// Format:
/// <task-notification>
///   <task-id>{id}</task-id>
///   <status>completed|failed|killed</status>
///   <summary>{human-readable status}</summary>
///   <result>{final text response}</result>
///   <usage>{token usage}</usage>
/// </task-notification>
pub fn enqueue_task_notification(
    task: &TaskState,
    queue: &CommandQueue,
);
```

## Task Manager

```rust
pub struct TaskManager {
    tasks: HashMap<TaskId, TaskState>,
}

impl TaskManager {
    pub fn spawn_shell(&mut self, input: ShellSpawnInput) -> TaskHandle;
    pub fn spawn_agent(&mut self, input: AgentSpawnInput) -> TaskHandle;
    pub fn get(&self, id: &TaskId) -> Option<&TaskState>;
    pub fn list(&self) -> Vec<&TaskState>;
    pub fn kill(&mut self, id: &TaskId) -> Result<(), TaskError>;
    pub fn read_output(&self, id: &TaskId, offset: i64) -> String;
    /// Transition foreground task to background.
    pub fn background_task(&mut self, id: &TaskId);
    /// Background all foreground tasks (Ctrl+B).
    pub fn background_all(&mut self);
}

pub struct ShellSpawnInput {
    pub command: String,
    pub description: String,
    pub timeout: Option<Duration>,
    pub agent_id: Option<AgentId>,
    pub run_in_background: bool,
}

pub struct AgentSpawnInput {
    pub prompt: String,
    pub agent_type: AgentTypeId,
    pub tools: Option<Vec<String>>,       // Glob patterns, stays String
    pub model: Option<String>,            // Dynamic model ID, stays String
    pub isolation: Option<IsolationMode>,  // None, Worktree
    pub run_in_background: bool,
}
```

## TodoV2 (from `utils/todo/types.ts`)

```rust
/// Lightweight todo tracking (separate from Task system).
/// Used by TodoWriteTool and TaskCreateTool for simple checklists.
pub enum TodoStatus { Pending, InProgress, Completed }

pub struct TodoItem {
    pub content: String,
    pub status: TodoStatus,
    pub active_form: String,  // present continuous for spinner (e.g. "Running tests")
}
```

## Task Dependency Graph (from `utils/tasks.ts`)

```rust
/// Tasks can declare dependencies via blocks/blockedBy arrays.
pub struct TaskDependency {
    pub blocks: Vec<TaskId>,      // task IDs this task blocks
    pub blocked_by: Vec<TaskId>,  // task IDs that block this task
}

/// Dependency resolution:
/// - can_start_task(): checks all blocked_by IDs are completed or deleted
/// - can_complete_task(): same check on blocked_by
/// - On task deletion: removes ID from all blocks/blockedBy arrays in other tasks
/// - Returns { success: false, reason: "blocked", blocked_by_tasks } if blocked
```

## Plan Mode (from `utils/plans.ts` 397 LOC, `utils/planModeV2.ts`)

```rust
/// Plan file management for plan-then-execute workflow.
/// Plans stored at: .claude/plans/{slug}.md
/// Agent-specific: .claude/plans/{slug}-agent-{agent_id}.md
pub struct PlanFileManager;

impl PlanFileManager {
    /// Generate random word slug (adjective-verb-noun pattern, NOT derived from prompt).
    /// Uses generateWordSlug() with up to 10 collision retries against existing files.
    pub fn get_plan_slug() -> String;
    /// Set active plan slug (cached for session).
    pub fn set_plan_slug(slug: &str);
    pub fn clear_plan_slug();
    pub fn clear_all_plan_slugs();
    /// Resolve plan directory: .claude/plans/
    pub fn get_plans_directory(cwd: &Path) -> PathBuf;
    /// Full path: .claude/plans/{slug}.md
    pub fn get_plan_file_path(slug: &str, agent_id: Option<&AgentId>) -> PathBuf;
    /// Read plan content.
    pub fn get_plan(slug: &str) -> Option<String>;
}
```
