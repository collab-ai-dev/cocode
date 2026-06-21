//! Implementations of the seven task/todo tools, all on top of the
//! shared `TaskListHandle` + `TodoListHandle` injected through
//! `ToolUseContext`.
//!
//! Output projections use stable JSON envelopes (like a `task` wrapper
//! or a `tasks` array) so the model sees consistent payloads.

use coco_messages::ToolResult;
use coco_tool_runtime::DescriptionOptions;
use coco_tool_runtime::MailboxEnvelope;
use coco_tool_runtime::PromptOptions;
use coco_tool_runtime::TaskListHandleRef;
use coco_tool_runtime::TaskListStatus;
use coco_tool_runtime::TaskRecord;
use coco_tool_runtime::TaskRecordUpdate;
use coco_tool_runtime::TodoListHandleRef;
use coco_tool_runtime::TodoRecord;
use coco_tool_runtime::Tool;
use coco_tool_runtime::ToolError;
use coco_tool_runtime::ToolResultContentPart;
use coco_tool_runtime::ToolUseContext;
use coco_types::AppStatePatch;
use coco_types::ExpandedView;
use coco_types::Feature;
use coco_types::ToolId;
use coco_types::ToolName;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

// ── Output projections ────────────────────────────────────────────────

/// Shape a freshly-created task for TaskCreate's `data`.
/// Returns `{task: {id, subject}}`.
fn project_create(task: &TaskRecord) -> Value {
    serde_json::json!({
        "task": { "id": task.id, "subject": task.subject }
    })
}

/// Returns `{task: {id, subject, description, status, blocks, blockedBy} | null}`.
fn project_get(task: Option<&TaskRecord>) -> Value {
    match task {
        Some(t) => serde_json::json!({
            "task": {
                "id": t.id,
                "subject": t.subject,
                "description": t.description,
                "status": t.status.as_str(),
                "blocks": t.blocks,
                "blockedBy": t.blocked_by,
            }
        }),
        None => serde_json::json!({ "task": null }),
    }
}

/// One TaskList entry: 4-5 fields (id, subject, status, blockedBy,
/// owner?). Completed tasks resolve blockers out of blockedBy.
fn project_list_entry(
    task: &TaskRecord,
    resolved_ids: &std::collections::HashSet<String>,
) -> Value {
    let filtered_blocked_by: Vec<&String> = task
        .blocked_by
        .iter()
        .filter(|id| !resolved_ids.contains(*id))
        .collect();
    let mut obj = serde_json::json!({
        "id": task.id,
        "subject": task.subject,
        "status": task.status.as_str(),
        "blockedBy": filtered_blocked_by,
    });
    if let Some(owner) = &task.owner {
        obj["owner"] = Value::String(owner.clone());
    }
    obj
}

fn is_internal_task(task: &TaskRecord) -> bool {
    task.metadata
        .as_ref()
        .is_some_and(|m| m.contains_key("_internal"))
}

fn project_update(
    task_id: &str,
    updated_fields: Vec<&'static str>,
    status_change: Option<(String, String)>,
    verification_nudge: bool,
    completed_nudge: bool,
) -> Value {
    let mut out = serde_json::json!({
        "success": true,
        "taskId": task_id,
        "updatedFields": updated_fields,
        "verificationNudgeNeeded": verification_nudge,
        "completedNudgeNeeded": completed_nudge,
    });
    if let Some((from, to)) = status_change {
        out["statusChange"] = serde_json::json!({ "from": from, "to": to });
    }
    out
}

fn project_update_error(task_id: &str, error: &str) -> Value {
    serde_json::json!({
        "success": false,
        "taskId": task_id,
        "updatedFields": [],
        "error": error,
    })
}

#[derive(Debug, Clone, Copy)]
enum RetrievalStatus {
    Success,
    Timeout,
    NotReady,
}

impl RetrievalStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Timeout => "timeout",
            Self::NotReady => "not_ready",
        }
    }
}

fn project_output_background(
    task_id: &str,
    task_status: &str,
    output: String,
    exit_code: Option<i32>,
    status: RetrievalStatus,
) -> Value {
    let mut task_obj = serde_json::json!({
        "task_id": task_id,
        "task_type": "background",
        "status": task_status,
        "description": "",
        "output": output,
    });
    if let Some(code) = exit_code {
        task_obj["exitCode"] = Value::Number(serde_json::Number::from(code));
    }
    serde_json::json!({
        "retrieval_status": status.as_str(),
        "task": task_obj,
    })
}

// ── app_state patch helpers ───────────────────────────────────────────

/// Snapshot the current plan-task list (filtered to model-visible
/// entries) and return a patch that:
/// 1. Stores the snapshot in `ToolAppState.plan_tasks`
/// 2. Sets `expanded_view = Tasks` (auto-expand)
/// 3. Updates `verification_nudge_pending`
///
/// The snapshot is computed *now* and moved into the closure. The
/// executor applies the patch post-execute under a single write lock.
async fn build_task_list_patch(
    task_list: &TaskListHandleRef,
    verification_nudge: bool,
) -> AppStatePatch {
    let mut visible = task_list.list_tasks().await.unwrap_or_default();
    visible.retain(|t| {
        !t.metadata
            .as_ref()
            .is_some_and(|m| m.contains_key("_internal"))
    });
    Box::new(move |state: &mut coco_types::ToolAppState| {
        state.plan_tasks = visible;
        state.expanded_view = ExpandedView::Tasks;
        if verification_nudge {
            state.verification_nudge_pending = true;
        }
    })
}

/// After TodoWrite, snapshot the store for `key` and patch AppState.
/// Updates the shared snapshot so the TUI can render V1 lists.
async fn build_todo_patch(
    todo_list: &TodoListHandleRef,
    key: String,
    verification_nudge: bool,
) -> AppStatePatch {
    let items = todo_list.read(&key).await;
    Box::new(move |state: &mut coco_types::ToolAppState| {
        if items.is_empty() {
            state.todos_by_agent.remove(&key);
        } else {
            state.todos_by_agent.insert(key, items);
        }
        if verification_nudge {
            state.verification_nudge_pending = true;
        }
    })
}

/// Key under which a TodoWrite list is stored in `TodoListHandle`.
///
/// Uses `context.agentId` when present, falling back to `session_id_for_history`
/// (passed to `ToolUseContext` at bootstrap). As a last resort (tests without
/// either), uses a stable literal so list-local operations remain round-trippable.
fn todo_key(ctx: &ToolUseContext) -> String {
    ctx.agent_id
        .as_ref()
        .map(ToString::to_string)
        .or_else(|| ctx.session_id_for_history.clone())
        .unwrap_or_else(|| "main-session".to_string())
}

const TASK_CREATE_DESCRIPTION: &str = "Create a new task in the task list";
const TASK_CREATE_SEARCH_HINT: &str = "create a task in the task list";

fn task_create_prompt(agent_teams_available: bool) -> String {
    let teammate_context = if agent_teams_available {
        " and potentially assigned to teammates"
    } else {
        ""
    };
    let teammate_tips = if agent_teams_available {
        "- Include enough detail in the description for another agent to understand and complete the task
- New tasks are created with status 'pending' and no owner - use TaskUpdate with the `owner` parameter to assign them
"
    } else {
        ""
    };

    format!(
        "Use this tool to create a structured task list for your current coding session. This helps you track progress, organize complex tasks, and demonstrate thoroughness to the user.
It also helps the user understand the progress of the task and overall progress of their requests.

## When to Use This Tool

Use this tool proactively in these scenarios:

- Complex multi-step tasks - When a task requires 3 or more distinct steps or actions
- Non-trivial and complex tasks - Tasks that require careful planning or multiple operations{teammate_context}
- Plan mode - When using plan mode, create a task list to track the work
- User explicitly requests todo list - When the user directly asks you to use the todo list
- User provides multiple tasks - When users provide a list of things to be done (numbered or comma-separated)
- After receiving new instructions - Immediately capture user requirements as tasks
- When you start working on a task - Mark it as in_progress BEFORE beginning work
- After completing a task - Mark it as completed and add any new follow-up tasks discovered during implementation

## When NOT to Use This Tool

Skip using this tool when:
- There is only a single, straightforward task
- The task is trivial and tracking it provides no organizational benefit
- The task can be completed in less than 3 trivial steps
- The task is purely conversational or informational

NOTE that you should not use this tool if there is only one trivial task to do. In this case you are better off just doing the task directly.

## Task Fields

- **subject**: A brief, actionable title in imperative form (e.g., \"Fix authentication bug in login flow\")
- **description**: What needs to be done
- **activeForm** (optional): Present continuous form shown in the spinner when the task is in_progress (e.g., \"Fixing authentication bug\"). If omitted, the spinner shows the subject instead.

All tasks are created with status `pending`.

## Tips

- Create tasks with clear, specific subjects that describe the outcome
- After creating tasks, use TaskUpdate to set up dependencies (blocks/blockedBy) if needed
{teammate_tips}- Check TaskList first to avoid creating duplicate tasks
"
    )
}

const TASK_GET_DESCRIPTION: &str = "Get a task by ID from the task list";
const TASK_GET_SEARCH_HINT: &str = "retrieve a task by ID";
const TASK_GET_PROMPT: &str = "Use this tool to retrieve a task by its ID from the task list.

## When to Use This Tool

- When you need the full description and context before starting work on a task
- To understand task dependencies (what it blocks, what blocks it)
- After being assigned a task, to get complete requirements

## Output

Returns full task details:
- **subject**: Task title
- **description**: Detailed requirements and context
- **status**: 'pending', 'in_progress', or 'completed'
- **blocks**: Tasks waiting on this one to complete
- **blockedBy**: Tasks that must complete before this one can start

## Tips

- After fetching a task, verify its blockedBy list is empty before beginning work.
- Use TaskList to see all tasks in summary form.
";

const TASK_LIST_DESCRIPTION: &str = "List all tasks in the task list";
const TASK_LIST_SEARCH_HINT: &str = "list all tasks";

fn task_list_prompt(agent_teams_available: bool) -> String {
    let teammate_use_case = if agent_teams_available {
        "- Before assigning tasks to teammates, to see what's available
"
    } else {
        ""
    };
    let id_description = "- **id**: Task identifier (use with TaskGet, TaskUpdate)";
    let teammate_workflow = if agent_teams_available {
        "
## Teammate Workflow

When working as a teammate:
1. After completing your current task, call TaskList to find available work
2. Look for tasks with status 'pending', no owner, and empty blockedBy
3. **Prefer tasks in ID order** (lowest ID first) when multiple tasks are available, as earlier tasks often set up context for later ones
4. Claim an available task using TaskUpdate (set `owner` to your name), or wait for leader assignment
5. If blocked, focus on unblocking tasks or notify the team lead
"
    } else {
        ""
    };

    format!(
        "Use this tool to list all tasks in the task list.

## When to Use This Tool

- To see what tasks are available to work on (status: 'pending', no owner, not blocked)
- To check overall progress on the project
- To find tasks that are blocked and need dependencies resolved
{teammate_use_case}- After completing a task, to check for newly unblocked work or claim the next available task
- **Prefer working on tasks in ID order** (lowest ID first) when multiple tasks are available, as earlier tasks often set up context for later ones

## Output

Returns a summary of each task:
{id_description}
- **subject**: Brief description of the task
- **status**: 'pending', 'in_progress', or 'completed'
- **owner**: Agent ID if assigned, empty if available
- **blockedBy**: List of open task IDs that must be resolved first (tasks with blockedBy cannot be claimed until dependencies resolve)

Use TaskGet with a specific task ID to view full details including description and comments.
{teammate_workflow}"
    )
}

const TASK_UPDATE_DESCRIPTION: &str = "Update a task in the task list";
const TASK_UPDATE_SEARCH_HINT: &str = "update a task";
const TASK_UPDATE_PROMPT: &str = "Use this tool to update a task in the task list.

## When to Use This Tool

**Mark tasks as resolved:**
- When you have completed the work described in a task
- When a task is no longer needed or has been superseded
- IMPORTANT: Always mark your assigned tasks as resolved when you finish them
- After resolving, call TaskList to find your next task

- ONLY mark a task as completed when you have FULLY accomplished it
- If you encounter errors, blockers, or cannot finish, keep the task as in_progress
- When blocked, create a new task describing what needs to be resolved
- Never mark a task as completed if:
  - Tests are failing
  - Implementation is partial
  - You encountered unresolved errors
  - You couldn't find necessary files or dependencies

**Delete tasks:**
- When a task is no longer relevant or was created in error
- Setting status to `deleted` permanently removes the task

**Update task details:**
- When requirements change or become clearer
- When establishing dependencies between tasks

## Fields You Can Update

- **status**: The task status (see Status Workflow below)
- **subject**: Change the task title (imperative form, e.g., \"Run tests\")
- **description**: Change the task description
- **activeForm**: Present continuous form shown in spinner when in_progress (e.g., \"Running tests\")
- **owner**: Change the task owner (agent name)
- **metadata**: Merge metadata keys into the task (set a key to null to delete it)
- **addBlocks**: Mark tasks that cannot start until this one completes
- **addBlockedBy**: Mark tasks that must complete before this one can start

## Status Workflow

Status progresses: `pending` → `in_progress` → `completed`

Use `deleted` to permanently remove a task.

## Staleness

Make sure to read a task's latest state using `TaskGet` before updating it.

## Examples

Mark task as in progress when starting work:
```json
{\"taskId\": \"1\", \"status\": \"in_progress\"}
```

Mark task as completed after finishing work:
```json
{\"taskId\": \"1\", \"status\": \"completed\"}
```

Delete a task:
```json
{\"taskId\": \"1\", \"status\": \"deleted\"}
```

Claim a task by setting owner:
```json
{\"taskId\": \"1\", \"owner\": \"my-name\"}
```

Set up task dependencies:
```json
{\"taskId\": \"2\", \"addBlockedBy\": [\"1\"]}
```
";

// ── TaskCreateTool ────────────────────────────────────────────────────

/// Typed input for [`TaskCreateTool`].
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct TaskCreateInput {
    /// Task subject/title
    pub subject: String,
    /// Detailed task description
    pub description: String,
    /// Present continuous form shown in spinner when in_progress
    /// (e.g., 'Running tests')
    #[serde(default, rename = "activeForm")]
    pub active_form: Option<String>,
    /// Arbitrary metadata to attach to the task
    #[serde(default)]
    pub metadata: Option<HashMap<String, Value>>,
}

pub struct TaskCreateTool;

#[async_trait::async_trait]
impl Tool for TaskCreateTool {
    type Input = TaskCreateInput;
    coco_tool_runtime::impl_runtime_schema!(TaskCreateInput);
    /// Output is a `{task: {...}}` envelope built by
    /// `project_create`. Kept as `Value` because the projection helper
    /// is shared across the 7 task tools and they share field-shape
    /// invariants the renderer reads positionally.
    type Output = Value;

    fn to_auto_classifier_input(&self, input: &TaskCreateInput) -> Option<String> {
        Some(input.subject.clone())
    }

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::TaskCreate)
    }
    fn name(&self) -> &str {
        ToolName::TaskCreate.as_str()
    }
    fn description(&self, _input: &TaskCreateInput, _options: &DescriptionOptions) -> String {
        TASK_CREATE_DESCRIPTION.into()
    }
    async fn prompt(&self, options: &PromptOptions) -> String {
        task_create_prompt(options.agent_teams_available)
    }

    fn is_enabled(&self, ctx: &ToolUseContext) -> bool {
        ctx.features.enabled(Feature::TaskV2)
    }

    fn is_concurrency_safe(&self, _input: &TaskCreateInput) -> bool {
        true
    }
    // Defer policy for the Task*/Todo family is set explicitly here (not via
    // the trait default) so it stays greppable. Deliberate divergence from the
    // TS reference, which defers all of these: coco-rs keeps the high-frequency
    // plan/todo tools (Create/Get/List/Update/Stop, TodoWrite) always-loaded so
    // weak non-Anthropic models don't need a ToolSearch round-trip before their
    // first call. Only the deprecated TaskOutput defers.
    fn should_defer(&self) -> bool {
        false
    }
    fn search_hint(&self) -> Option<&str> {
        Some(TASK_CREATE_SEARCH_HINT)
    }

    /// Render the create envelope as `Task #{id} created successfully: {subject}`.
    fn render_for_model(&self, data: &Value) -> Vec<ToolResultContentPart> {
        let task = data.get("task");
        let id = task
            .and_then(|t| t.get("id"))
            .and_then(Value::as_str)
            .unwrap_or("?");
        let subject = task
            .and_then(|t| t.get("subject"))
            .and_then(Value::as_str)
            .unwrap_or("");
        vec![ToolResultContentPart::Text {
            text: format!("Task #{id} created successfully: {subject}"),
            provider_options: None,
        }]
    }

    async fn execute(
        &self,
        input: TaskCreateInput,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<Value>, ToolError> {
        let subject = input.subject;
        let description = input.description;
        let active_form = input.active_form;
        let metadata = input.metadata;

        let task = ctx
            .task_list
            .create_task(subject.clone(), description.clone(), active_form, metadata)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("task_list.create_task failed: {e}"),
                display_data: None,
                source: None,
            })?;

        // Fire TaskCreated hooks AFTER the task is persisted. A blocking
        // hook rolls the task back so the model sees the failure and the
        // store stays consistent.
        if let Some(handle) = ctx.hook_handle.as_ref() {
            let outcome = handle
                .run_task_created(
                    &task.id,
                    &subject,
                    if description.is_empty() {
                        None
                    } else {
                        Some(description.as_str())
                    },
                    /*teammate_name*/ None,
                    /*team_name*/ None,
                )
                .await;
            if let Some(reason) = outcome.blocking_reason {
                let _ = ctx.task_list.delete_task(&task.id).await;
                return Err(ToolError::ExecutionFailed {
                    message: format!("TaskCreated hook feedback:\n{reason}"),
                    display_data: None,
                    source: None,
                });
            }
        }

        // Auto-expand the task panel.
        let patch = build_task_list_patch(&ctx.task_list, false).await;
        Ok(ToolResult {
            data: project_create(&task),
            new_messages: vec![],
            app_state_patch: Some(patch),
            permission_updates: Vec::new(),
            display_data: None,
        })
    }
}

// ── TaskGetTool ───────────────────────────────────────────────────────

/// Typed input for [`TaskGetTool`]. Wire key is `taskId` (camelCase).
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct TaskGetInput {
    /// The task ID to look up
    #[serde(rename = "taskId")]
    pub task_id: String,
}

pub struct TaskGetTool;

#[async_trait::async_trait]
impl Tool for TaskGetTool {
    type Input = TaskGetInput;
    coco_tool_runtime::impl_runtime_schema!(TaskGetInput);
    type Output = Value;

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::TaskGet)
    }
    fn name(&self) -> &str {
        ToolName::TaskGet.as_str()
    }
    fn description(&self, _input: &TaskGetInput, _options: &DescriptionOptions) -> String {
        TASK_GET_DESCRIPTION.into()
    }
    async fn prompt(&self, _options: &PromptOptions) -> String {
        TASK_GET_PROMPT.into()
    }
    fn is_enabled(&self, ctx: &ToolUseContext) -> bool {
        ctx.features.enabled(Feature::TaskV2)
    }
    fn is_read_only(&self, _input: &TaskGetInput) -> bool {
        true
    }
    fn is_always_read_only(&self) -> bool {
        true
    }
    fn is_concurrency_safe(&self, _input: &TaskGetInput) -> bool {
        true
    }
    fn should_defer(&self) -> bool {
        false
    }
    fn search_hint(&self) -> Option<&str> {
        Some(TASK_GET_SEARCH_HINT)
    }

    /// Render the task envelope as a multi-line text block, or
    /// "Task not found" when `task` is null.
    fn render_for_model(&self, data: &Value) -> Vec<ToolResultContentPart> {
        let task = data.get("task");
        let text = match task {
            Some(t) if !t.is_null() => format_task_full(t),
            _ => "Task not found".to_string(),
        };
        vec![ToolResultContentPart::Text {
            text,
            provider_options: None,
        }]
    }

    async fn execute(
        &self,
        input: TaskGetInput,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<Value>, ToolError> {
        let task_id = input.task_id;
        if task_id.is_empty() {
            return Err(ToolError::InvalidInput {
                message: "taskId parameter is required".into(),
                error_code: None,
            });
        }
        let task =
            ctx.task_list
                .get_task(&task_id)
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    message: format!("task_list.get_task failed: {e}"),
                    display_data: None,
                    source: None,
                })?;
        Ok(ToolResult {
            data: project_get(task.as_ref()),
            new_messages: vec![],
            app_state_patch: None,
            permission_updates: Vec::new(),
            display_data: None,
        })
    }
}

/// Render a task object into the multi-line TaskGet text block.
/// Uses `#` prefix on every id, and always includes the Description
/// line (never conditionally suppressed on empty).
fn format_task_full(task: &Value) -> String {
    let id = task.get("id").and_then(Value::as_str).unwrap_or("?");
    let subject = task.get("subject").and_then(Value::as_str).unwrap_or("");
    let status = task.get("status").and_then(Value::as_str).unwrap_or("?");
    let description = task
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("");
    let mut out = format!("Task #{id}: {subject}\nStatus: {status}\nDescription: {description}");
    if let Some(arr) = task.get("blockedBy").and_then(Value::as_array)
        && !arr.is_empty()
    {
        let ids: Vec<String> = arr
            .iter()
            .filter_map(|v| v.as_str().map(|s| format!("#{s}")))
            .collect();
        out.push_str(&format!("\nBlocked by: {}", ids.join(", ")));
    }
    if let Some(arr) = task.get("blocks").and_then(Value::as_array)
        && !arr.is_empty()
    {
        let ids: Vec<String> = arr
            .iter()
            .filter_map(|v| v.as_str().map(|s| format!("#{s}")))
            .collect();
        out.push_str(&format!("\nBlocks: {}", ids.join(", ")));
    }
    out
}

// ── TaskListTool ──────────────────────────────────────────────────────

/// Typed input for [`TaskListTool`] — no parameters.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct TaskListInput {}

pub struct TaskListTool;

#[async_trait::async_trait]
impl Tool for TaskListTool {
    type Input = TaskListInput;
    coco_tool_runtime::impl_runtime_schema!(TaskListInput);
    type Output = Value;

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::TaskList)
    }
    fn name(&self) -> &str {
        ToolName::TaskList.as_str()
    }
    fn description(&self, _input: &TaskListInput, _options: &DescriptionOptions) -> String {
        TASK_LIST_DESCRIPTION.into()
    }
    async fn prompt(&self, options: &PromptOptions) -> String {
        task_list_prompt(options.agent_teams_available)
    }
    fn is_enabled(&self, ctx: &ToolUseContext) -> bool {
        ctx.features.enabled(Feature::TaskV2)
    }
    fn is_read_only(&self, _input: &TaskListInput) -> bool {
        true
    }
    fn is_always_read_only(&self) -> bool {
        true
    }
    fn is_concurrency_safe(&self, _input: &TaskListInput) -> bool {
        true
    }
    fn should_defer(&self) -> bool {
        false
    }
    fn search_hint(&self) -> Option<&str> {
        Some(TASK_LIST_SEARCH_HINT)
    }

    /// Render `{tasks: [...]}` as `#{id} [{status}] {subject}{owner}{blocked}`
    /// per line. Empty list collapses to "No tasks found".
    fn render_for_model(&self, data: &Value) -> Vec<ToolResultContentPart> {
        let tasks = data.get("tasks").and_then(Value::as_array);
        let text = match tasks {
            Some(arr) if arr.is_empty() => "No tasks found".to_string(),
            Some(arr) => arr
                .iter()
                .map(|t| {
                    let id = t.get("id").and_then(Value::as_str).unwrap_or("?");
                    let subject = t.get("subject").and_then(Value::as_str).unwrap_or("");
                    let status = t.get("status").and_then(Value::as_str).unwrap_or("?");
                    let owner = t
                        .get("owner")
                        .and_then(Value::as_str)
                        .map(|o| format!(" ({o})"))
                        .unwrap_or_default();
                    let blocked = t
                        .get("blockedBy")
                        .and_then(Value::as_array)
                        .filter(|a| !a.is_empty())
                        .map(|arr| {
                            let ids: Vec<String> = arr
                                .iter()
                                .filter_map(|v| v.as_str().map(|s| format!("#{s}")))
                                .collect();
                            format!(" [blocked by {}]", ids.join(", "))
                        })
                        .unwrap_or_default();
                    format!("#{id} [{status}] {subject}{owner}{blocked}")
                })
                .collect::<Vec<_>>()
                .join("\n"),
            None => serde_json::to_string(data).unwrap_or_default(),
        };
        vec![ToolResultContentPart::Text {
            text,
            provider_options: None,
        }]
    }

    async fn execute(
        &self,
        _input: TaskListInput,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<Value>, ToolError> {
        let all = ctx
            .task_list
            .list_tasks()
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("task_list.list_tasks failed: {e}"),
                display_data: None,
                source: None,
            })?;

        // Any completed task id is removed from other tasks' `blockedBy`
        // so the model only sees unresolved blockers.
        let resolved_ids: std::collections::HashSet<String> = all
            .iter()
            .filter(|t| t.status == TaskListStatus::Completed)
            .map(|t| t.id.clone())
            .collect();

        let tasks: Vec<Value> = all
            .iter()
            .filter(|t| !is_internal_task(t))
            .map(|t| project_list_entry(t, &resolved_ids))
            .collect();

        Ok(ToolResult {
            data: serde_json::json!({ "tasks": tasks }),
            new_messages: vec![],
            app_state_patch: None,
            permission_updates: Vec::new(),
            display_data: None,
        })
    }
}

// ── TaskUpdateTool ────────────────────────────────────────────────────

/// Wire status values for [`TaskUpdateInput`].
/// `deleted` routes to a task deletion in `execute`, not a persistent status set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskUpdateStatus {
    Pending,
    InProgress,
    Completed,
    Deleted,
}

impl TaskUpdateStatus {
    /// The persistent-store status, or `None` for `Deleted` (routed to a
    /// delete, not a status set).
    fn to_list_status(self) -> Option<TaskListStatus> {
        match self {
            Self::Pending => Some(TaskListStatus::Pending),
            Self::InProgress => Some(TaskListStatus::InProgress),
            Self::Completed => Some(TaskListStatus::Completed),
            Self::Deleted => None,
        }
    }
}

/// Typed input for [`TaskUpdateTool`]. Wire keys are camelCase
/// (`taskId`, `activeForm`, `addBlocks`, `addBlockedBy`).
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct TaskUpdateInput {
    /// The task ID to update
    #[serde(rename = "taskId")]
    pub task_id: String,
    /// New status for the task — `deleted` routes to a delete (not a
    /// status set).
    #[serde(default)]
    pub status: Option<TaskUpdateStatus>,
    /// New subject for the task
    #[serde(default)]
    pub subject: Option<String>,
    /// New description
    #[serde(default)]
    pub description: Option<String>,
    /// Present continuous form for spinner
    #[serde(default, rename = "activeForm")]
    pub active_form: Option<String>,
    /// New owner (agent name)
    #[serde(default)]
    pub owner: Option<String>,
    /// Task IDs that cannot start until this one completes
    #[serde(default, rename = "addBlocks")]
    pub add_blocks: Option<Vec<String>>,
    /// Task IDs that must complete before this one can start
    #[serde(default, rename = "addBlockedBy")]
    pub add_blocked_by: Option<Vec<String>>,
    /// Metadata keys to merge (set key to null to delete)
    #[serde(default)]
    pub metadata: Option<HashMap<String, Value>>,
}

pub struct TaskUpdateTool;

#[async_trait::async_trait]
impl Tool for TaskUpdateTool {
    type Input = TaskUpdateInput;
    // `status` is the typed [`TaskUpdateStatus`] enum
    // (enum[pending, in_progress, completed, deleted]), so the derived
    // schema carries the enum and is auto-closed — no hand-patching.
    coco_tool_runtime::impl_runtime_schema!(TaskUpdateInput);
    type Output = Value;

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::TaskUpdate)
    }
    fn name(&self) -> &str {
        ToolName::TaskUpdate.as_str()
    }
    fn description(&self, _input: &TaskUpdateInput, _options: &DescriptionOptions) -> String {
        TASK_UPDATE_DESCRIPTION.into()
    }
    async fn prompt(&self, _options: &PromptOptions) -> String {
        TASK_UPDATE_PROMPT.into()
    }

    fn is_enabled(&self, ctx: &ToolUseContext) -> bool {
        ctx.features.enabled(Feature::TaskV2)
    }

    fn is_concurrency_safe(&self, _input: &TaskUpdateInput) -> bool {
        true
    }
    fn should_defer(&self) -> bool {
        false
    }
    fn search_hint(&self) -> Option<&str> {
        Some(TASK_UPDATE_SEARCH_HINT)
    }

    /// Render the update envelope.
    /// Error: surface the `error` field directly (or `Task #{id} not
    /// found` fallback). Success: `Updated task #{id} {fields...}` with
    /// optional teammate-completion + verification nudges.
    fn render_for_model(&self, data: &Value) -> Vec<ToolResultContentPart> {
        let success = data
            .get("success")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let task_id = data.get("taskId").and_then(Value::as_str).unwrap_or("?");
        let text = if success {
            let updated_fields: Vec<&str> = data
                .get("updatedFields")
                .and_then(Value::as_array)
                .map(|arr| arr.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let mut out = format!("Updated task #{task_id} {}", updated_fields.join(", "));
            // Teammate-completion nudge fires before the verification
            // nudge when both would apply.
            if data
                .get("completedNudgeNeeded")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                out.push_str("\n\nTask completed. Call TaskList now to find your next available task or see if your work unblocked others.");
            }
            if data
                .get("verificationNudgeNeeded")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                out.push_str("\n\nNOTE: You just closed out 3+ tasks and none of them was a verification step. Before writing your final summary, spawn the verification agent (subagent_type=\"verification-agent\"). You cannot self-assign PARTIAL by listing caveats in your summary — only the verifier issues a verdict.");
            }
            out
        } else {
            data.get("error")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("Task #{task_id} not found"))
        };
        vec![ToolResultContentPart::Text {
            text,
            provider_options: None,
        }]
    }

    async fn execute(
        &self,
        input: TaskUpdateInput,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<Value>, ToolError> {
        let task_id = input.task_id.clone();
        if task_id.is_empty() {
            return Err(ToolError::InvalidInput {
                message: "taskId parameter is required".into(),
                error_code: None,
            });
        }

        // Fetch existing — returns `{success: false, error}` when the
        // task is missing rather than erroring out, so the model can
        // handle it gracefully.
        let existing = match ctx.task_list.get_task(&task_id).await {
            Ok(Some(t)) => t,
            Ok(None) => {
                return Ok(ToolResult {
                    data: project_update_error(&task_id, "Task not found"),
                    new_messages: vec![],
                    app_state_patch: None,
                    permission_updates: Vec::new(),
                    display_data: None,
                });
            }
            Err(e) => {
                return Err(ToolError::ExecutionFailed {
                    message: format!("task_list.get_task failed: {e}"),
                    display_data: None,
                    source: None,
                });
            }
        };

        let mut updated_fields: Vec<&'static str> = Vec::new();

        // ── Handle `status=deleted` — delete the task and return early.
        if input.status == Some(TaskUpdateStatus::Deleted) {
            let deleted = ctx.task_list.delete_task(&task_id).await.map_err(|e| {
                ToolError::ExecutionFailed {
                    message: format!("task_list.delete_task failed: {e}"),
                    display_data: None,
                    source: None,
                }
            })?;
            let status_change = if deleted {
                Some((existing.status.as_str().to_string(), "deleted".into()))
            } else {
                None
            };
            let data = if deleted {
                let mut out =
                    project_update(&task_id, vec!["deleted"], status_change, false, false);
                out["success"] = Value::Bool(true);
                out
            } else {
                project_update_error(&task_id, "Failed to delete task")
            };
            let patch = build_task_list_patch(&ctx.task_list, false).await;
            return Ok(ToolResult {
                data,
                new_messages: vec![],
                app_state_patch: Some(patch),
                permission_updates: Vec::new(),
                display_data: None,
            });
        }

        // ── Regular updates ──────────────────────────────────────────
        let mut update = TaskRecordUpdate::default();
        let mut status_change: Option<(String, String)> = None;
        let mut newly_completed = false;

        if let Some(s) = input.subject.as_deref()
            && s != existing.subject
        {
            update.subject = Some(s.to_string());
            updated_fields.push("subject");
        }
        if let Some(d) = input.description.as_deref()
            && d != existing.description
        {
            update.description = Some(d.to_string());
            updated_fields.push("description");
        }
        if let Some(af) = input.active_form.as_deref()
            && Some(af) != existing.active_form.as_deref()
        {
            update.active_form = Some(af.to_string());
            updated_fields.push("activeForm");
        }

        let requested_owner = input.owner.clone();
        if let Some(o) = &requested_owner
            && existing.owner.as_deref() != Some(o.as_str())
        {
            update.owner = Some(o.clone());
            updated_fields.push("owner");
        }

        // Auto-owner assignment: when a teammate sets status=in_progress
        // without an explicit owner and the task is unclaimed, auto-assign.
        if input.status == Some(TaskUpdateStatus::InProgress)
            && requested_owner.is_none()
            && existing.owner.is_none()
            && ctx.is_teammate
            && let Some(name) = ctx.agent_name.as_deref()
            && !name.is_empty()
        {
            update.owner = Some(name.to_string());
            if !updated_fields.contains(&"owner") {
                updated_fields.push("owner");
            }
        }

        // Status transition. `Deleted` is routed to a delete above (early
        // return) so `to_list_status` yields `None` and is skipped here; the
        // typed enum makes an "invalid status" impossible.
        if let Some(new_status) = input.status.and_then(TaskUpdateStatus::to_list_status)
            && new_status != existing.status
        {
            // Fire pre-hook via task-list store: the store runs
            // `HookEventType::TaskCompleted` on transition to Completed (see
            // `task_list.rs`). The hook fires from inside `update_task`.
            update.status = Some(new_status);
            status_change = Some((existing.status.as_str().into(), new_status.as_str().into()));
            updated_fields.push("status");
            if new_status == TaskListStatus::Completed {
                newly_completed = true;
            }
        }

        // Metadata merge (null deletions handled inside the store).
        if let Some(merge) = input.metadata
            && !merge.is_empty()
        {
            update.metadata_merge = Some(merge);
            updated_fields.push("metadata");
        }

        // TaskCompleted hook fires BEFORE the status flip is persisted
        // so a blocking hook leaves the task in its current state.
        if newly_completed && let Some(handle) = ctx.hook_handle.as_ref() {
            let outcome = handle
                .run_task_completed(
                    &task_id,
                    &existing.subject,
                    if existing.description.is_empty() {
                        None
                    } else {
                        Some(existing.description.as_str())
                    },
                    /*teammate_name*/ None,
                    /*team_name*/ None,
                )
                .await;
            if let Some(reason) = outcome.blocking_reason {
                return Err(ToolError::ExecutionFailed {
                    message: format!("TaskCompleted hook feedback:\n{reason}"),
                    display_data: None,
                    source: None,
                });
            }
        }

        // Persist the update.
        if update_has_changes(&update) {
            ctx.task_list
                .update_task(&task_id, update.clone())
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    message: format!("task_list.update_task failed: {e}"),
                    display_data: None,
                    source: None,
                })?;
        }

        // Add blocks / blockedBy edges (these go through block_task).
        if let Some(add_blocks) = input.add_blocks.as_deref() {
            let mut any_added = false;
            for id in add_blocks {
                let added = ctx
                    .task_list
                    .block_task(&task_id, id)
                    .await
                    .unwrap_or(false);
                if added {
                    any_added = true;
                }
            }
            if any_added {
                updated_fields.push("blocks");
            }
        }
        if let Some(add_blocked) = input.add_blocked_by.as_deref() {
            let mut any_added = false;
            for id in add_blocked {
                let added = ctx
                    .task_list
                    .block_task(id, &task_id)
                    .await
                    .unwrap_or(false);
                if added {
                    any_added = true;
                }
            }
            if any_added {
                updated_fields.push("blockedBy");
            }
        }

        // Mailbox-notify the new owner (swarm only).
        if let Some(new_owner) = update.owner.as_deref()
            && ctx.is_teammate
            && let Some(team_name) = ctx.team_name.as_deref()
        {
            let sender = ctx
                .agent_name
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or("team-lead");
            let body = serde_json::json!({
                "type": "task_assignment",
                "taskId": task_id,
                "subject": existing.subject,
                "description": existing.description,
                "assignedBy": sender,
                "timestamp": now_iso(),
            })
            .to_string();
            let envelope = MailboxEnvelope {
                text: body,
                from: sender.to_string(),
                timestamp: now_iso(),
            };
            // Best effort — failures in mailbox routing shouldn't block
            // the core task update.
            let _ = ctx
                .mailbox
                .write_to_mailbox(new_owner, team_name, envelope)
                .await;
        }

        // Verification nudge — #213: only fire when the verification agent
        // is actually registered.
        let is_main_thread = ctx.agent_id.is_none();
        let verification_nudge = verification_agent_registered(ctx)
            && ctx
                .task_list
                .should_nudge_verification(newly_completed, is_main_thread)
                .await;

        // Teammate completion nudge — fires when a swarm teammate
        // transitions a task to completed; primes the next TaskList
        // call so the agent picks up unblocked downstream work.
        let completed_nudge = newly_completed && ctx.is_teammate;

        // Auto-expand on update.
        let patch = build_task_list_patch(&ctx.task_list, verification_nudge).await;
        Ok(ToolResult {
            data: project_update(
                &task_id,
                updated_fields,
                status_change,
                verification_nudge,
                completed_nudge,
            ),
            new_messages: vec![],
            app_state_patch: Some(patch),
            permission_updates: Vec::new(),
            display_data: None,
        })
    }
}

fn update_has_changes(u: &TaskRecordUpdate) -> bool {
    u.subject.is_some()
        || u.description.is_some()
        || u.active_form.is_some()
        || u.owner.is_some()
        || u.status.is_some()
        || u.metadata_merge.is_some()
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

// ── TaskStopTool ──────────────────────────────────────────────────────

/// Prompt description for [`TaskStopTool`] (incl. leading and trailing newlines).
const TASK_STOP_PROMPT: &str = "
- Stops a running background task by its ID
- Takes a task_id parameter identifying the task to stop
- Returns a success or failure status
- Use this tool when you need to terminate a long-running task
";

/// Typed input for [`TaskStopTool`]. Advertises exactly two optional
/// keys — `task_id` (canonical) and `shell_id` (deprecated KillShell
/// compatibility). Both are optional; `execute` enforces that at least
/// one resolves.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct TaskStopInput {
    /// The ID of the background task to stop.
    #[serde(default)]
    pub task_id: Option<String>,
    /// Deprecated: use task_id instead.
    #[serde(default)]
    pub shell_id: Option<String>,
}

pub struct TaskStopTool;

#[async_trait::async_trait]
impl Tool for TaskStopTool {
    type Input = TaskStopInput;
    coco_tool_runtime::impl_runtime_schema!(TaskStopInput);
    type Output = Value;

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::TaskStop)
    }
    fn name(&self) -> &str {
        ToolName::TaskStop.as_str()
    }
    fn description(&self, _input: &TaskStopInput, _options: &DescriptionOptions) -> String {
        "Stop a running background task by ID".into()
    }
    async fn prompt(&self, _options: &PromptOptions) -> String {
        TASK_STOP_PROMPT.into()
    }

    fn is_concurrency_safe(&self, _input: &TaskStopInput) -> bool {
        true
    }
    fn should_defer(&self) -> bool {
        false
    }
    fn search_hint(&self) -> Option<&str> {
        Some("kill a running background task")
    }

    // The entire `{message, task_id, task_type}` envelope is emitted as
    // JSON. This matches the trait's default `render_for_model` impl
    // exactly, so no override is needed.

    async fn execute(
        &self,
        input: TaskStopInput,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<Value>, ToolError> {
        let task_id = [input.task_id.as_deref(), input.shell_id.as_deref()]
            .into_iter()
            .flatten()
            .find(|s| !s.is_empty())
            .map(String::from)
            .unwrap_or_default();
        if task_id.is_empty() {
            return Err(ToolError::InvalidInput {
                message: "Missing required parameter: task_id".into(),
                error_code: Some("1".into()),
            });
        }

        // The task must live in the running-task registry (`appState.tasks[id]`).
        // Plan items live in a disjoint namespace and must be terminated
        // via `TaskUpdate(status=completed|deleted)` — **not** this tool.
        let Some(handle) = ctx.task_handle.as_ref() else {
            return Err(ToolError::ExecutionFailed {
                message: format!(
                    "No running task found with ID: {task_id} (no task runtime configured)"
                ),
                display_data: None,
                source: None,
            });
        };
        // #49: pre-check status so a not-running task reports errorCode 3
        // (distinct from not-found), and capture the real task type +
        // command for the output.
        let state = handle.task_state(&task_id).await;
        if let Some(s) = &state
            && s.status != coco_types::TaskStatus::Running
        {
            return Err(ToolError::InvalidInput {
                message: format!(
                    "Task {task_id} is not running (status: {})",
                    task_status_wire(s.status)
                ),
                error_code: Some("3".into()),
            });
        }

        match handle.kill_task(&task_id).await {
            Ok(()) => {
                // Command is the shell command for shell tasks, else the
                // description. Type is the real wire name, not a hardcoded
                // "background".
                let (task_type, command) = match &state {
                    Some(s) => (
                        s.task_type().wire_name().to_string(),
                        s.shell_extras()
                            .map(|e| e.command.clone())
                            .filter(|c| !c.is_empty())
                            .unwrap_or_else(|| s.description.clone()),
                    ),
                    None => (
                        coco_types::TaskType::Shell.wire_name().to_string(),
                        task_id.clone(),
                    ),
                };
                Ok(ToolResult {
                    data: serde_json::json!({
                        "message": format!("Successfully stopped task: {task_id} ({command})"),
                        "task_id": task_id,
                        "task_type": task_type,
                        "command": command,
                    }),
                    new_messages: vec![],
                    app_state_patch: None,
                    permission_updates: Vec::new(),
                    display_data: None,
                })
            }
            Err(e) => Err(ToolError::ExecutionFailed {
                message: format!("No running task found with ID: {task_id} ({e})"),
                display_data: None,
                source: None,
            }),
        }
    }
}

/// #213: the verification nudge must only fire when the verification
/// agent is actually registered. Keys off the active agent catalog so
/// 3P builds without the agent never get spawn-nonexistent-agent
/// instructions.
fn verification_agent_registered(ctx: &ToolUseContext) -> bool {
    ctx.agent_catalog.as_ref().is_some_and(|c| {
        c.find_active(coco_types::SubagentType::Verification.as_str())
            .is_some()
    })
}

/// Lowercase wire string for a [`coco_types::TaskStatus`].
/// `TaskStatus` has no `as_str`, so spell it out here.
fn task_status_wire(status: coco_types::TaskStatus) -> &'static str {
    match status {
        coco_types::TaskStatus::Pending => "pending",
        coco_types::TaskStatus::Running => "running",
        coco_types::TaskStatus::Completed => "completed",
        coco_types::TaskStatus::Failed => "failed",
        coco_types::TaskStatus::Killed => "killed",
    }
}

// ── TaskOutputTool ────────────────────────────────────────────────────

/// Prompt for [`TaskOutputTool`] — deprecated notice.
const TASK_OUTPUT_PROMPT: &str = "DEPRECATED: Prefer using the Read tool on the task's output file path instead. Background tasks return their output file path in the tool result, and you receive a <task-notification> with the same path when the task completes — Read that file directly.

- Retrieves output from a running or completed task (background shell, agent, or remote session)
- Takes a task_id parameter identifying the task
- Returns the task output along with status information
- Use block=true (default) to wait for task completion
- Use block=false for non-blocking check of current status
- Task IDs can be found using the /tasks command
- Works with all task types: background shells, async agents, and remote sessions";

/// Typed input for [`TaskOutputTool`]. `task_id` is required, `block`
/// defaults to true, `timeout` is `0..=600000` defaulting to 30000.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct TaskOutputInput {
    /// The task ID to get output from
    pub task_id: String,
    /// When true (default), wait for the task to complete before
    /// returning. Set to false for an immediate snapshot.
    #[serde(default = "default_true")]
    #[schemars(extend("default" = true))]
    pub block: bool,
    /// Blocking timeout in milliseconds (default 30000).
    #[serde(default = "default_timeout_ms")]
    #[schemars(range(min = 0, max = 600000), extend("default" = 30000))]
    pub timeout: u64,
}

fn default_true() -> bool {
    true
}

fn default_timeout_ms() -> u64 {
    30_000
}

pub struct TaskOutputTool;

#[async_trait::async_trait]
impl Tool for TaskOutputTool {
    type Input = TaskOutputInput;
    // `block`/`timeout` bounds + defaults are declared via `#[schemars(...)]`
    // attrs on the struct, so the derived schema is correct and auto-closed —
    // no hand-patching.
    coco_tool_runtime::impl_runtime_schema!(TaskOutputInput);
    type Output = Value;

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::TaskOutput)
    }
    fn name(&self) -> &str {
        ToolName::TaskOutput.as_str()
    }
    fn search_hint(&self) -> Option<&str> {
        Some("read output/logs from a background task")
    }
    fn description(&self, _input: &TaskOutputInput, _options: &DescriptionOptions) -> String {
        "[Deprecated] — prefer Read on the task output file path".into()
    }
    async fn prompt(&self, _options: &PromptOptions) -> String {
        TASK_OUTPUT_PROMPT.into()
    }
    fn is_read_only(&self, _input: &TaskOutputInput) -> bool {
        true
    }
    fn is_always_read_only(&self) -> bool {
        true
    }
    fn is_concurrency_safe(&self, _input: &TaskOutputInput) -> bool {
        true
    }
    // The lone deferred Task tool: deprecated + low-frequency, so it stays
    // behind ToolSearch (matches TS `shouldDefer: true`). See the family defer
    // policy on `TaskCreateTool::should_defer`.
    fn should_defer(&self) -> bool {
        true
    }

    /// Render the retrieval envelope as XML tags. Format:
    /// ```text
    /// <retrieval_status>STATUS</retrieval_status>
    ///
    /// <task_id>ID</task_id>
    ///
    /// <task_type>TYPE</task_type>
    ///
    /// <status>TASK_STATUS</status>
    ///
    /// <exit_code>N</exit_code>      # only when present
    ///
    /// <output>
    /// CAPTURED_OUTPUT
    /// </output>                     # only when output is non-empty
    ///
    /// <error>...</error>            # only when present
    /// ```
    /// Pieces are joined with `\n\n`. Missing fields are skipped.
    fn render_for_model(&self, data: &Value) -> Vec<ToolResultContentPart> {
        let status = data
            .get("retrieval_status")
            .and_then(Value::as_str)
            .unwrap_or("");
        let mut parts: Vec<String> = vec![format!("<retrieval_status>{status}</retrieval_status>")];
        if let Some(task) = data.get("task")
            && !task.is_null()
        {
            let task_id = task.get("task_id").and_then(Value::as_str).unwrap_or("");
            let task_type = task.get("task_type").and_then(Value::as_str).unwrap_or("");
            let task_status = task.get("status").and_then(Value::as_str).unwrap_or("");
            parts.push(format!("<task_id>{task_id}</task_id>"));
            parts.push(format!("<task_type>{task_type}</task_type>"));
            parts.push(format!("<status>{task_status}</status>"));
            if let Some(code) = task.get("exitCode").and_then(Value::as_i64) {
                parts.push(format!("<exit_code>{code}</exit_code>"));
            }
            if let Some(output) = task.get("output").and_then(Value::as_str)
                && !output.trim().is_empty()
            {
                parts.push(format!("<output>\n{}\n</output>", output.trim_end()));
            }
            if let Some(err) = task.get("error").and_then(Value::as_str) {
                parts.push(format!("<error>{err}</error>"));
            }
        }
        vec![ToolResultContentPart::Text {
            text: parts.join("\n\n"),
            provider_options: None,
        }]
    }

    async fn execute(
        &self,
        input: TaskOutputInput,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<Value>, ToolError> {
        let task_id = input.task_id;
        if task_id.is_empty() {
            return Err(ToolError::InvalidInput {
                message: "task_id parameter is required".into(),
                error_code: None,
            });
        }
        let block = input.block;
        let timeout_ms = input.timeout;

        // Stage 1: background task namespace.
        if let Some(handle) = ctx.task_handle.as_ref()
            && let Ok(initial) = handle.get_task_status(&task_id).await
        {
            let info = if block {
                wait_for_task_completion(handle.as_ref(), &task_id, initial, timeout_ms).await
            } else {
                initial
            };
            let output = handle
                .get_task_output_delta(&task_id, 0)
                .await
                .map(|d| d.content)
                .unwrap_or_default();
            let retrieval = if info.status.is_terminal() {
                RetrievalStatus::Success
            } else if block {
                RetrievalStatus::Timeout
            } else {
                RetrievalStatus::NotReady
            };
            let status_str = task_status_wire_string(info.status);
            return Ok(ToolResult {
                data: project_output_background(&info.id, status_str, output, None, retrieval),
                new_messages: vec![],
                app_state_patch: None,
                permission_updates: Vec::new(),
                display_data: None,
            });
        }

        // Unknown IDs fall through to `{retrieval_status: 'not_ready', task: null}`.
        // Plan items live in a disjoint namespace and are inspected via
        // `TaskGet`; we do not read them here.
        Ok(ToolResult {
            data: serde_json::json!({
                "retrieval_status": RetrievalStatus::NotReady.as_str(),
                "task": null,
                "error": format!("Task '{task_id}' not found"),
            }),
            new_messages: vec![],
            app_state_patch: None,
            permission_updates: Vec::new(),
            display_data: None,
        })
    }
}

/// Wait for a background task to reach a terminal state. Event-
/// driven via [`TaskReader::subscribe_terminal`] — the production
/// impl returns a `watch::Receiver` that fires exactly once on the
/// `update_status` transition into Completed/Failed/Killed.
///
/// Falls back to a one-shot `get_task_status` snapshot if no
/// terminal subscription is available (test handles without watch
/// wiring). Uses O(1) await instead of polling — no reason to busy-wait.
async fn wait_for_task_completion(
    handle: &dyn coco_tool_runtime::TaskHandle,
    task_id: &str,
    initial: coco_types::TaskStateBase,
    timeout_ms: u64,
) -> coco_types::TaskStateBase {
    if initial.status.is_terminal() {
        return initial;
    }
    let timeout = std::time::Duration::from_millis(timeout_ms);

    let Some(signal) = handle.subscribe_terminal(task_id).await else {
        return initial;
    };
    tokio::select! {
        _final_status = signal.await_terminal() => {
            handle.get_task_status(task_id).await.unwrap_or(initial)
        }
        _ = tokio::time::sleep(timeout) => initial,
    }
}

/// Wire-string projection of [`coco_types::TaskStatus`] used in the
/// `TaskOutput` envelope's `<status>` tag. Returns lowercase strings.
fn task_status_wire_string(status: coco_types::TaskStatus) -> &'static str {
    match status {
        coco_types::TaskStatus::Pending => "pending",
        coco_types::TaskStatus::Running => "running",
        coco_types::TaskStatus::Completed => "completed",
        coco_types::TaskStatus::Failed => "failed",
        coco_types::TaskStatus::Killed => "killed",
    }
}

// ── TodoWriteTool ─────────────────────────────────────────────────────

/// Description for [`TodoWriteTool`].
const TODO_WRITE_DESCRIPTION: &str = "Update the todo list for the current session. To be used proactively and often to track progress and pending tasks. Make sure that at least one task is in_progress at all times. Always provide both content (imperative) and activeForm (present continuous) for each task.";

/// Prompt for [`TodoWriteTool`].
const TODO_WRITE_PROMPT: &str = "Use this tool to create and manage a structured task list for your current coding session. This helps you track progress, organize complex tasks, and demonstrate thoroughness to the user.
It also helps the user understand the progress of the task and overall progress of their requests.

## When to Use This Tool
Use this tool proactively in these scenarios:

1. Complex multi-step tasks - When a task requires 3 or more distinct steps or actions
2. Non-trivial and complex tasks - Tasks that require careful planning or multiple operations
3. User explicitly requests todo list - When the user directly asks you to use the todo list
4. User provides multiple tasks - When users provide a list of things to be done (numbered or comma-separated)
5. After receiving new instructions - Immediately capture user requirements as todos
6. When you start working on a task - Mark it as in_progress BEFORE beginning work. Ideally you should only have one todo as in_progress at a time
7. After completing a task - Mark it as completed and add any new follow-up tasks discovered during implementation

## When NOT to Use This Tool

Skip using this tool when:
1. There is only a single, straightforward task
2. The task is trivial and tracking it provides no organizational benefit
3. The task can be completed in less than 3 trivial steps
4. The task is purely conversational or informational

NOTE that you should not use this tool if there is only one trivial task to do. In this case you are better off just doing the task directly.

## Examples of When to Use the Todo List

<example>
User: I want to add a dark mode toggle to the application settings. Make sure you run the tests and build when you're done!
Assistant: *Creates todo list with the following items:*
1. Creating dark mode toggle component in Settings page
2. Adding dark mode state management (context/store)
3. Implementing CSS-in-JS styles for dark theme
4. Updating existing components to support theme switching
5. Running tests and build process, addressing any failures or errors that occur
*Begins working on the first task*

<reasoning>
The assistant used the todo list because:
1. Adding dark mode is a multi-step feature requiring UI, state management, and styling changes
2. The user explicitly requested tests and build be run afterward
3. The assistant inferred that tests and build need to pass by adding \"Ensure tests and build succeed\" as the final task
</reasoning>
</example>

<example>
User: Help me rename the function getCwd to getCurrentWorkingDirectory across my project
Assistant: *Uses grep or search tools to locate all instances of getCwd in the codebase*
I've found 15 instances of 'getCwd' across 8 different files.
*Creates todo list with specific items for each file that needs updating*

<reasoning>
The assistant used the todo list because:
1. First, the assistant searched to understand the scope of the task
2. Upon finding multiple occurrences across different files, it determined this was a complex task with multiple steps
3. The todo list helps ensure every instance is tracked and updated systematically
4. This approach prevents missing any occurrences and maintains code consistency
</reasoning>
</example>


<example>
User: I need to implement these features for my e-commerce site: user registration, product catalog, shopping cart, and checkout flow.
Assistant: *Creates a todo list breaking down each feature into specific tasks based on the project architecture*
Let's start with user registration. This will involve creating database models, API endpoints, and frontend forms that integrate with your existing authentication system.

<reasoning>
The assistant used the todo list because:
1. The user provided multiple complex features to implement in a comma separated list
2. The todo list helps organize these large features into manageable tasks
3. This approach allows for tracking progress across the entire implementation
</reasoning>
</example>

<example>
User: Can you help optimize my React application? It's rendering slowly and has performance issues.
Assistant: *Reviews component structure, render patterns, state management, and data fetching*
After analyzing your codebase, I've identified several performance issues.
*Creates todo list with items like: 1) Implementing memoization for expensive calculations in ProductList, 2) Adding virtualization for long lists in Dashboard, 3) Optimizing image loading in Gallery component, 4) Fixing state update loops in ShoppingCart, 5) Reviewing bundle size and implementing code splitting*
Let's start by implementing memoization for the expensive calculations in your ProductList component.

<reasoning>
The assistant used the todo list because:
1. First, the assistant examined the codebase to identify specific performance issues
2. Based on this analysis, it identified multiple optimization opportunities
3. Performance optimization is a non-trivial task requiring multiple steps
4. The todo list helps methodically track improvements across different components
5. This systematic approach ensures all performance bottlenecks are addressed
</reasoning>
</example>

## Examples of When NOT to Use the Todo List

<example>
User: How do I print 'Hello World' in Python?
Assistant: In Python, you can print \"Hello World\" with this simple code:

python
print(\"Hello World\")

This will output the text \"Hello World\" to the console when executed.

<reasoning>
The assistant did not use the todo list because this is a single, trivial task that can be completed in one step. There's no need to track multiple tasks or steps for such a straightforward request.
</reasoning>
</example>

<example>
User: What does the git status command do?
Assistant: The git status command shows the current state of your working directory and staging area. It displays which changes have been staged, which haven't, and which files aren't being tracked by Git. This is a fundamental command for understanding the current state of your repository before making commits.

<reasoning>
The assistant did not use the todo list because this is an informational request with no actual coding task to complete. The user is simply asking for an explanation, not for the assistant to perform multiple steps or tasks.
</reasoning>
</example>

<example>
User: Can you add a comment to the calculateTotal function to explain what it does?
Assistant: Sure, let me add a comment to the calculateTotal function to explain what it does.
* Uses the Edit tool to add a comment to the calculateTotal function *

<reasoning>
The assistant did not use the todo list because this is a single, straightforward task confined to one location in the code. Adding a comment doesn't require tracking multiple steps or systematic organization.
</reasoning>
</example>

<example>
User: Run npm install for me and tell me what happens.
Assistant: I'll run the npm install command for you.

*Executes: npm install*

The command completed successfully. Here's the output:
[Output of npm install command]

All dependencies have been installed according to your package.json file.

<reasoning>
The assistant did not use the todo list because this is a single command execution with immediate results. There are no multiple steps to track or organize, making the todo list unnecessary for this straightforward task.
</reasoning>
</example>

## Task States and Management

1. **Task States**: Use these states to track progress:
   - pending: Task not yet started
   - in_progress: Currently working on (limit to ONE task at a time)
   - completed: Task finished successfully

   **IMPORTANT**: Task descriptions must have two forms:
   - content: The imperative form describing what needs to be done (e.g., \"Run tests\", \"Build the project\")
   - activeForm: The present continuous form shown during execution (e.g., \"Running tests\", \"Building the project\")

2. **Task Management**:
   - Update task status in real-time as you work
   - Mark tasks complete IMMEDIATELY after finishing (don't batch completions)
   - Exactly ONE task must be in_progress at any time (not less, not more)
   - Complete current tasks before starting new ones
   - Remove tasks that are no longer relevant from the list entirely

3. **Task Completion Requirements**:
   - ONLY mark a task as completed when you have FULLY accomplished it
   - If you encounter errors, blockers, or cannot finish, keep the task as in_progress
   - When blocked, create a new task describing what needs to be resolved
   - Never mark a task as completed if:
     - Tests are failing
     - Implementation is partial
     - You encountered unresolved errors
     - You couldn't find necessary files or dependencies

4. **Task Breakdown**:
   - Create specific, actionable items
   - Break complex tasks into smaller, manageable steps
   - Use clear, descriptive task names
   - Always provide both forms:
     - content: \"Fix authentication bug\"
     - activeForm: \"Fixing authentication bug\"

When in doubt, use this tool. Being proactive with task management demonstrates attentiveness and ensures you complete all requirements successfully.
";

/// Typed input for [`TodoWriteTool`]. Items deserialize directly into
/// the `TodoRecord` type; `JsonSchema` is derived at its definition.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct TodoWriteInput {
    /// The updated todo list. Pass the full list each call; the
    /// prior list is replaced. `todos` is a required field.
    pub todos: Vec<TodoRecord>,
}

pub struct TodoWriteTool;

#[async_trait::async_trait]
impl Tool for TodoWriteTool {
    type Input = TodoWriteInput;
    // `todos` items derive from `TodoRecord`, whose `#[schemars(...)]` attrs
    // carry the `content`/`activeForm` `minLength:1` and the `status` enum,
    // so the derived schema is correct + auto-closed.
    coco_tool_runtime::impl_runtime_schema!(TodoWriteInput);
    type Output = Value;

    fn to_auto_classifier_input(&self, input: &TodoWriteInput) -> Option<String> {
        Some(format!("{} items", input.todos.len()))
    }

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::TodoWrite)
    }
    fn name(&self) -> &str {
        ToolName::TodoWrite.as_str()
    }
    fn description(&self, _input: &TodoWriteInput, _options: &DescriptionOptions) -> String {
        TODO_WRITE_DESCRIPTION.into()
    }
    async fn prompt(&self, _options: &PromptOptions) -> String {
        TODO_WRITE_PROMPT.into()
    }
    fn is_enabled(&self, ctx: &ToolUseContext) -> bool {
        !ctx.features.enabled(Feature::TaskV2)
    }
    fn should_defer(&self) -> bool {
        false
    }
    fn search_hint(&self) -> Option<&str> {
        Some("manage the session task checklist")
    }

    /// The model only needs the success message + optional verification
    /// nudge — `oldTodos`/`newTodos` arrays are TUI/state concerns, not
    /// model-visible content. JSON-stringifying them wastes tokens.
    fn render_for_model(&self, data: &Value) -> Vec<ToolResultContentPart> {
        let nudge_needed = data
            .get("verificationNudgeNeeded")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let base = "Todos have been modified successfully. Ensure that you continue to use the todo list to track your progress. Please proceed with the current tasks if applicable";
        let text = if nudge_needed {
            format!(
                "{base}\n\nNOTE: You just closed out 3+ tasks and none of them was a verification step. Before writing your final summary, spawn the verification agent. You cannot self-assign PARTIAL by listing caveats in your summary — only the verifier issues a verdict."
            )
        } else {
            base.to_string()
        };
        vec![ToolResultContentPart::Text {
            text,
            provider_options: None,
        }]
    }

    async fn execute(
        &self,
        input: TodoWriteInput,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<Value>, ToolError> {
        let incoming = input.todos;

        for (i, item) in incoming.iter().enumerate() {
            if item.content.is_empty() {
                return Err(ToolError::InvalidInput {
                    message: format!("todos[{i}].content cannot be empty"),
                    error_code: None,
                });
            }
            if item.active_form.is_empty() {
                return Err(ToolError::InvalidInput {
                    message: format!("todos[{i}].activeForm cannot be empty"),
                    error_code: None,
                });
            }
            if !matches!(
                item.status.as_str(),
                "pending" | "in_progress" | "completed"
            ) {
                return Err(ToolError::InvalidInput {
                    message: format!(
                        "todos[{i}].status must be pending, in_progress, or completed"
                    ),
                    error_code: None,
                });
            }
        }

        let key = todo_key(ctx);
        let old_todos = ctx.todo_list.read(&key).await;

        // `allDone → clear`.
        let all_done = !incoming.is_empty() && incoming.iter().all(|t| t.status == "completed");
        let to_store = if all_done {
            Vec::new()
        } else {
            incoming.clone()
        };
        ctx.todo_list.write(&key, to_store).await;

        // Verification nudge — main-thread only, all done, ≥3 items,
        // no verify matches. Main-thread here = `ctx.agent_id.is_none()`.
        let is_main_thread = ctx.agent_id.is_none();
        let verification_nudge = if is_main_thread && all_done && verification_agent_registered(ctx)
        {
            let contents: Vec<&str> = incoming.iter().map(|i| i.content.as_str()).collect();
            coco_tool_runtime::check_verification_nudge(&contents)
        } else {
            false
        };

        let old_json = serde_json::to_value(&old_todos).unwrap_or_default();
        let new_json = serde_json::to_value(&incoming).unwrap_or_default();
        let patch = build_todo_patch(&ctx.todo_list, key, verification_nudge).await;
        Ok(ToolResult {
            data: serde_json::json!({
                "oldTodos": old_json,
                "newTodos": new_json,
                "verificationNudgeNeeded": verification_nudge,
            }),
            new_messages: vec![],
            app_state_patch: Some(patch),
            permission_updates: Vec::new(),
            display_data: None,
        })
    }
}

#[cfg(test)]
#[path = "task_tools.test.rs"]
mod tests;
