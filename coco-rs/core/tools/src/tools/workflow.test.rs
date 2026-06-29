use coco_tool_runtime::TaskHandle;
use coco_tool_runtime::Tool;
use coco_tool_runtime::ToolError;
use coco_tool_runtime::ToolUseContext;
use coco_tool_runtime::WorkflowTaskRequest;
use coco_types::Feature;
use coco_types::Features;
use coco_types::PermissionRule;
use coco_types::PermissionRuleSource;
use coco_types::PermissionRuleValue;
use coco_types::ToolCheckResult;
use coco_types::ToolId;
use coco_types::ToolName;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use tokio_util::sync::CancellationToken;

use super::WorkflowInput;
use super::WorkflowTool;

#[derive(Default)]
struct RecordingTaskHandle {
    registered: Mutex<Vec<WorkflowTaskRequest>>,
    progress: Mutex<Vec<coco_types::WorkflowProgressEvent>>,
    completed: Mutex<Vec<coco_tool_runtime::AgentCompletionPayload>>,
    notify_progress: tokio::sync::Notify,
    notify_completed: tokio::sync::Notify,
}

#[async_trait::async_trait]
impl TaskHandle for RecordingTaskHandle {
    async fn register_workflow_task(
        &self,
        request: WorkflowTaskRequest,
        _cancel: CancellationToken,
    ) -> String {
        let task_id = request.task_id.clone();
        self.registered
            .lock()
            .expect("registered lock")
            .push(request);
        task_id
    }

    async fn output_file_path(&self, task_id: &str) -> Option<PathBuf> {
        Some(PathBuf::from(format!("/tmp/{task_id}.out")))
    }

    async fn push_workflow_progress(
        &self,
        _task_id: &str,
        event: coco_types::WorkflowProgressEvent,
    ) {
        self.progress.lock().expect("progress lock").push(event);
        self.notify_progress.notify_waiters();
    }

    async fn mark_completed(
        &self,
        _task_id: &str,
        payload: coco_tool_runtime::AgentCompletionPayload,
    ) {
        self.completed.lock().expect("completed lock").push(payload);
        self.notify_completed.notify_waiters();
    }
}

/// A task-handle mock that exposes a single canned `local_workflow` row so the
/// resume path can be exercised (run_id → task lookup, errorCode-3 precedence).
struct ResumeLookupTaskHandle {
    row: coco_types::TaskStateBase,
}

fn workflow_row(
    task_id: &str,
    run_id: &str,
    status: coco_types::TaskStatus,
    output_file: Option<&str>,
) -> coco_types::TaskStateBase {
    coco_types::TaskStateBase {
        id: task_id.to_string(),
        status,
        notified: false,
        description: "wf".to_string(),
        tool_use_id: None,
        start_time: 0,
        end_time: None,
        killed_by: None,
        total_paused_ms: None,
        output_file: output_file.map(str::to_string),
        output_offset: 0,
        extras: coco_types::TaskExtras::local_workflow(
            run_id.to_string(),
            Some("wf".to_string()),
            None,
        ),
    }
}

#[async_trait::async_trait]
impl TaskHandle for ResumeLookupTaskHandle {
    async fn list_tasks(&self) -> Vec<coco_types::TaskStateBase> {
        vec![self.row.clone()]
    }

    async fn output_file_path(&self, task_id: &str) -> Option<PathBuf> {
        self.row
            .output_file
            .as_deref()
            .filter(|_| task_id == self.row.id)
            .map(PathBuf::from)
    }
}

#[tokio::test]
async fn workflow_resume_of_running_run_rejects_with_taskstop_hint() {
    // errorCode-3: a resume whose run id maps to a still-running local_workflow
    // task is rejected before any launch, with the TaskStop hint.
    let tool = WorkflowTool;
    let mut ctx = ToolUseContext::test_default();
    let handle = Arc::new(ResumeLookupTaskHandle {
        row: workflow_row(
            "wtask01",
            "wf_running01",
            coco_types::TaskStatus::Running,
            None,
        ),
    });
    let task_handle: coco_tool_runtime::TaskHandleRef = handle;
    ctx.task_handle = Some(task_handle);

    let input = WorkflowInput {
        resume_from_run_id: Some("wf_running01".to_string()),
        ..WorkflowInput::default()
    };
    let err = tool.execute(input, &ctx).await.unwrap_err();
    match err {
        ToolError::InvalidInput {
            message,
            error_code,
        } => {
            assert_eq!(
                error_code.as_deref(),
                Some(super::WorkflowValidationCode::ResumeRunning.as_str())
            );
            assert!(message.contains("is still running"));
            assert!(message.contains("task wtask01"));
            assert!(message.contains("TaskStop"));
        }
        other => panic!("expected InvalidInput with ResumeRunning code, got {other:?}"),
    }
}

#[tokio::test]
async fn workflow_resume_of_unknown_run_rejects_same_session_only() {
    // A run id with no matching in-session row cannot be resumed (same-session
    // only) — surfaced as a typed source error, not a launch.
    let tool = WorkflowTool;
    let mut ctx = ToolUseContext::test_default();
    let handle = Arc::new(ResumeLookupTaskHandle {
        row: workflow_row(
            "wtask01",
            "wf_known001",
            coco_types::TaskStatus::Completed,
            None,
        ),
    });
    let task_handle: coco_tool_runtime::TaskHandleRef = handle;
    ctx.task_handle = Some(task_handle);

    let input = WorkflowInput {
        resume_from_run_id: Some("wf_absent001".to_string()),
        ..WorkflowInput::default()
    };
    let err = tool.execute(input, &ctx).await.unwrap_err();
    match err {
        ToolError::InvalidInput { error_code, .. } => {
            assert_eq!(
                error_code.as_deref(),
                Some(super::WorkflowValidationCode::SourceError.as_str())
            );
        }
        other => panic!("expected InvalidInput with SourceError code, got {other:?}"),
    }
}

#[test]
fn workflow_tool_identity_and_alias_match_contract() {
    let tool = WorkflowTool;

    assert_eq!(tool.id(), ToolId::Builtin(ToolName::Workflow));
    assert_eq!(tool.name(), "Workflow");
    assert_eq!(tool.aliases(), ["RunWorkflow"]);
}

#[test]
fn workflow_tool_is_feature_gated() {
    let tool = WorkflowTool;
    let mut ctx = ToolUseContext::test_default();

    // Default-on.
    assert!(tool.is_enabled(&ctx));

    // Explicitly disabled → filtered out of the model's tool list.
    let mut features = Features::with_defaults();
    features.disable(Feature::Workflow);
    ctx.features = Arc::new(features);
    assert!(!tool.is_enabled(&ctx));
}

#[tokio::test]
async fn workflow_execute_without_task_runtime_errors() {
    // `test_default` has no `task_handle`, so a valid script cannot be
    // launched and the tool reports the missing background runtime.
    let tool = WorkflowTool;
    let ctx = ToolUseContext::test_default();
    let input = WorkflowInput {
        script: Some("export const meta = { name: 'x', description: 'y' };".into()),
        ..WorkflowInput::default()
    };

    let err = tool.execute(input, &ctx).await.unwrap_err();
    assert!(matches!(err, ToolError::ExecutionFailed { .. }));
    assert!(err.to_string().contains("Background tasks"));
}

#[tokio::test]
async fn workflow_execute_launches_background_task() {
    let tool = WorkflowTool;
    let mut ctx = ToolUseContext::test_default();
    ctx.tool_use_id = Some("toolu_workflow".to_string());
    let handle = Arc::new(RecordingTaskHandle::default());
    let task_handle: coco_tool_runtime::TaskHandleRef = handle.clone();
    ctx.task_handle = Some(task_handle);

    let result = tool
        .execute(
            WorkflowInput {
                script: Some(
                    "export const meta = { name: 'launch', description: 'test' };\n\
                     log('queued');\n\
                     return { ok: args.ok };"
                        .to_string(),
                ),
                args: serde_json::json!({"ok": true}),
                ..WorkflowInput::default()
            },
            &ctx,
        )
        .await
        .expect("workflow launch");

    assert_eq!(result.data.status, "async_launched");
    assert_eq!(result.data.task_type, "local_workflow");
    assert_eq!(result.data.workflow_name.as_deref(), Some("launch"));
    assert!(result.data.task_id.starts_with('w'));
    assert!(result.data.run_id.starts_with("wf_"));
    let output_file = format!("/tmp/{}.out", result.data.task_id);
    assert_eq!(
        result.data.output_file.as_deref(),
        Some(output_file.as_str())
    );

    {
        let registered = handle.registered.lock().expect("registered lock");
        assert_eq!(registered.len(), 1);
        assert_eq!(registered[0].task_id, result.data.task_id);
        assert_eq!(registered[0].workflow_name.as_deref(), Some("launch"));
        assert_eq!(registered[0].tool_use_id.as_deref(), Some("toolu_workflow"));
        assert!(registered[0].script.contains("export const meta"));
    }

    let completed_empty = handle.completed.lock().expect("completed lock").is_empty();
    if completed_empty {
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            handle.notify_completed.notified(),
        )
        .await
        .expect("workflow completes");
    }
    let progress_empty = handle.progress.lock().expect("progress lock").is_empty();
    if progress_empty {
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            handle.notify_progress.notified(),
        )
        .await
        .expect("workflow progress arrives");
    }
    let progress = handle.progress.lock().expect("progress lock");
    assert!(matches!(
        progress.as_slice(),
        [coco_types::WorkflowProgressEvent::WorkflowLog { message }]
            if message == "queued"
    ));
    drop(progress);
    let completed = handle.completed.lock().expect("completed lock");
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0].result.as_deref(), Some("{\n  \"ok\": true\n}"));
}

#[tokio::test]
async fn workflow_execute_surfaces_source_errors_as_real_errors() {
    let tool = WorkflowTool;
    let ctx = ToolUseContext::test_default();
    let input = WorkflowInput {
        script: Some("const x = 1; export const meta = { name: 'x', description: 'y' };".into()),
        ..WorkflowInput::default()
    };

    // `meta` is not the first statement → parse failure surfaces as an error,
    // not a fake launch.
    let err = tool.execute(input, &ctx).await.unwrap_err();
    assert!(matches!(err, ToolError::ExecutionFailed { .. }));
    assert!(err.to_string().contains("not launched"));
}

#[test]
fn workflow_validate_input_accepts_ts_fields_and_rejects_bad_resume() {
    let tool = WorkflowTool;
    let ctx = ToolUseContext::test_default();
    let valid = WorkflowInput {
        script: Some("export const meta = { name: 'x', description: 'y' };".into()),
        description: Some("ignored".into()),
        title: Some("ignored".into()),
        args: serde_json::json!({"x": 1}),
        ..WorkflowInput::default()
    };

    assert!(tool.validate_input(&valid, &ctx).is_valid());

    let invalid = WorkflowInput {
        name: Some("x".into()),
        resume_from_run_id: Some("bad".into()),
        ..WorkflowInput::default()
    };
    assert!(!tool.validate_input(&invalid, &ctx).is_valid());
}

#[test]
fn workflow_validation_code_round_trips_wire_strings() {
    use super::WorkflowValidationCode;
    assert_eq!(WorkflowValidationCode::SourceError.as_str(), "source_error");
    assert_eq!(WorkflowValidationCode::MetaParse.as_str(), "meta_parse");
    assert_eq!(WorkflowValidationCode::Determinism.as_str(), "determinism");
    assert_eq!(
        WorkflowValidationCode::ResumeRunning.as_str(),
        "resume_running"
    );
}

#[test]
fn workflow_missing_source_carries_typed_source_error_code() {
    use super::WorkflowValidationCode;
    use coco_tool_runtime::ValidationResult;

    let tool = WorkflowTool;
    let ctx = ToolUseContext::test_default();
    // No script / name / scriptPath → SourceError.
    let result = tool.validate_input(&WorkflowInput::default(), &ctx);
    match result {
        ValidationResult::Invalid { error_code, .. } => {
            assert_eq!(
                error_code.as_deref(),
                Some(WorkflowValidationCode::SourceError.as_str())
            );
        }
        ValidationResult::Valid => panic!("expected an invalid result with a typed code"),
    }
}

fn allow_rule(tool_pattern: &str, rule_content: Option<&str>) -> PermissionRule {
    PermissionRule {
        source: PermissionRuleSource::Session,
        behavior: coco_types::PermissionBehavior::Allow,
        value: PermissionRuleValue {
            tool_pattern: tool_pattern.to_string(),
            rule_content: rule_content.map(str::to_string),
        },
    }
}

#[tokio::test]
async fn workflow_named_allow_rule_allows_matching_name() {
    let tool = WorkflowTool;
    let mut ctx = ToolUseContext::test_default();
    ctx.permission_context.allow_rules.insert(
        PermissionRuleSource::Session,
        vec![allow_rule("Workflow", Some("release"))],
    );

    let input = WorkflowInput {
        name: Some("release".into()),
        ..WorkflowInput::default()
    };
    assert!(matches!(
        tool.check_permissions(&input, &ctx).await,
        ToolCheckResult::Allow { .. }
    ));
}

#[tokio::test]
async fn workflow_script_path_with_name_rule_still_asks() {
    // A {scriptPath, name} call must never be auto-allowed by a Workflow(name)
    // rule — scriptPath has no stable identity, so it always asks.
    let tool = WorkflowTool;
    let mut ctx = ToolUseContext::test_default();
    ctx.permission_context.allow_rules.insert(
        PermissionRuleSource::Session,
        vec![allow_rule("Workflow", Some("release"))],
    );

    let input = WorkflowInput {
        name: Some("release".into()),
        script_path: Some("/tmp/evil.js".into()),
        ..WorkflowInput::default()
    };
    assert!(matches!(
        tool.check_permissions(&input, &ctx).await,
        ToolCheckResult::Ask { .. }
    ));
}

#[tokio::test]
async fn workflow_inline_script_not_auto_allowed_by_contentless_rule() {
    // A content-less `Workflow` allow rule must not blanket-allow an inline
    // script (arbitrary code) — it falls through to Ask.
    let tool = WorkflowTool;
    let mut ctx = ToolUseContext::test_default();
    ctx.permission_context.allow_rules.insert(
        PermissionRuleSource::Session,
        vec![allow_rule("Workflow", None)],
    );

    let input = WorkflowInput {
        script: Some("export const meta = { name: 'x', description: 'y' };".into()),
        ..WorkflowInput::default()
    };
    assert!(matches!(
        tool.check_permissions(&input, &ctx).await,
        ToolCheckResult::Ask { .. }
    ));
}
