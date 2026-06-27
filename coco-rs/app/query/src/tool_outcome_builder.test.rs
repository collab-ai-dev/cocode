use super::*;

use std::sync::Arc;

use coco_hooks::orchestration::OrchestrationContext;
use coco_llm_types::LlmMessage;
use coco_llm_types::ToolContentPart;
use coco_llm_types::ToolResultContent;
use coco_messages::ToolResult as CocoToolResult;
use coco_messages::ToolResultContentPart;
use coco_tool_runtime::DescriptionOptions;
use coco_tool_runtime::Tool;
use coco_tool_runtime::ToolError;
use coco_tool_runtime::ToolUseContext;
use coco_types::ApplyPatchPreview;
use coco_types::ApplyPatchPreviewRow;
use coco_types::ToolDisplayData;
use coco_types::ToolId;
use pretty_assertions::assert_eq;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

struct RenderOnlyTool {
    parts: Vec<ToolResultContentPart>,
    is_mcp: bool,
    max_result_size_bound: coco_tool_runtime::ResultSizeBound,
}

#[async_trait::async_trait]
impl Tool for RenderOnlyTool {
    fn runtime_validation_schema(&self) -> &coco_tool_runtime::ToolInputSchema {
        static S: std::sync::OnceLock<coco_tool_runtime::ToolInputSchema> =
            std::sync::OnceLock::new();
        S.get_or_init(|| {
            coco_tool_runtime::ToolInputSchema::from_value(serde_json::json!({"type":"object"}))
                .expect("schema")
        })
    }
    // Migration scaffold: assoc types pinned to `Value`.
    type Input = serde_json::Value;
    type Output = serde_json::Value;

    fn id(&self) -> ToolId {
        ToolId::Custom("RenderOnly".into())
    }

    fn name(&self) -> &str {
        "RenderOnly"
    }

    fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "render only".into()
    }

    async fn prompt(&self, _options: &coco_tool_runtime::PromptOptions) -> String {
        "test tool".into()
    }

    async fn execute(
        &self,
        _: Value,
        _: &ToolUseContext,
    ) -> Result<CocoToolResult<Value>, ToolError> {
        unreachable!("tests inject execute_result directly")
    }

    fn render_for_model(&self, _: &Value) -> Vec<ToolResultContentPart> {
        self.parts.clone()
    }

    fn max_result_size_bound(&self) -> coco_tool_runtime::ResultSizeBound {
        self.max_result_size_bound
    }

    fn is_mcp(&self) -> bool {
        self.is_mcp
    }
}

fn text_part(text: impl Into<String>) -> ToolResultContentPart {
    ToolResultContentPart::Text {
        text: text.into(),
        provider_options: None,
    }
}

fn test_orchestration_ctx() -> OrchestrationContext {
    OrchestrationContext {
        session_id: "session-1".into(),
        cwd: std::path::PathBuf::from("/tmp"),
        project_dir: None,
        permission_mode: None,
        transcript_path: None,
        agent_id: None,
        agent_type: None,
        cancel: CancellationToken::new(),
        disable_all_hooks: false,
        allow_managed_hooks_only: false,
        workspace_trust_accepted: None,
        attachment_emitter: coco_messages::AttachmentEmitter::noop(),
        sync_event_sink: None,
        http_url_allowlist: None,
        http_env_var_policy: None,
        async_registry: None,
        async_rewake_sink: None,
        llm_handle: None,
    }
}

fn tool_result_text(message: &Message) -> (&str, bool) {
    let Message::ToolResult(tr) = message else {
        panic!("expected tool result");
    };
    let LlmMessage::Tool { content, .. } = &tr.message else {
        panic!("expected tool-role message");
    };
    let ToolContentPart::ToolResult(result) = &content[0] else {
        panic!("expected tool result content");
    };
    let text = match &result.output {
        ToolResultContent::Text { value, .. } | ToolResultContent::ErrorText { value, .. } => {
            value.as_str()
        }
        other => panic!("expected text output, got {other:?}"),
    };
    (text, result.is_error)
}

fn tool_result_display_data(message: &Message) -> Option<&ToolDisplayData> {
    let Message::ToolResult(tr) = message else {
        panic!("expected tool result");
    };
    tr.display_data.as_ref()
}

#[tokio::test]
async fn text_only_multipart_output_uses_level1_persistence() {
    let tmp = tempfile::TempDir::new().unwrap();
    let first = "a".repeat(30_000);
    let second = "b".repeat(30_000);
    let tool: Arc<dyn coco_tool_runtime::DynTool> = Arc::new(RenderOnlyTool {
        parts: vec![text_part(first.clone()), text_part(second.clone())],
        is_mcp: false,
        max_result_size_bound: coco_tool_runtime::ResultSizeBound::Chars(100_000),
    });

    let outcome = build_outcome_from_execution(RunOneTail {
        tool_use_id: "call-1".into(),
        tool_id: tool.id(),
        tool_name: tool.name().into(),
        model_index: 0,
        tool,
        effective_input: Value::Null,
        execute_result: Ok(CocoToolResult::data(Value::String("ignored".into()))),
        hooks: None,
        orchestration_ctx: test_orchestration_ctx(),
        hook_tx: None,
        tool_result_session_dir: Some(tmp.path().join("session-1")),
    })
    .await;

    let (text, is_error) = tool_result_text(&outcome.ordered_messages[0]);
    assert!(!is_error);
    assert!(text.starts_with("<persisted-output>"), "got: {text}");
    let persisted = tmp.path().join("session-1/tool-results/call-1.txt");
    assert_eq!(
        std::fs::read_to_string(persisted).unwrap(),
        format!("{first}\n\n{second}")
    );
}

#[tokio::test]
async fn mcp_error_envelope_creates_error_tool_result() {
    let tool: Arc<dyn coco_tool_runtime::DynTool> = Arc::new(RenderOnlyTool {
        parts: vec![text_part("server failed")],
        is_mcp: true,
        max_result_size_bound: coco_tool_runtime::ResultSizeBound::Chars(100_000),
    });

    let outcome = build_outcome_from_execution(RunOneTail {
        tool_use_id: "call-err".into(),
        tool_id: tool.id(),
        tool_name: tool.name().into(),
        model_index: 0,
        tool,
        effective_input: Value::Null,
        execute_result: Ok(CocoToolResult::data(serde_json::json!({
            "error": true,
            "content": [{"type": "text", "text": "server failed"}],
        }))),
        hooks: None,
        orchestration_ctx: test_orchestration_ctx(),
        hook_tx: None,
        tool_result_session_dir: None,
    })
    .await;

    let (text, is_error) = tool_result_text(&outcome.ordered_messages[0]);
    assert_eq!(text, "server failed");
    assert!(is_error);
}

#[tokio::test]
async fn structured_output_uses_tool_result_side_channel_only() {
    let tool: Arc<dyn coco_tool_runtime::DynTool> = Arc::new(RenderOnlyTool {
        parts: vec![text_part("visible result")],
        is_mcp: false,
        max_result_size_bound: coco_tool_runtime::ResultSizeBound::Chars(100_000),
    });
    let structured = serde_json::json!({"answer": 42});

    let outcome = build_outcome_from_execution(RunOneTail {
        tool_use_id: "call-structured".into(),
        tool_id: tool.id(),
        tool_name: tool.name().into(),
        model_index: 0,
        tool,
        effective_input: Value::Null,
        execute_result: Ok(CocoToolResult::data(serde_json::json!({
            "structuredOutput": {"answer": "model-visible-lookalike"}
        }))
        .with_structured_output(structured.clone())),
        hooks: None,
        orchestration_ctx: test_orchestration_ctx(),
        hook_tx: None,
        tool_result_session_dir: None,
    })
    .await;

    let Message::Attachment(att) = &outcome.ordered_messages[1] else {
        panic!("expected structured output attachment");
    };
    let coco_messages::AttachmentBody::Silent(coco_messages::SilentPayload::StructuredOutput(
        payload,
    )) = &att.body
    else {
        panic!("expected structured output payload");
    };
    assert_eq!(payload.data, structured);
}

#[tokio::test]
async fn structured_output_ignores_model_visible_data_lookalike() {
    let tool: Arc<dyn coco_tool_runtime::DynTool> = Arc::new(RenderOnlyTool {
        parts: vec![text_part("visible result")],
        is_mcp: false,
        max_result_size_bound: coco_tool_runtime::ResultSizeBound::Chars(100_000),
    });

    let outcome = build_outcome_from_execution(RunOneTail {
        tool_use_id: "call-lookalike".into(),
        tool_id: tool.id(),
        tool_name: tool.name().into(),
        model_index: 0,
        tool,
        effective_input: Value::Null,
        execute_result: Ok(CocoToolResult::data(serde_json::json!({
            "structuredOutput": {"answer": "not-side-channel"}
        }))),
        hooks: None,
        orchestration_ctx: test_orchestration_ctx(),
        hook_tx: None,
        tool_result_session_dir: None,
    })
    .await;

    assert_eq!(outcome.ordered_messages.len(), 1);
    let (text, is_error) = tool_result_text(&outcome.ordered_messages[0]);
    assert_eq!(text, "visible result");
    assert!(!is_error);
}

#[tokio::test]
async fn success_copies_tool_result_display_data() {
    let tool: Arc<dyn coco_tool_runtime::DynTool> = Arc::new(RenderOnlyTool {
        parts: vec![text_part("visible result")],
        is_mcp: false,
        max_result_size_bound: coco_tool_runtime::ResultSizeBound::Chars(100_000),
    });
    let display_data = ToolDisplayData::ApplyPatchPreview(ApplyPatchPreview {
        rows: vec![ApplyPatchPreviewRow::Omitted { rows: 5 }],
    });

    let outcome = build_outcome_from_execution(RunOneTail {
        tool_use_id: "call-display".into(),
        tool_id: tool.id(),
        tool_name: tool.name().into(),
        model_index: 0,
        tool,
        effective_input: Value::Null,
        execute_result: Ok(
            CocoToolResult::data(Value::Null).with_display_data(display_data.clone())
        ),
        hooks: None,
        orchestration_ctx: test_orchestration_ctx(),
        hook_tx: None,
        tool_result_session_dir: None,
    })
    .await;

    assert_eq!(
        tool_result_display_data(&outcome.ordered_messages[0]),
        Some(&display_data)
    );
}

#[tokio::test]
async fn error_copies_execution_failed_display_data() {
    let tool: Arc<dyn coco_tool_runtime::DynTool> = Arc::new(RenderOnlyTool {
        parts: vec![text_part("unused")],
        is_mcp: false,
        max_result_size_bound: coco_tool_runtime::ResultSizeBound::Chars(100_000),
    });
    let display_data = ToolDisplayData::ApplyPatchPreview(ApplyPatchPreview {
        rows: vec![ApplyPatchPreviewRow::Omitted { rows: 9 }],
    });

    let outcome = build_outcome_from_execution(RunOneTail {
        tool_use_id: "call-display-error".into(),
        tool_id: tool.id(),
        tool_name: tool.name().into(),
        model_index: 0,
        tool,
        effective_input: Value::Null,
        execute_result: Err(ToolError::ExecutionFailed {
            message: "failed".into(),
            display_data: Some(display_data.clone()),
            source: None,
        }),
        hooks: None,
        orchestration_ctx: test_orchestration_ctx(),
        hook_tx: None,
        tool_result_session_dir: None,
    })
    .await;

    assert_eq!(
        tool_result_display_data(&outcome.ordered_messages[0]),
        Some(&display_data)
    );
}

#[tokio::test]
async fn plain_tool_error_has_no_display_data() {
    let tool: Arc<dyn coco_tool_runtime::DynTool> = Arc::new(RenderOnlyTool {
        parts: vec![text_part("unused")],
        is_mcp: false,
        max_result_size_bound: coco_tool_runtime::ResultSizeBound::Chars(100_000),
    });

    let outcome = build_outcome_from_execution(RunOneTail {
        tool_use_id: "call-plain-error".into(),
        tool_id: tool.id(),
        tool_name: tool.name().into(),
        model_index: 0,
        tool,
        effective_input: Value::Null,
        execute_result: Err(ToolError::ExecutionFailed {
            message: "failed".into(),
            display_data: None,
            source: None,
        }),
        hooks: None,
        orchestration_ctx: test_orchestration_ctx(),
        hook_tx: None,
        tool_result_session_dir: None,
    })
    .await;

    assert!(tool_result_display_data(&outcome.ordered_messages[0]).is_none());
}

#[test]
fn build_early_outcome_pre_execution_cancelled_skips_failure_hooks() {
    // tool-runtime#15: a pre-execute turn abort yields PreExecutionCancelled
    // on the EarlyReturn path (no PostToolUseFailure) carrying CANCEL_MESSAGE
    // verbatim with is_error=true — distinct from a mid-execute cancel.
    let outcome = build_early_outcome(
        "tc-1".to_string(),
        coco_types::ToolId::Builtin(coco_types::ToolName::Bash),
        "Bash",
        0,
        coco_tool_runtime::ToolCallErrorKind::PreExecutionCancelled,
        coco_messages::CANCEL_MESSAGE,
        None,
    );
    assert_eq!(
        outcome.error_kind,
        Some(coco_tool_runtime::ToolCallErrorKind::PreExecutionCancelled)
    );
    assert!(
        !coco_tool_runtime::ToolCallErrorKind::PreExecutionCancelled.runs_post_tool_use_failure(),
        "pre-execution cancel must NOT fire PostToolUseFailure"
    );
    assert!(matches!(
        outcome.message_path,
        coco_tool_runtime::ToolMessagePath::EarlyReturn
    ));
    let (text, is_error) = tool_result_text(&outcome.ordered_messages[0]);
    assert_eq!(text, coco_messages::CANCEL_MESSAGE);
    assert!(is_error);
    // Mid-execute cancel keeps the failure-hook-firing classification.
    assert!(
        coco_tool_runtime::ToolCallErrorKind::ExecutionCancelled.runs_post_tool_use_failure(),
        "mid-execution cancel still fires PostToolUseFailure"
    );
}
