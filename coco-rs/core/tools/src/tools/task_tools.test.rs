//! Tests for task_tools.
//!
//! Alignment contracts locked in here:
//! - TaskStop operates on the running-task registry only.
//!   Plan-item IDs must error; completing/deleting plan items uses `TaskUpdate`.
//! - TaskOutput operates on the running-task registry only; unknown IDs
//!   return `{retrieval_status: "not_ready", task: null}`.
//! - TaskCreate/Get/List/Update output shapes are stable.

use super::TaskCreateTool;
use super::TaskStopTool;
use coco_tool_runtime::DescriptionOptions;
use coco_tool_runtime::DynTool;
use coco_tool_runtime::PromptOptions;
use coco_tool_runtime::{
    BackgroundShellRequest, TaskHandle, TaskOutputDelta, TerminalSignal, ToolUseContext,
};
use coco_types::{TaskExtras, TaskStateBase, TaskStatus};
use serde_json::json;
use std::sync::Arc;

/// Test double that tracks `kill_task` / `get_task_status` calls and
/// returns canned results. Exercises the TaskStop/TaskOutput
/// paths (which only operate on `appState.tasks`, i.e. the running-task
/// registry).
#[derive(Default)]
struct RecordingTaskHandle {
    known_ids: std::sync::Mutex<Vec<String>>,
    kill_calls: std::sync::Mutex<Vec<String>>,
}

impl RecordingTaskHandle {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    fn register(&self, task_id: &str) {
        self.known_ids.lock().unwrap().push(task_id.to_string());
    }
    fn killed(&self) -> Vec<String> {
        self.kill_calls.lock().unwrap().clone()
    }
}

fn canned_state(task_id: &str) -> TaskStateBase {
    TaskStateBase {
        id: task_id.into(),
        status: TaskStatus::Running,
        notified: false,
        description: String::new(),
        tool_use_id: None,
        start_time: 0,
        end_time: None,
        total_paused_ms: None,
        output_file: None,
        output_offset: 0,
        extras: TaskExtras::shell_default(),
    }
}

/// Catalog snapshot containing the built-in `verification` agent, so the
/// #213 verification-nudge gate passes in tests.
fn catalog_with_verification() -> std::sync::Arc<coco_subagent::AgentCatalogSnapshot> {
    let mut store = coco_subagent::AgentDefinitionStore::new(
        coco_subagent::BuiltinAgentCatalog::all_enabled(),
        coco_subagent::AgentSearchPaths::empty(),
    );
    store.load();
    store.snapshot()
}

#[async_trait::async_trait]
impl TaskHandle for RecordingTaskHandle {
    async fn get_task_status(
        &self,
        task_id: &str,
    ) -> Result<TaskStateBase, coco_error::BoxedError> {
        if self
            .known_ids
            .lock()
            .unwrap()
            .iter()
            .any(|id| id == task_id)
        {
            Ok(canned_state(task_id))
        } else {
            Err(Box::new(coco_error::PlainError::new(
                format!("unknown background task: {task_id}"),
                coco_error::StatusCode::Internal,
            )))
        }
    }
    async fn get_task_output_delta(
        &self,
        _task_id: &str,
        _from_offset: i64,
    ) -> Result<TaskOutputDelta, coco_error::BoxedError> {
        Ok(TaskOutputDelta {
            content: String::new(),
            new_offset: 0,
            is_complete: false,
        })
    }
    async fn list_tasks(&self) -> Vec<TaskStateBase> {
        Vec::new()
    }
    async fn subscribe_terminal(&self, _: &str) -> Option<TerminalSignal> {
        None
    }
    async fn detach_handle(&self, _: &str) -> Option<std::sync::Arc<tokio::sync::Notify>> {
        None
    }
    async fn read_terminal_outputs(
        &self,
        _: &str,
    ) -> Result<coco_tool_runtime::TerminalOutputs, coco_error::BoxedError> {
        Err(Box::new(coco_error::PlainError::new(
            "read_terminal_outputs not used in these tests",
            coco_error::StatusCode::Internal,
        )))
    }
    async fn read_output(&self, _: &str) -> String {
        String::new()
    }
    async fn task_state(&self, task_id: &str) -> Option<TaskStateBase> {
        if self
            .known_ids
            .lock()
            .unwrap()
            .iter()
            .any(|id| id == task_id)
        {
            Some(canned_state(task_id))
        } else {
            None
        }
    }
    async fn is_terminal(&self, _: &str) -> bool {
        false
    }
    async fn kill_task(&self, task_id: &str) -> Result<(), coco_error::BoxedError> {
        if self
            .known_ids
            .lock()
            .unwrap()
            .iter()
            .any(|id| id == task_id)
        {
            self.kill_calls.lock().unwrap().push(task_id.to_string());
            Ok(())
        } else {
            Err(Box::new(coco_error::PlainError::new(
                format!("task not found: {task_id}"),
                coco_error::StatusCode::Internal,
            )))
        }
    }
    async fn signal_detach(&self, _: &str) -> coco_tool_runtime::DetachOutcome {
        coco_tool_runtime::DetachOutcome::Unknown
    }
    async fn spawn_shell_task(
        &self,
        _request: BackgroundShellRequest,
    ) -> Result<String, coco_error::BoxedError> {
        unimplemented!("not used in these tests")
    }
    async fn register_agent_task(
        &self,
        _: &str,
        _: Option<&str>,
        _: Option<&str>,
        _: tokio_util::sync::CancellationToken,
        _: coco_tool_runtime::AgentRegistration,
    ) -> String {
        String::new()
    }
    async fn register_agent_task_with_id(
        &self,
        task_id: String,
        _: &str,
        _: Option<&str>,
        _: Option<&str>,
        _: tokio_util::sync::CancellationToken,
        _: coco_tool_runtime::AgentRegistration,
    ) -> String {
        task_id
    }
    async fn register_dream_task(&self, _: &str, _: tokio_util::sync::CancellationToken) -> String {
        String::new()
    }
    async fn append_output(&self, _: &str, _: &str) {}
    async fn set_progress_summary(&self, _: &str, _: String) {}
    async fn set_progress(&self, _: &str, _: coco_types::TaskProgress) {}
    async fn mark_completed(&self, _: &str, _: coco_tool_runtime::AgentCompletionPayload) {}
    async fn mark_failed(&self, _: &str, _: &str) {}
    async fn complete_silent(&self, _: &str, _: bool) {}
}

// ---------------------------------------------------------------------------
// TaskStop: unified entry for shell + agent + TODO tasks
// ---------------------------------------------------------------------------

/// TaskStop accepts `task_id` (canonical) and `shell_id` (deprecated
/// KillShell alias) as equivalent parameter names. Missing both is an
/// InvalidInput error.
#[tokio::test]
async fn test_task_stop_rejects_missing_id() {
    let ctx = ToolUseContext::test_default();
    let result = <TaskStopTool as DynTool>::execute(&TaskStopTool, json!({}), &ctx).await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("task_id"),
        "error should mention task_id: {err}"
    );
}

/// TaskStop must accept all three alias names when they resolve to a
/// registered background task. Uses a `RecordingTaskHandle` to stand in
/// for the running-task registry (`appState.tasks`).
#[tokio::test]
async fn test_task_stop_accepts_task_id_for_background_task() {
    let handle = RecordingTaskHandle::new();
    handle.register("bg-1");
    let mut ctx = ToolUseContext::test_default();
    ctx.task_handle = Some(handle.clone());

    let stop_result =
        <TaskStopTool as DynTool>::execute(&TaskStopTool, json!({"task_id": "bg-1"}), &ctx)
            .await
            .unwrap();
    assert_eq!(stop_result.data["task_id"], "bg-1");
    assert_eq!(
        stop_result.data["task_type"],
        coco_types::TaskType::Shell.wire_name()
    );
    assert_eq!(handle.killed(), vec!["bg-1".to_string()]);
}

#[tokio::test]
async fn test_task_stop_accepts_shell_id_alias() {
    let handle = RecordingTaskHandle::new();
    handle.register("bg-2");
    let mut ctx = ToolUseContext::test_default();
    ctx.task_handle = Some(handle.clone());

    let stop_result =
        <TaskStopTool as DynTool>::execute(&TaskStopTool, json!({"shell_id": "bg-2"}), &ctx)
            .await
            .unwrap();
    assert_eq!(stop_result.data["task_id"], "bg-2");
    assert_eq!(
        stop_result.data["task_type"],
        coco_types::TaskType::Shell.wire_name()
    );
}

/// Plan-item IDs are NOT valid for TaskStop, which only operates on
/// running tasks (`appState.tasks`). Must surface as an error so the
/// model learns to use `TaskUpdate(status=completed|deleted)` instead.
#[tokio::test]
async fn test_task_stop_rejects_plan_item_id() {
    let ctx = ToolUseContext::test_default();
    let create = <TaskCreateTool as DynTool>::execute(
        &TaskCreateTool,
        json!({"subject": "plan item", "description": "x"}),
        &ctx,
    )
    .await
    .unwrap();
    let tid = create.data["task"]["id"].as_str().unwrap().to_string();

    // No TaskHandle registered for this id → must error out.
    let err = <TaskStopTool as DynTool>::execute(&TaskStopTool, json!({"task_id": tid}), &ctx)
        .await
        .expect_err("plan-item id must not be accepted by TaskStop");
    let msg = err.to_string();
    assert!(
        msg.contains("No running task found"),
        "error must steer the model: {msg}"
    );
}

/// Unknown IDs return a structured error, not a hard panic. Because the
/// TaskHandle stage will also fail (NoOpTaskHandle → err), the final result
/// should be an error JSON payload mentioning "not found".
#[tokio::test]
async fn test_task_stop_unknown_id_returns_error() {
    // R3: unknown ID must surface as a tool error, not as a successful
    // ToolResult with an `error` field. The model perceives the two cases
    // differently — errors trigger retry logic, successful results don't.
    let ctx = ToolUseContext::test_default();
    let result = <TaskStopTool as DynTool>::execute(
        &TaskStopTool,
        json!({"task_id": "nonexistent-id-12345"}),
        &ctx,
    )
    .await;
    assert!(result.is_err(), "unknown ID should surface as tool error");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("No running task found"),
        "error should name the missing running-task namespace: {err}"
    );
}

// ---------------------------------------------------------------------------
// B4.5: TaskOutput blocking with Notify (polling-based wait)
// ---------------------------------------------------------------------------

use super::TaskOutputTool;

/// TaskOutput operates on the running-task registry only
/// (`appState.tasks`). A plan-item id is unknown from that perspective
/// and must return `{retrieval_status: "not_ready", task: null}`.
#[tokio::test]
async fn test_task_output_returns_null_for_plan_item_id() {
    let ctx = ToolUseContext::test_default();
    let create = <TaskCreateTool as DynTool>::execute(
        &TaskCreateTool,
        json!({"subject": "snapshot test", "description": "x"}),
        &ctx,
    )
    .await
    .unwrap();
    let tid = create.data["task"]["id"].as_str().unwrap().to_string();

    let result =
        <TaskOutputTool as DynTool>::execute(&TaskOutputTool, json!({"task_id": tid}), &ctx)
            .await
            .unwrap();
    assert_eq!(result.data["retrieval_status"], "not_ready");
    assert!(result.data["task"].is_null());
}

/// TaskOutput resolves output for a registered background task by its
/// canonical `task_id` (the only key; no camelCase alias).
#[tokio::test]
async fn test_task_output_accepts_task_id() {
    let handle = RecordingTaskHandle::new();
    handle.register("bg-output");
    let mut ctx = ToolUseContext::test_default();
    ctx.task_handle = Some(handle);

    let result = <TaskOutputTool as DynTool>::execute(
        &TaskOutputTool,
        json!({"task_id": "bg-output", "block": false}),
        &ctx,
    )
    .await
    .unwrap();
    assert_eq!(result.data["task"]["task_id"], "bg-output");
    assert_eq!(result.data["task"]["task_type"], "background");
}

/// Unknown IDs return a structured error with `retrieval_status: "not_ready"`
/// and `task: null`.
#[tokio::test]
async fn test_task_output_unknown_id_returns_error() {
    let ctx = ToolUseContext::test_default();
    let result = <TaskOutputTool as DynTool>::execute(
        &TaskOutputTool,
        json!({"task_id": "nonexistent-xyz"}),
        &ctx,
    )
    .await
    .unwrap();
    assert_eq!(result.data["retrieval_status"], "not_ready");
    assert!(result.data["task"].is_null());
    let err = result.data["error"].as_str().unwrap_or_default();
    assert!(err.contains("not found"));
}

/// Missing ID parameter → InvalidInput error.
#[tokio::test]
async fn test_task_output_rejects_missing_id() {
    let ctx = ToolUseContext::test_default();
    let result = <TaskOutputTool as DynTool>::execute(&TaskOutputTool, json!({}), &ctx).await;
    assert!(result.is_err());
}

/// `block` defaults to `true`. Regression guard: if we ever flip the
/// default back to false, this test catches it.
/// Note: the TODO-task fall-through in test_default always returns
/// `blocked: false` because TODO tasks can't actually block (they're
/// synchronous). The default only matters for background tasks via
/// TaskHandle — which we can't easily exercise without an impl. We
/// instead assert the schema DEFAULT by inspecting the JSON description.
#[test]
fn test_task_output_schema_documents_block_default_true() {
    let schema = <TaskOutputTool as DynTool>::runtime_validation_schema(&TaskOutputTool).as_value();
    let block_prop = schema["properties"].get("block").unwrap();
    let desc = block_prop["description"].as_str().unwrap();
    assert!(
        desc.contains("true (default)") || desc.contains("default true"),
        "block param description should advertise default=true, got: {desc}"
    );
}

/// `task_id` is required, `timeout` is `0..=600000` defaulting to 30000.
/// Lock the constraints into the model-facing schema.
#[test]
fn test_task_output_schema_required_id_and_timeout_bounds() {
    let schema = <TaskOutputTool as DynTool>::runtime_validation_schema(&TaskOutputTool);
    assert!(
        required_fields(schema.as_value()).contains(&"task_id"),
        "task_id must be required"
    );
    // No camelCase alias property (strictObject has only task_id).
    assert!(
        schema.as_value()["properties"].get("taskId").is_none(),
        "TaskOutput must not advertise a taskId alias"
    );
    let timeout = &schema.as_value()["properties"]["timeout"];
    assert_eq!(timeout["maximum"], json!(600000));
    assert_eq!(timeout["default"], json!(30000));
    // Strict object: an unknown key is rejected.
    assert!(
        schema
            .validate(&json!({"task_id": "x", "bogus": 1}))
            .is_err(),
        "TaskOutput is a strict object"
    );
}

/// TaskStop advertises only `task_id` and `shell_id` — no camelCase
/// `taskId` alias.
#[test]
fn test_task_stop_schema_has_no_taskid_alias() {
    let schema = <TaskStopTool as DynTool>::runtime_validation_schema(&TaskStopTool);
    let props = schema.as_value()["properties"].as_object().unwrap();
    assert!(props.contains_key("task_id"));
    assert!(props.contains_key("shell_id"));
    assert!(
        !props.contains_key("taskId"),
        "TaskStop must not advertise a taskId alias property"
    );
}

/// Canonical parameter takes precedence over deprecated aliases when
/// multiple are provided.
#[tokio::test]
async fn test_task_stop_canonical_precedence() {
    let handle = RecordingTaskHandle::new();
    handle.register("bg-canonical");
    let mut ctx = ToolUseContext::test_default();
    ctx.task_handle = Some(handle.clone());

    // Both `task_id` (valid) and `shell_id` (garbage) — canonical must win.
    let stop_result = <TaskStopTool as DynTool>::execute(
        &TaskStopTool,
        json!({"task_id": "bg-canonical", "shell_id": "garbage-id"}),
        &ctx,
    )
    .await
    .unwrap();
    assert_eq!(stop_result.data["task_id"], "bg-canonical");
    assert_eq!(
        stop_result.data["task_type"],
        coco_types::TaskType::Shell.wire_name()
    );
    assert_eq!(handle.killed(), vec!["bg-canonical".to_string()]);
}

// ---------------------------------------------------------------------------
// TodoWriteTool
// ---------------------------------------------------------------------------
//
// Uses replace-all semantics — the model sends the complete list on every
// call, prior contents are replaced, and the response returns `{oldTodos,
// newTodos, verificationNudgeNeeded}`. Each `TodoItem` must have `content`,
// `status`, and `activeForm` (min-length 1 each, no `id`). These tests
// lock in the schema + output shape so regressions are caught early.

use super::TodoWriteTool;

/// Schema requires:
///   - items have `content`, `status`, `activeForm` (all required)
///   - NO `id` field
#[test]
fn test_todo_write_schema_matches_ts() {
    let schema = <TodoWriteTool as DynTool>::runtime_validation_schema(&TodoWriteTool).as_value();
    let todos_prop = schema["properties"].get("todos").unwrap();
    let items = &todos_prop["items"];
    let required = items["required"].as_array().unwrap();

    // Must require exactly content, status, activeForm.
    let required_set: std::collections::HashSet<_> = required
        .iter()
        .filter_map(|v| v.as_str())
        .map(String::from)
        .collect();
    assert!(required_set.contains("content"));
    assert!(required_set.contains("status"));
    assert!(required_set.contains("activeForm"));
    assert!(!required_set.contains("id"), "TodoItem has no id field");

    // Status enum values.
    let status_enum = items["properties"]["status"]["enum"].as_array().unwrap();
    let enum_set: std::collections::HashSet<_> =
        status_enum.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(
        enum_set,
        ["pending", "in_progress", "completed"]
            .into_iter()
            .collect()
    );

    // content/activeForm carry `.min(1)`.
    assert_eq!(items["properties"]["content"]["minLength"], json!(1));
    assert_eq!(items["properties"]["activeForm"]["minLength"], json!(1));

    // `todos` field is required.
    assert!(
        required_fields(schema).contains(&"todos"),
        "todos must be a required field"
    );
}

/// Round-trip: write a todo list, verify the output has the expected
/// `{oldTodos, newTodos, verificationNudgeNeeded}` shape.
#[tokio::test]
async fn test_todo_write_output_shape_matches_ts() {
    let ctx = ToolUseContext::test_default();

    // Clear any leftover state from parallel tests.
    let _ = <TodoWriteTool as DynTool>::execute(&TodoWriteTool, json!({"todos": []}), &ctx)
        .await
        .unwrap();

    let result = <TodoWriteTool as DynTool>::execute(
        &TodoWriteTool,
        json!({
            "todos": [
                {
                    "content": "write the tests",
                    "status": "pending",
                    "activeForm": "Writing the tests"
                }
            ]
        }),
        &ctx,
    )
    .await
    .unwrap();

    // Expected output keys.
    assert!(result.data["oldTodos"].is_array(), "oldTodos missing");
    assert!(result.data["newTodos"].is_array(), "newTodos missing");
    assert!(
        result.data["verificationNudgeNeeded"].is_boolean(),
        "verificationNudgeNeeded missing"
    );

    let new_todos = result.data["newTodos"].as_array().unwrap();
    assert_eq!(new_todos.len(), 1);
    assert_eq!(new_todos[0]["content"], "write the tests");
    assert_eq!(new_todos[0]["activeForm"], "Writing the tests");
    assert!(
        new_todos[0].get("id").is_none(),
        "must not emit an id field"
    );
}

/// Missing `activeForm` is rejected (`min(1)` constraint).
#[tokio::test]
async fn test_todo_write_rejects_missing_active_form() {
    let ctx = ToolUseContext::test_default();
    let result = <TodoWriteTool as DynTool>::execute(
        &TodoWriteTool,
        json!({
            "todos": [
                {"content": "x", "status": "pending"}
            ]
        }),
        &ctx,
    )
    .await;
    assert!(result.is_err(), "missing activeForm must error");
}

/// Empty `content` is rejected.
#[tokio::test]
async fn test_todo_write_rejects_empty_content() {
    let ctx = ToolUseContext::test_default();
    let result = <TodoWriteTool as DynTool>::execute(
        &TodoWriteTool,
        json!({
            "todos": [
                {"content": "", "status": "pending", "activeForm": "Doing"}
            ]
        }),
        &ctx,
    )
    .await;
    assert!(result.is_err(), "empty content must error");
}

/// Invalid status is rejected.
#[tokio::test]
async fn test_todo_write_rejects_bad_status() {
    let ctx = ToolUseContext::test_default();
    let result = <TodoWriteTool as DynTool>::execute(
        &TodoWriteTool,
        json!({
            "todos": [
                {"content": "x", "status": "cancelled", "activeForm": "Doing"}
            ]
        }),
        &ctx,
    )
    .await;
    assert!(result.is_err(), "status=cancelled must error");
}

// ---------------------------------------------------------------------------
// Output schemas for TaskCreate/Get/List/Update/Output
// ---------------------------------------------------------------------------
//
// Task tools return minimal wrapped JSON (`{task: {...}}`,
// `{tasks: [...]}`) so internal fields like `output`, `active_form`,
// `metadata` never leak to the model. These tests lock in each shape.

use super::TaskGetTool;
use super::TaskListTool;
use super::TaskUpdateTool;

fn required_fields(schema: &serde_json::Value) -> Vec<&str> {
    schema["required"]
        .as_array()
        .map(|values| {
            values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn test_task_v2_input_schema_required_fields_and_strict_aliases() {
    let create_schema = <TaskCreateTool as DynTool>::runtime_validation_schema(&TaskCreateTool);
    let create_required = required_fields(create_schema.as_value());
    assert!(create_required.contains(&"subject"));
    assert!(create_required.contains(&"description"));
    assert!(
        create_schema
            .validate(&json!({"name": "legacy", "description": "x"}))
            .is_err(),
        "TaskCreate must not accept name as a subject alias"
    );
    assert!(
        create_schema
            .validate(&json!({"description": "x"}))
            .is_err(),
        "TaskCreate subject is required"
    );

    let get_schema = <TaskGetTool as DynTool>::runtime_validation_schema(&TaskGetTool);
    assert!(required_fields(get_schema.as_value()).contains(&"taskId"));
    assert!(
        get_schema.validate(&json!({})).is_err(),
        "TaskGet taskId is required"
    );

    let update_schema = <TaskUpdateTool as DynTool>::runtime_validation_schema(&TaskUpdateTool);
    assert!(required_fields(update_schema.as_value()).contains(&"taskId"));
    assert!(
        update_schema
            .validate(&json!({"status": "completed"}))
            .is_err(),
        "TaskUpdate taskId is required"
    );
    // Status enum includes `deleted`.
    let status_enum: std::collections::HashSet<&str> =
        update_schema.as_value()["properties"]["status"]["enum"]
            .as_array()
            .expect("status enum present")
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect();
    assert_eq!(
        status_enum,
        ["pending", "in_progress", "completed", "deleted"]
            .into_iter()
            .collect()
    );
    // An out-of-enum status is rejected at validation.
    assert!(
        update_schema
            .validate(&json!({"taskId": "1", "status": "bogus"}))
            .is_err(),
        "TaskUpdate status must be one of the advertised enum values"
    );

    let list_schema = <TaskListTool as DynTool>::runtime_validation_schema(&TaskListTool);
    assert!(
        list_schema.validate(&json!({"status": "pending"})).is_err(),
        "TaskList is a strict empty object"
    );
}

#[tokio::test]
async fn test_task_v2_description_search_hint_and_prompt_text_matches_ts() {
    let default_opts = PromptOptions::default();
    let team_opts = PromptOptions {
        agent_teams_available: true,
        ..Default::default()
    };
    let description_opts = DescriptionOptions::default();

    assert_eq!(
        <TaskCreateTool as DynTool>::description(
            &TaskCreateTool,
            &json!({"subject": "s", "description": "d"}),
            &description_opts
        ),
        "Create a new task in the task list"
    );
    assert_eq!(
        <TaskCreateTool as DynTool>::search_hint(&TaskCreateTool),
        Some("create a task in the task list")
    );
    assert_eq!(
        <TaskGetTool as DynTool>::description(
            &TaskGetTool,
            &json!({"taskId": "1"}),
            &description_opts
        ),
        "Get a task by ID from the task list"
    );
    assert_eq!(
        <TaskGetTool as DynTool>::search_hint(&TaskGetTool),
        Some("retrieve a task by ID")
    );
    assert_eq!(
        <TaskListTool as DynTool>::description(&TaskListTool, &json!({}), &description_opts),
        "List all tasks in the task list"
    );
    assert_eq!(
        <TaskListTool as DynTool>::search_hint(&TaskListTool),
        Some("list all tasks")
    );
    assert_eq!(
        <TaskUpdateTool as DynTool>::description(
            &TaskUpdateTool,
            &json!({"taskId": "1"}),
            &description_opts
        ),
        "Update a task in the task list"
    );
    assert_eq!(
        <TaskUpdateTool as DynTool>::search_hint(&TaskUpdateTool),
        Some("update a task")
    );

    let create_prompt = <TaskCreateTool as DynTool>::prompt(&TaskCreateTool, &default_opts).await;
    assert_eq!(create_prompt, TASK_CREATE_PROMPT_NO_TEAMS);

    let create_team_prompt = <TaskCreateTool as DynTool>::prompt(&TaskCreateTool, &team_opts).await;
    assert_eq!(create_team_prompt, TASK_CREATE_PROMPT_TEAMS);

    let get_prompt = <TaskGetTool as DynTool>::prompt(&TaskGetTool, &default_opts).await;
    assert_eq!(get_prompt, TASK_GET_PROMPT_TS);

    let list_prompt = <TaskListTool as DynTool>::prompt(&TaskListTool, &default_opts).await;
    assert_eq!(list_prompt, TASK_LIST_PROMPT_NO_TEAMS);

    let list_prompt = <TaskListTool as DynTool>::prompt(&TaskListTool, &team_opts).await;
    assert_eq!(list_prompt, TASK_LIST_PROMPT_TEAMS);

    let update_prompt = <TaskUpdateTool as DynTool>::prompt(&TaskUpdateTool, &default_opts).await;
    assert_eq!(update_prompt, TASK_UPDATE_PROMPT_TS);

    let update_prompt = <TaskUpdateTool as DynTool>::prompt(&TaskUpdateTool, &team_opts).await;
    assert_eq!(update_prompt, TASK_UPDATE_PROMPT_TS);
}

const TASK_CREATE_PROMPT_NO_TEAMS: &str = "Use this tool to create a structured task list for your current coding session. This helps you track progress, organize complex tasks, and demonstrate thoroughness to the user.
It also helps the user understand the progress of the task and overall progress of their requests.

## When to Use This Tool

Use this tool proactively in these scenarios:

- Complex multi-step tasks - When a task requires 3 or more distinct steps or actions
- Non-trivial and complex tasks - Tasks that require careful planning or multiple operations
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
- Check TaskList first to avoid creating duplicate tasks
";

const TASK_CREATE_PROMPT_TEAMS: &str = "Use this tool to create a structured task list for your current coding session. This helps you track progress, organize complex tasks, and demonstrate thoroughness to the user.
It also helps the user understand the progress of the task and overall progress of their requests.

## When to Use This Tool

Use this tool proactively in these scenarios:

- Complex multi-step tasks - When a task requires 3 or more distinct steps or actions
- Non-trivial and complex tasks - Tasks that require careful planning or multiple operations and potentially assigned to teammates
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
- Include enough detail in the description for another agent to understand and complete the task
- New tasks are created with status 'pending' and no owner - use TaskUpdate with the `owner` parameter to assign them
- Check TaskList first to avoid creating duplicate tasks
";

const TASK_GET_PROMPT_TS: &str = "Use this tool to retrieve a task by its ID from the task list.

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

const TASK_LIST_PROMPT_NO_TEAMS: &str = "Use this tool to list all tasks in the task list.

## When to Use This Tool

- To see what tasks are available to work on (status: 'pending', no owner, not blocked)
- To check overall progress on the project
- To find tasks that are blocked and need dependencies resolved
- After completing a task, to check for newly unblocked work or claim the next available task
- **Prefer working on tasks in ID order** (lowest ID first) when multiple tasks are available, as earlier tasks often set up context for later ones

## Output

Returns a summary of each task:
- **id**: Task identifier (use with TaskGet, TaskUpdate)
- **subject**: Brief description of the task
- **status**: 'pending', 'in_progress', or 'completed'
- **owner**: Agent ID if assigned, empty if available
- **blockedBy**: List of open task IDs that must be resolved first (tasks with blockedBy cannot be claimed until dependencies resolve)

Use TaskGet with a specific task ID to view full details including description and comments.
";

const TASK_LIST_PROMPT_TEAMS: &str = "Use this tool to list all tasks in the task list.

## When to Use This Tool

- To see what tasks are available to work on (status: 'pending', no owner, not blocked)
- To check overall progress on the project
- To find tasks that are blocked and need dependencies resolved
- Before assigning tasks to teammates, to see what's available
- After completing a task, to check for newly unblocked work or claim the next available task
- **Prefer working on tasks in ID order** (lowest ID first) when multiple tasks are available, as earlier tasks often set up context for later ones

## Output

Returns a summary of each task:
- **id**: Task identifier (use with TaskGet, TaskUpdate)
- **subject**: Brief description of the task
- **status**: 'pending', 'in_progress', or 'completed'
- **owner**: Agent ID if assigned, empty if available
- **blockedBy**: List of open task IDs that must be resolved first (tasks with blockedBy cannot be claimed until dependencies resolve)

Use TaskGet with a specific task ID to view full details including description and comments.

## Teammate Workflow

When working as a teammate:
1. After completing your current task, call TaskList to find available work
2. Look for tasks with status 'pending', no owner, and empty blockedBy
3. **Prefer tasks in ID order** (lowest ID first) when multiple tasks are available, as earlier tasks often set up context for later ones
4. Claim an available task using TaskUpdate (set `owner` to your name), or wait for leader assignment
5. If blocked, focus on unblocking tasks or notify the team lead
";

const TASK_UPDATE_PROMPT_TS: &str = "Use this tool to update a task in the task list.

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

/// Output is `{task: {id, subject}}` only. No description, metadata,
/// owner, etc.
#[tokio::test]
async fn test_task_create_output_shape_ts() {
    let ctx = ToolUseContext::test_default();
    let result = <TaskCreateTool as DynTool>::execute(
        &TaskCreateTool,
        json!({
            "subject": "write docs",
            "description": "update the CLAUDE.md",
            "activeForm": "Writing docs",
            "metadata": {"priority": "high"}
        }),
        &ctx,
    )
    .await
    .unwrap();

    // Exactly {task: {id, subject}} — no other keys leak.
    let task = result.data["task"].as_object().unwrap();
    assert_eq!(task.len(), 2, "task object should have exactly 2 keys");
    assert!(task["id"].is_string());
    assert_eq!(task["subject"], "write docs");
    // Fields that MUST NOT leak:
    assert!(
        task.get("description").is_none(),
        "description must not leak"
    );
    assert!(task.get("metadata").is_none(), "metadata must not leak");
    assert!(
        task.get("active_form").is_none() && task.get("activeForm").is_none(),
        "active_form must not leak"
    );
    assert!(task.get("output").is_none(), "output must not leak");
}

/// Returns wrapped `{task: {id, subject, description, status, blocks,
/// blockedBy} | null}`.
#[tokio::test]
async fn test_task_get_output_shape_ts() {
    let ctx = ToolUseContext::test_default();
    let create = <TaskCreateTool as DynTool>::execute(
        &TaskCreateTool,
        json!({"subject": "test get", "description": "test description"}),
        &ctx,
    )
    .await
    .unwrap();
    let tid = create.data["task"]["id"].as_str().unwrap().to_string();

    let result =
        <TaskGetTool as DynTool>::execute(&TaskGetTool, json!({"taskId": tid.clone()}), &ctx)
            .await
            .unwrap();

    // Wrapped in `task`.
    let task = result.data["task"].as_object().unwrap();
    assert_eq!(task["id"], json!(tid));
    assert_eq!(task["subject"], "test get");
    assert_eq!(task["description"], "test description");
    assert_eq!(task["status"], "pending");
    assert!(task["blocks"].is_array());
    assert!(task["blockedBy"].is_array());
    // owner/metadata/active_form/output must NOT leak.
    assert!(task.get("owner").is_none());
    assert!(task.get("metadata").is_none());
    assert!(task.get("output").is_none());
    assert!(task.get("active_form").is_none());
}

/// Unknown id returns `{task: null}`, not an error.
#[tokio::test]
async fn test_task_get_unknown_returns_null() {
    let ctx = ToolUseContext::test_default();
    let result =
        <TaskGetTool as DynTool>::execute(&TaskGetTool, json!({"taskId": "no-such-id"}), &ctx)
            .await
            .unwrap();
    assert!(result.data["task"].is_null());
}

/// Returns `{tasks: [...]}` with 5 fields per entry (id, subject, status,
/// owner?, blockedBy).
#[tokio::test]
async fn test_task_list_output_shape_ts() {
    let ctx = ToolUseContext::test_default();
    // Use unique subjects so we can find our tasks in a potentially
    // non-empty store.
    let unique = format!(
        "t10-list-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let _ = <TaskCreateTool as DynTool>::execute(
        &TaskCreateTool,
        json!({"subject": &unique, "description": "check list shape"}),
        &ctx,
    )
    .await
    .unwrap();

    let result = <TaskListTool as DynTool>::execute(&TaskListTool, json!({}), &ctx)
        .await
        .unwrap();

    let tasks = result.data["tasks"].as_array().unwrap();
    // Find our task by subject.
    let ours = tasks
        .iter()
        .find(|t| t["subject"].as_str() == Some(&unique))
        .expect("our task should be in the list");
    assert!(ours["id"].is_string());
    assert_eq!(ours["status"], "pending");
    assert!(ours["blockedBy"].is_array());
    // description/metadata/output/active_form must NOT leak.
    assert!(ours.get("description").is_none());
    assert!(ours.get("metadata").is_none());
    assert!(ours.get("output").is_none());
}

/// Filters tasks whose metadata has `_internal`.
#[tokio::test]
async fn test_task_list_filters_internal_tasks() {
    let ctx = ToolUseContext::test_default();
    // Create a visible task.
    let unique_visible = format!(
        "visible-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let _ = <TaskCreateTool as DynTool>::execute(
        &TaskCreateTool,
        json!({"subject": &unique_visible, "description": "x"}),
        &ctx,
    )
    .await
    .unwrap();
    // Create an internal task with _internal metadata.
    let unique_internal = format!(
        "internal-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let _ = <TaskCreateTool as DynTool>::execute(
        &TaskCreateTool,
        json!({
            "subject": &unique_internal,
            "description": "y",
            "metadata": {"_internal": true}
        }),
        &ctx,
    )
    .await
    .unwrap();

    let result = <TaskListTool as DynTool>::execute(&TaskListTool, json!({}), &ctx)
        .await
        .unwrap();
    let tasks = result.data["tasks"].as_array().unwrap();
    assert!(
        tasks
            .iter()
            .any(|t| t["subject"].as_str() == Some(&unique_visible)),
        "visible task should appear"
    );
    assert!(
        !tasks
            .iter()
            .any(|t| t["subject"].as_str() == Some(&unique_internal)),
        "_internal task must be filtered out"
    );
}

/// Output is `{success, taskId, updatedFields, statusChange?}`.
#[tokio::test]
async fn test_task_update_output_shape_ts() {
    let ctx = ToolUseContext::test_default();
    let create = <TaskCreateTool as DynTool>::execute(
        &TaskCreateTool,
        json!({"subject": "test update", "description": "start"}),
        &ctx,
    )
    .await
    .unwrap();
    let tid = create.data["task"]["id"].as_str().unwrap().to_string();

    let result = <TaskUpdateTool as DynTool>::execute(
        &TaskUpdateTool,
        json!({
            "taskId": tid.clone(),
            "status": "in_progress",
            "description": "new description"
        }),
        &ctx,
    )
    .await
    .unwrap();

    assert_eq!(result.data["success"], true);
    assert_eq!(result.data["taskId"], json!(tid));
    let updated: std::collections::HashSet<_> = result.data["updatedFields"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(updated.contains("status"));
    assert!(updated.contains("description"));
    // statusChange emitted when status changed.
    assert_eq!(result.data["statusChange"]["from"], "pending");
    assert_eq!(result.data["statusChange"]["to"], "in_progress");
}

/// Unknown task id → `{success: false, taskId, updatedFields: [], error}`.
#[tokio::test]
async fn test_task_update_unknown_id_shape() {
    let ctx = ToolUseContext::test_default();
    let result = <TaskUpdateTool as DynTool>::execute(
        &TaskUpdateTool,
        json!({"taskId": "no-such-task", "status": "completed"}),
        &ctx,
    )
    .await
    .unwrap();
    assert_eq!(result.data["success"], false);
    assert_eq!(result.data["taskId"], "no-such-task");
    assert!(result.data["error"].as_str().unwrap().contains("not found"));
}

// ---------------------------------------------------------------------------
// Phase C: app_state_patch — tools surface snapshots to AppState for the TUI.
// ---------------------------------------------------------------------------

/// TaskCreate returns an `app_state_patch` that fills `plan_tasks`,
/// sets `expanded_view = Tasks`.
#[tokio::test]
async fn test_task_create_emits_snapshot_and_auto_expand() {
    let ctx = ToolUseContext::test_default();
    let result = <TaskCreateTool as DynTool>::execute(
        &TaskCreateTool,
        json!({"subject": "panel item", "description": "x"}),
        &ctx,
    )
    .await
    .unwrap();

    let patch = result.app_state_patch.expect("patch must be emitted");
    let mut state = coco_types::ToolAppState::default();
    patch(&mut state);
    assert_eq!(state.plan_tasks.len(), 1);
    assert_eq!(state.plan_tasks[0].subject, "panel item");
    assert_eq!(state.expanded_view, coco_types::ExpandedView::Tasks);
    assert!(!state.verification_nudge_pending);
}

/// TaskUpdate on completion with all-done gate sets
/// `verification_nudge_pending = true`.
#[tokio::test]
async fn test_task_update_sets_verification_nudge_in_patch() {
    let mut ctx = ToolUseContext::test_default();
    ctx.agent_id = None; // main thread
    ctx.agent_catalog = Some(catalog_with_verification());
    let mut ids = Vec::new();
    for i in 0..3 {
        let r = <TaskCreateTool as DynTool>::execute(
            &TaskCreateTool,
            json!({"subject": format!("step {i}"), "description": ""}),
            &ctx,
        )
        .await
        .unwrap();
        ids.push(r.data["task"]["id"].as_str().unwrap().to_string());
    }
    for id in &ids[..2] {
        let _ = <TaskUpdateTool as DynTool>::execute(
            &TaskUpdateTool,
            json!({"taskId": id, "status": "completed"}),
            &ctx,
        )
        .await
        .unwrap();
    }
    // Final completion — patch must flip the nudge flag.
    let result = <TaskUpdateTool as DynTool>::execute(
        &TaskUpdateTool,
        json!({"taskId": ids[2], "status": "completed"}),
        &ctx,
    )
    .await
    .unwrap();
    let patch = result.app_state_patch.expect("patch must be emitted");
    let mut state = coco_types::ToolAppState::default();
    patch(&mut state);
    assert!(state.verification_nudge_pending);
    assert_eq!(state.expanded_view, coco_types::ExpandedView::Tasks);
}

/// TodoWrite emits a patch keyed by agent_id (or session fallback).
#[tokio::test]
async fn test_todo_write_emits_snapshot_keyed_by_agent() {
    use coco_types::AgentId;
    let mut ctx = ToolUseContext::test_default();
    ctx.agent_id = Some(AgentId::new("subagent-7"));

    let result = <TodoWriteTool as DynTool>::execute(
        &TodoWriteTool,
        json!({"todos": [
            {"content": "item", "status": "pending", "activeForm": "Doing it"}
        ]}),
        &ctx,
    )
    .await
    .unwrap();
    let patch = result.app_state_patch.expect("patch must be emitted");
    let mut state = coco_types::ToolAppState::default();
    patch(&mut state);
    let list = state
        .todos_by_agent
        .get("subagent-7")
        .expect("todos for subagent-7");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].content, "item");
    assert_eq!(list[0].active_form, "Doing it");
}

/// Plan-item completion is observed via `TaskGet`, not `TaskOutput`.
/// This locks in the separation: TaskOutput doesn't know
/// about plan items.
#[tokio::test]
async fn test_task_get_surfaces_completed_plan_item() {
    let ctx = ToolUseContext::test_default();
    let create = <TaskCreateTool as DynTool>::execute(
        &TaskCreateTool,
        json!({"subject": "terminal test", "description": "x"}),
        &ctx,
    )
    .await
    .unwrap();
    let tid = create.data["task"]["id"].as_str().unwrap().to_string();

    let _ = <TaskUpdateTool as DynTool>::execute(
        &TaskUpdateTool,
        json!({"taskId": tid.clone(), "status": "completed"}),
        &ctx,
    )
    .await
    .unwrap();

    let got = <TaskGetTool as DynTool>::execute(&TaskGetTool, json!({"taskId": tid}), &ctx)
        .await
        .unwrap();
    assert_eq!(got.data["task"]["status"], "completed");
}

// ---------------------------------------------------------------------------
// Phase 5: deleted status, auto-owner, blockedBy filter,
// verification nudge, mailbox notify on owner change.
// ---------------------------------------------------------------------------

use coco_tool_runtime::InboxMessage;
use coco_tool_runtime::MailboxEnvelope;
use coco_tool_runtime::MailboxHandle;

/// `status=deleted` permanently removes the task.
#[tokio::test]
async fn test_task_update_delete_status_removes_task() {
    let ctx = ToolUseContext::test_default();
    let create = <TaskCreateTool as DynTool>::execute(
        &TaskCreateTool,
        json!({"subject": "to delete", "description": "x"}),
        &ctx,
    )
    .await
    .unwrap();
    let tid = create.data["task"]["id"].as_str().unwrap().to_string();

    let result = <TaskUpdateTool as DynTool>::execute(
        &TaskUpdateTool,
        json!({"taskId": tid.clone(), "status": "deleted"}),
        &ctx,
    )
    .await
    .unwrap();
    assert_eq!(result.data["success"], true);
    assert_eq!(result.data["statusChange"]["to"], "deleted");

    // Subsequent get must return null.
    let got = <TaskGetTool as DynTool>::execute(&TaskGetTool, json!({"taskId": tid}), &ctx)
        .await
        .unwrap();
    assert!(got.data["task"].is_null(), "deleted task should vanish");
}

/// TaskList filters resolved blockers from `blockedBy` so the model only
/// sees currently-active dependencies.
#[tokio::test]
async fn test_task_list_filters_resolved_blockers_from_blocked_by() {
    let ctx = ToolUseContext::test_default();
    let a = <TaskCreateTool as DynTool>::execute(
        &TaskCreateTool,
        json!({"subject": "a", "description": ""}),
        &ctx,
    )
    .await
    .unwrap();
    let b = <TaskCreateTool as DynTool>::execute(
        &TaskCreateTool,
        json!({"subject": "b", "description": ""}),
        &ctx,
    )
    .await
    .unwrap();
    let a_id = a.data["task"]["id"].as_str().unwrap().to_string();
    let b_id = b.data["task"]["id"].as_str().unwrap().to_string();

    // Set: a blocks b → b.blockedBy = [a].
    let _ = <TaskUpdateTool as DynTool>::execute(
        &TaskUpdateTool,
        json!({"taskId": b_id.clone(), "addBlockedBy": [a_id.clone()]}),
        &ctx,
    )
    .await
    .unwrap();

    // Before resolving a, b's blockedBy contains a.
    let list = <TaskListTool as DynTool>::execute(&TaskListTool, json!({}), &ctx)
        .await
        .unwrap();
    let b_entry = list.data["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == json!(b_id))
        .unwrap()
        .clone();
    assert_eq!(
        b_entry["blockedBy"].as_array().unwrap().len(),
        1,
        "b should be blocked by a before a completes"
    );

    // After a completes, b.blockedBy should be filtered out.
    let _ = <TaskUpdateTool as DynTool>::execute(
        &TaskUpdateTool,
        json!({"taskId": a_id, "status": "completed"}),
        &ctx,
    )
    .await
    .unwrap();
    let list = <TaskListTool as DynTool>::execute(&TaskListTool, json!({}), &ctx)
        .await
        .unwrap();
    let b_entry = list.data["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == json!(b_id))
        .unwrap()
        .clone();
    assert!(
        b_entry["blockedBy"].as_array().unwrap().is_empty(),
        "b.blockedBy should be empty once a is completed"
    );
}

/// Verification nudge fires when main thread closes out 3+ tasks and
/// none match /verif/i.
#[tokio::test]
async fn test_task_update_verification_nudge_main_thread_all_done() {
    let mut ctx = ToolUseContext::test_default();
    ctx.agent_id = None; // main thread
    ctx.agent_catalog = Some(catalog_with_verification());
    let mut ids = Vec::new();
    for i in 0..3 {
        let created = <TaskCreateTool as DynTool>::execute(
            &TaskCreateTool,
            json!({"subject": format!("step {i}"), "description": ""}),
            &ctx,
        )
        .await
        .unwrap();
        ids.push(created.data["task"]["id"].as_str().unwrap().to_string());
    }

    // Complete the first two with no nudge yet.
    for id in ids.iter().take(2) {
        let res = <TaskUpdateTool as DynTool>::execute(
            &TaskUpdateTool,
            json!({"taskId": id, "status": "completed"}),
            &ctx,
        )
        .await
        .unwrap();
        assert_eq!(res.data["verificationNudgeNeeded"], false);
    }

    // Final completion → all done, none match verify → nudge.
    let res = <TaskUpdateTool as DynTool>::execute(
        &TaskUpdateTool,
        json!({"taskId": ids[2], "status": "completed"}),
        &ctx,
    )
    .await
    .unwrap();
    assert_eq!(res.data["verificationNudgeNeeded"], true);
}

/// Nudge skipped when a verification task exists.
#[tokio::test]
async fn test_task_update_verification_nudge_skipped_when_verify_task_exists() {
    let mut ctx = ToolUseContext::test_default();
    ctx.agent_id = None;
    let mut ids = Vec::new();
    for subject in ["impl", "test", "Verify output"] {
        let created = <TaskCreateTool as DynTool>::execute(
            &TaskCreateTool,
            json!({"subject": subject, "description": ""}),
            &ctx,
        )
        .await
        .unwrap();
        ids.push(created.data["task"]["id"].as_str().unwrap().to_string());
    }
    for id in &ids[..2] {
        let _ = <TaskUpdateTool as DynTool>::execute(
            &TaskUpdateTool,
            json!({"taskId": id, "status": "completed"}),
            &ctx,
        )
        .await
        .unwrap();
    }
    let res = <TaskUpdateTool as DynTool>::execute(
        &TaskUpdateTool,
        json!({"taskId": ids[2], "status": "completed"}),
        &ctx,
    )
    .await
    .unwrap();
    assert_eq!(
        res.data["verificationNudgeNeeded"], false,
        "verify task present → no nudge"
    );
}

/// Nudge gate: subagent context (agent_id set) never receives the nudge.
#[tokio::test]
async fn test_task_update_verification_nudge_skipped_in_subagent() {
    use coco_types::AgentId;
    let mut ctx = ToolUseContext::test_default();
    ctx.agent_id = Some(AgentId::new("subagent-1"));
    let mut ids = Vec::new();
    for i in 0..3 {
        let created = <TaskCreateTool as DynTool>::execute(
            &TaskCreateTool,
            json!({"subject": format!("step {i}"), "description": ""}),
            &ctx,
        )
        .await
        .unwrap();
        ids.push(created.data["task"]["id"].as_str().unwrap().to_string());
    }
    for id in &ids[..2] {
        let _ = <TaskUpdateTool as DynTool>::execute(
            &TaskUpdateTool,
            json!({"taskId": id, "status": "completed"}),
            &ctx,
        )
        .await
        .unwrap();
    }
    let res = <TaskUpdateTool as DynTool>::execute(
        &TaskUpdateTool,
        json!({"taskId": ids[2], "status": "completed"}),
        &ctx,
    )
    .await
    .unwrap();
    assert_eq!(
        res.data["verificationNudgeNeeded"], false,
        "subagents never receive the nudge"
    );
}

// Recording mailbox for test assertions.
struct RecordingMailbox {
    written: std::sync::Mutex<Vec<(String, String, MailboxEnvelope)>>,
}

impl RecordingMailbox {
    fn new() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            written: std::sync::Mutex::new(Vec::new()),
        })
    }
    fn calls(&self) -> Vec<(String, String, MailboxEnvelope)> {
        self.written.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl MailboxHandle for RecordingMailbox {
    async fn write_to_mailbox(
        &self,
        recipient: &str,
        team_name: &str,
        message: MailboxEnvelope,
    ) -> Result<(), coco_error::BoxedError> {
        self.written
            .lock()
            .unwrap()
            .push((recipient.to_string(), team_name.to_string(), message));
        Ok(())
    }
    async fn read_unread(
        &self,
        _agent_name: &str,
        _team_name: &str,
    ) -> Result<Vec<InboxMessage>, coco_error::BoxedError> {
        Ok(Vec::new())
    }
    async fn mark_read(
        &self,
        _agent_name: &str,
        _team_name: &str,
        _index: usize,
    ) -> Result<(), coco_error::BoxedError> {
        Ok(())
    }
}

/// Setting an owner in a teammate context writes a `task_assignment`
/// envelope to the new owner's mailbox.
#[tokio::test]
async fn test_task_update_owner_change_writes_mailbox() {
    let mailbox = RecordingMailbox::new();
    let mut ctx = ToolUseContext::test_default();
    ctx.is_teammate = true;
    ctx.agent_name = Some("leader".into());
    ctx.team_name = Some("alpha-team".into());
    ctx.mailbox = mailbox.clone();

    let created = <TaskCreateTool as DynTool>::execute(
        &TaskCreateTool,
        json!({"subject": "fix bug", "description": "find root cause"}),
        &ctx,
    )
    .await
    .unwrap();
    let tid = created.data["task"]["id"].as_str().unwrap().to_string();

    let _ = <TaskUpdateTool as DynTool>::execute(
        &TaskUpdateTool,
        json!({"taskId": tid.clone(), "owner": "bob"}),
        &ctx,
    )
    .await
    .unwrap();

    let calls = mailbox.calls();
    assert_eq!(calls.len(), 1, "exactly one mailbox write expected");
    assert_eq!(calls[0].0, "bob", "recipient = new owner");
    assert_eq!(calls[0].1, "alpha-team");
    let payload: serde_json::Value = serde_json::from_str(&calls[0].2.text).unwrap();
    assert_eq!(payload["type"], "task_assignment");
    assert_eq!(payload["taskId"], json!(tid));
    assert_eq!(payload["assignedBy"], "leader");
}

/// Auto-owner: when a teammate marks a task in_progress without an
/// explicit owner and the task is unclaimed, the teammate auto-claims it.
#[tokio::test]
async fn test_task_update_auto_owner_on_in_progress() {
    let mailbox = RecordingMailbox::new();
    let mut ctx = ToolUseContext::test_default();
    ctx.is_teammate = true;
    ctx.agent_name = Some("alice".into());
    ctx.team_name = Some("alpha-team".into());
    ctx.mailbox = mailbox.clone();

    let created = <TaskCreateTool as DynTool>::execute(
        &TaskCreateTool,
        json!({"subject": "claim me", "description": "x"}),
        &ctx,
    )
    .await
    .unwrap();
    let tid = created.data["task"]["id"].as_str().unwrap().to_string();

    let res = <TaskUpdateTool as DynTool>::execute(
        &TaskUpdateTool,
        json!({"taskId": tid.clone(), "status": "in_progress"}),
        &ctx,
    )
    .await
    .unwrap();
    let has_owner = res.data["updatedFields"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v.as_str() == Some("owner"));
    assert!(
        has_owner,
        "auto-owner should be in updatedFields: {:?}",
        res.data["updatedFields"]
    );

    let got = <TaskGetTool as DynTool>::execute(&TaskGetTool, json!({"taskId": tid}), &ctx)
        .await
        .unwrap();
    let _ = got; // owner isn't projected into TaskGet output; validate via mailbox write
    let calls = mailbox.calls();
    assert_eq!(calls.len(), 1, "auto-owner must also trigger mailbox");
    assert_eq!(calls[0].0, "alice");
}

/// Auto-owner does NOT kick in for non-teammate contexts.
#[tokio::test]
async fn test_task_update_auto_owner_skipped_outside_swarm() {
    let mut ctx = ToolUseContext::test_default();
    ctx.is_teammate = false;
    ctx.agent_name = Some("alice".into());

    let created = <TaskCreateTool as DynTool>::execute(
        &TaskCreateTool,
        json!({"subject": "x", "description": ""}),
        &ctx,
    )
    .await
    .unwrap();
    let tid = created.data["task"]["id"].as_str().unwrap().to_string();

    let res = <TaskUpdateTool as DynTool>::execute(
        &TaskUpdateTool,
        json!({"taskId": tid, "status": "in_progress"}),
        &ctx,
    )
    .await
    .unwrap();
    let has_owner = res.data["updatedFields"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v.as_str() == Some("owner"));
    assert!(!has_owner, "non-teammate must not auto-claim");
}

// ---------------------------------------------------------------------------
// TodoWrite render_for_model
// ---------------------------------------------------------------------------

mod todo_write_render_tests {
    use crate::tools::task_tools::TodoWriteTool;
    use coco_tool_runtime::DynTool;

    use coco_tool_runtime::ToolResultContentPart;
    use serde_json::json;

    #[test]
    fn render_emits_base_message_when_no_nudge() {
        let data = json!({
            "oldTodos": [],
            "newTodos": [{"content": "task", "status": "pending", "activeForm": "Doing task"}],
            "verificationNudgeNeeded": false,
        });
        let parts = <TodoWriteTool as DynTool>::render_for_model(&TodoWriteTool, &data);
        let ToolResultContentPart::Text { text, .. } = &parts[0] else {
            panic!("expected Text part");
        };
        assert!(text.starts_with("Todos have been modified successfully."));
        assert!(!text.contains("verification agent"));
    }

    #[test]
    fn render_appends_verification_nudge_when_needed() {
        let data = json!({
            "oldTodos": [],
            "newTodos": [],
            "verificationNudgeNeeded": true,
        });
        let parts = <TodoWriteTool as DynTool>::render_for_model(&TodoWriteTool, &data);
        let ToolResultContentPart::Text { text, .. } = &parts[0] else {
            panic!("expected Text part");
        };
        assert!(text.contains("Todos have been modified successfully."));
        assert!(text.contains("verification agent"));
    }
}

// ── render_for_model — Task* tools ────────────────────────────────────

mod task_render_tests {
    use crate::tools::task_tools::TaskCreateTool;
    use crate::tools::task_tools::TaskGetTool;
    use crate::tools::task_tools::TaskListTool;
    use crate::tools::task_tools::TaskOutputTool;
    use crate::tools::task_tools::TaskStopTool;
    use crate::tools::task_tools::TaskUpdateTool;
    use coco_tool_runtime::DynTool;

    use coco_tool_runtime::ToolResultContentPart;
    use serde_json::json;

    #[test]
    fn task_create_render_emits_subject_line() {
        let data = json!({"task": {"id": "t-1", "subject": "Investigate auth"}});
        let parts = <TaskCreateTool as DynTool>::render_for_model(&TaskCreateTool, &data);
        let ToolResultContentPart::Text { text, .. } = &parts[0] else {
            panic!("expected Text part");
        };
        assert_eq!(text, "Task #t-1 created successfully: Investigate auth");
    }

    #[test]
    fn task_get_render_found_includes_status_and_blockers() {
        let data = json!({
            "task": {
                "id": "t-1",
                "subject": "Refactor auth",
                "description": "Replace legacy middleware",
                "status": "in_progress",
                "blocks": [],
                "blockedBy": ["t-2", "t-3"],
            }
        });
        let parts = <TaskGetTool as DynTool>::render_for_model(&TaskGetTool, &data);
        let ToolResultContentPart::Text { text, .. } = &parts[0] else {
            panic!("expected Text part");
        };
        assert!(text.starts_with("Task #t-1: Refactor auth"), "got: {text}");
        assert!(text.contains("Status: in_progress"));
        assert!(text.contains("Description: Replace legacy middleware"));
        assert!(text.contains("Blocked by: #t-2, #t-3"));
    }

    #[test]
    fn task_get_render_not_found() {
        let data = json!({"task": null});
        let parts = <TaskGetTool as DynTool>::render_for_model(&TaskGetTool, &data);
        let ToolResultContentPart::Text { text, .. } = &parts[0] else {
            panic!("expected Text part");
        };
        assert_eq!(text, "Task not found");
    }

    #[test]
    fn task_list_render_empty() {
        let data = json!({"tasks": []});
        let parts = <TaskListTool as DynTool>::render_for_model(&TaskListTool, &data);
        let ToolResultContentPart::Text { text, .. } = &parts[0] else {
            panic!("expected Text part");
        };
        assert_eq!(text, "No tasks found");
    }

    #[test]
    fn task_list_render_summarizes_tasks() {
        let data = json!({
            "tasks": [
                {"id": "t-1", "subject": "First", "status": "pending", "blockedBy": []},
                {"id": "t-2", "subject": "Second", "status": "in_progress", "blockedBy": []},
            ]
        });
        let parts = <TaskListTool as DynTool>::render_for_model(&TaskListTool, &data);
        let ToolResultContentPart::Text { text, .. } = &parts[0] else {
            panic!("expected Text part");
        };
        // Shape: `#{id} [{status}] {subject}` per line.
        assert!(text.contains("#t-1 [pending] First"), "got: {text}");
        assert!(text.contains("#t-2 [in_progress] Second"), "got: {text}");
    }

    #[test]
    fn task_list_render_includes_owner_and_blockers() {
        let data = json!({
            "tasks": [
                {
                    "id": "t-3",
                    "subject": "Pair task",
                    "status": "pending",
                    "blockedBy": ["t-1", "t-2"],
                    "owner": "alice",
                },
            ]
        });
        let parts = <TaskListTool as DynTool>::render_for_model(&TaskListTool, &data);
        let ToolResultContentPart::Text { text, .. } = &parts[0] else {
            panic!("expected Text part");
        };
        assert_eq!(
            text,
            "#t-3 [pending] Pair task (alice) [blocked by #t-1, #t-2]"
        );
    }

    #[test]
    fn task_update_render_success_lists_fields() {
        let data = json!({
            "success": true,
            "taskId": "t-1",
            "updatedFields": ["status", "owner"],
            "verificationNudgeNeeded": false,
            "statusChange": {"from": "pending", "to": "in_progress"},
        });
        let parts = <TaskUpdateTool as DynTool>::render_for_model(&TaskUpdateTool, &data);
        let ToolResultContentPart::Text { text, .. } = &parts[0] else {
            panic!("expected Text part");
        };
        // Shape: `Updated task #{id} {fields}` (status change not included
        // in the render).
        assert_eq!(text, "Updated task #t-1 status, owner");
    }

    #[test]
    fn task_update_render_appends_verification_nudge() {
        let data = json!({
            "success": true,
            "taskId": "t-1",
            "updatedFields": ["status"],
            "verificationNudgeNeeded": true,
            "statusChange": {"from": "in_progress", "to": "completed"},
        });
        let parts = <TaskUpdateTool as DynTool>::render_for_model(&TaskUpdateTool, &data);
        let ToolResultContentPart::Text { text, .. } = &parts[0] else {
            panic!("expected Text part");
        };
        assert!(text.starts_with("Updated task #t-1 status"));
        assert!(text.contains("verification agent (subagent_type=\"verification-agent\")"));
        assert!(text.contains("only the verifier issues a verdict"));
    }

    #[test]
    fn task_update_render_appends_teammate_completed_nudge() {
        // When a swarm teammate transitions a task to completed, append
        // the "Call TaskList now" nudge so the agent picks up downstream
        // work.
        let data = json!({
            "success": true,
            "taskId": "t-1",
            "updatedFields": ["status"],
            "verificationNudgeNeeded": false,
            "completedNudgeNeeded": true,
            "statusChange": {"from": "in_progress", "to": "completed"},
        });
        let parts = <TaskUpdateTool as DynTool>::render_for_model(&TaskUpdateTool, &data);
        let ToolResultContentPart::Text { text, .. } = &parts[0] else {
            panic!("expected Text part");
        };
        assert!(text.starts_with("Updated task #t-1 status"));
        assert!(text.contains("Task completed. Call TaskList now"));
        assert!(text.contains("see if your work unblocked others"));
    }

    #[test]
    fn task_update_render_completed_then_verification_nudges_in_order() {
        // Both nudges fire when a teammate completes their 3rd+ task
        // without verification. Completed nudge precedes verification
        // nudge in the render output.
        let data = json!({
            "success": true,
            "taskId": "t-9",
            "updatedFields": ["status"],
            "completedNudgeNeeded": true,
            "verificationNudgeNeeded": true,
            "statusChange": {"from": "in_progress", "to": "completed"},
        });
        let parts = <TaskUpdateTool as DynTool>::render_for_model(&TaskUpdateTool, &data);
        let ToolResultContentPart::Text { text, .. } = &parts[0] else {
            panic!("expected Text part");
        };
        let completed_idx = text.find("Task completed").expect("completed nudge");
        let verify_idx = text.find("verification agent").expect("verification nudge");
        assert!(
            completed_idx < verify_idx,
            "completed nudge must precede verification nudge: {text}"
        );
    }

    #[test]
    fn task_update_render_omits_completed_nudge_when_flag_false() {
        let data = json!({
            "success": true,
            "taskId": "t-1",
            "updatedFields": ["status"],
            "completedNudgeNeeded": false,
            "verificationNudgeNeeded": false,
            "statusChange": {"from": "in_progress", "to": "completed"},
        });
        let parts = <TaskUpdateTool as DynTool>::render_for_model(&TaskUpdateTool, &data);
        let ToolResultContentPart::Text { text, .. } = &parts[0] else {
            panic!("expected Text part");
        };
        assert_eq!(text, "Updated task #t-1 status");
    }

    #[test]
    fn task_update_render_error_uses_error_field_directly() {
        let data = json!({
            "success": false,
            "taskId": "t-99",
            "updatedFields": [],
            "error": "Permission denied",
        });
        let parts = <TaskUpdateTool as DynTool>::render_for_model(&TaskUpdateTool, &data);
        let ToolResultContentPart::Text { text, .. } = &parts[0] else {
            panic!("expected Text part");
        };
        assert_eq!(text, "Permission denied");
    }

    #[test]
    fn task_update_render_error_falls_back_to_not_found() {
        let data = json!({"success": false, "taskId": "t-99", "updatedFields": []});
        let parts = <TaskUpdateTool as DynTool>::render_for_model(&TaskUpdateTool, &data);
        let ToolResultContentPart::Text { text, .. } = &parts[0] else {
            panic!("expected Text part");
        };
        assert_eq!(text, "Task #t-99 not found");
    }

    #[test]
    fn task_stop_render_uses_default_json_impl() {
        // Emits the full envelope as JSON — matches the trait default
        // exactly. No override.
        let data = json!({
            "message": "Successfully stopped task: bg-1",
            "task_id": "bg-1",
            "task_type": "background",
        });
        let parts = <TaskStopTool as DynTool>::render_for_model(&TaskStopTool, &data);
        let ToolResultContentPart::Text { text, .. } = &parts[0] else {
            panic!("expected Text part");
        };
        // Default impl JSON-stringifies the whole envelope.
        assert!(text.starts_with("{"), "got: {text}");
        assert!(text.contains("Successfully stopped task: bg-1"));
        assert!(text.contains("\"task_id\":\"bg-1\""));
    }

    #[test]
    fn task_output_render_success_emits_xml_tagged_block() {
        let data = json!({
            "retrieval_status": "success",
            "task": {
                "task_id": "bg-1",
                "task_type": "background",
                "status": "completed",
                "description": "",
                "output": "stdout line 1\nstdout line 2",
                "exitCode": 0,
            }
        });
        let parts = <TaskOutputTool as DynTool>::render_for_model(&TaskOutputTool, &data);
        let ToolResultContentPart::Text { text, .. } = &parts[0] else {
            panic!("expected Text part");
        };
        assert_eq!(
            text,
            "<retrieval_status>success</retrieval_status>\n\n\
             <task_id>bg-1</task_id>\n\n\
             <task_type>background</task_type>\n\n\
             <status>completed</status>\n\n\
             <exit_code>0</exit_code>\n\n\
             <output>\nstdout line 1\nstdout line 2\n</output>"
        );
    }

    #[test]
    fn task_output_render_not_ready_emits_just_status_tag() {
        let data = json!({
            "retrieval_status": "not_ready",
            "task": {"task_id": "bg-2", "task_type": "background", "status": "unknown", "description": "", "output": ""}
        });
        let parts = <TaskOutputTool as DynTool>::render_for_model(&TaskOutputTool, &data);
        let ToolResultContentPart::Text { text, .. } = &parts[0] else {
            panic!("expected Text part");
        };
        assert!(text.starts_with("<retrieval_status>not_ready</retrieval_status>"));
        assert!(text.contains("<task_id>bg-2</task_id>"));
        // Empty output is suppressed (blank `output` field is not rendered).
        assert!(!text.contains("<output>"), "got: {text}");
    }

    #[test]
    fn task_output_render_skips_exit_code_when_absent() {
        let data = json!({
            "retrieval_status": "success",
            "task": {
                "task_id": "bg-3",
                "task_type": "agent",
                "status": "completed",
                "output": "result",
            }
        });
        let parts = <TaskOutputTool as DynTool>::render_for_model(&TaskOutputTool, &data);
        let ToolResultContentPart::Text { text, .. } = &parts[0] else {
            panic!("expected Text part");
        };
        assert!(!text.contains("<exit_code>"), "got: {text}");
        assert!(text.contains("<output>\nresult\n</output>"));
    }
}

// ── TaskV2 feature gate ───────────────────────────────────────────────
//
// V1/V2 mutual exclusion at tool level. When `Feature::TaskV2` is on
// (default), V2 tools are exposed and TodoWrite is hidden; when off,
// the inverse. `TaskOutput` and `TaskStop` operate on the
// background-task namespace (Bash `run_in_background`, agent spawns)
// and stay enabled either way — they're orthogonal to the V1/V2
// plan-item dichotomy.

fn ctx_with_task_v2(enabled: bool) -> ToolUseContext {
    let mut features = coco_types::Features::with_defaults();
    features.set_enabled(coco_types::Feature::TaskV2, enabled);
    let mut ctx = ToolUseContext::test_default();
    ctx.features = Arc::new(features);
    ctx
}

#[test]
fn task_v2_on_exposes_v2_hides_todo_write() {
    let ctx = ctx_with_task_v2(true);
    assert!(
        !<TodoWriteTool as DynTool>::is_enabled(&TodoWriteTool, &ctx),
        "V2 mode → TodoWrite hidden"
    );
    assert!(<TaskCreateTool as DynTool>::is_enabled(
        &TaskCreateTool,
        &ctx
    ));
    assert!(<TaskGetTool as DynTool>::is_enabled(&TaskGetTool, &ctx));
    assert!(<TaskListTool as DynTool>::is_enabled(&TaskListTool, &ctx));
    assert!(<TaskUpdateTool as DynTool>::is_enabled(
        &TaskUpdateTool,
        &ctx
    ));
    // Background-task tools unaffected by the V1/V2 gate.
    assert!(<TaskOutputTool as DynTool>::is_enabled(
        &TaskOutputTool,
        &ctx
    ));
    assert!(<TaskStopTool as DynTool>::is_enabled(&TaskStopTool, &ctx));
}

#[test]
fn task_v2_off_exposes_todo_write_hides_v2() {
    let ctx = ctx_with_task_v2(false);
    assert!(
        <TodoWriteTool as DynTool>::is_enabled(&TodoWriteTool, &ctx),
        "V1 mode → TodoWrite shown"
    );
    assert!(!<TaskCreateTool as DynTool>::is_enabled(
        &TaskCreateTool,
        &ctx
    ));
    assert!(!<TaskGetTool as DynTool>::is_enabled(&TaskGetTool, &ctx));
    assert!(!<TaskListTool as DynTool>::is_enabled(&TaskListTool, &ctx));
    assert!(!<TaskUpdateTool as DynTool>::is_enabled(
        &TaskUpdateTool,
        &ctx
    ));
    // Background-task tools unaffected by the V1/V2 gate.
    assert!(<TaskOutputTool as DynTool>::is_enabled(
        &TaskOutputTool,
        &ctx
    ));
    assert!(<TaskStopTool as DynTool>::is_enabled(&TaskStopTool, &ctx));
}
