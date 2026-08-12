use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use coco_inference::AISdkError;
use coco_inference::LanguageModel;
use coco_inference::LanguageModelCallOptions;
use coco_inference::LanguageModelGenerateResult;
use coco_inference::LanguageModelStreamResult;
use coco_llm_types::AssistantContentPart;
use coco_llm_types::FinishReason;
use coco_llm_types::StopReason;
use coco_llm_types::TextPart;
use coco_llm_types::Usage;
use coco_messages::ToolResult;
use coco_tool_runtime::CanUseToolCallContext;
use coco_tool_runtime::CanUseToolDecision;
use coco_tool_runtime::CanUseToolHandle;
use coco_tool_runtime::DecisionReason;
use coco_tool_runtime::DescriptionOptions;
use coco_tool_runtime::ProgrammaticToolCallError;
use coco_tool_runtime::PromptOptions;
use coco_tool_runtime::Tool;
use coco_tool_runtime::ToolError;
use coco_tool_runtime::ToolInputSchema;
use coco_tool_runtime::ToolRegistry;
use coco_tool_runtime::ToolUseContext;
use coco_types::PermissionMode;
use coco_types::ToolId;
use serde_json::Value;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::*;

struct StubModel;

#[async_trait::async_trait]
impl LanguageModel for StubModel {
    fn provider(&self) -> &str {
        "stub"
    }

    fn model_id(&self) -> &str {
        "stub"
    }

    async fn do_generate(
        &self,
        _options: &LanguageModelCallOptions,
        _abort_signal: Option<CancellationToken>,
    ) -> Result<LanguageModelGenerateResult, AISdkError> {
        Ok(LanguageModelGenerateResult {
            content: vec![AssistantContentPart::Text(TextPart {
                text: String::new(),
                provider_metadata: None,
            })],
            usage: Usage::new(0, 0),
            finish_reason: FinishReason::new(StopReason::EndTurn),
            warnings: Vec::new(),
            provider_metadata: None,
            request: None,
            response: None,
        })
    }

    async fn do_stream(
        &self,
        options: &LanguageModelCallOptions,
        _abort_signal: Option<CancellationToken>,
    ) -> Result<LanguageModelStreamResult, AISdkError> {
        let result = self.do_generate(options, None).await?;
        Ok(coco_inference::synthetic_stream_from_content(
            result.content,
            result.usage,
            result.finish_reason,
        ))
    }
}

struct DynamicReadTool {
    executions: Arc<AtomicUsize>,
}

fn schema() -> &'static ToolInputSchema {
    static SCHEMA: std::sync::OnceLock<ToolInputSchema> = std::sync::OnceLock::new();
    SCHEMA.get_or_init(|| {
        ToolInputSchema::from_value(json!({
            "type": "object",
            "properties": { "mutate": { "type": "boolean" } },
            "required": ["mutate"],
            "additionalProperties": false
        }))
        .expect("schema")
    })
}

#[async_trait::async_trait]
impl Tool for DynamicReadTool {
    type Input = Value;
    type Output = Value;

    fn id(&self) -> ToolId {
        ToolId::Custom("DynamicRead".to_string())
    }

    fn name(&self) -> &str {
        "DynamicRead"
    }

    fn runtime_validation_schema(&self) -> &ToolInputSchema {
        schema()
    }

    fn description(&self, _input: &Value, _options: &DescriptionOptions) -> String {
        "dynamic read test".to_string()
    }

    async fn prompt(&self, _options: &PromptOptions) -> String {
        "dynamic read test".to_string()
    }

    fn is_read_only(&self, input: &Value) -> bool {
        input.get("mutate").and_then(Value::as_bool) == Some(false)
    }

    async fn execute(
        &self,
        input: Value,
        _ctx: &ToolUseContext,
    ) -> Result<ToolResult<Value>, ToolError> {
        self.executions.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult {
            data: input,
            new_messages: Vec::new(),
            app_state_patch: None,
            permission_updates: Vec::new(),
            display_data: None,
        })
    }
}

#[derive(Debug)]
struct RewriteToMutation;

#[async_trait::async_trait]
impl CanUseToolHandle for RewriteToMutation {
    async fn check(
        &self,
        _tool_id: &ToolId,
        _tool_name: &str,
        _input: &Value,
        _ctx: &CanUseToolCallContext,
    ) -> CanUseToolDecision {
        CanUseToolDecision::Allow {
            updated_input: Some(json!({ "mutate": true })),
            decision_reason: DecisionReason::Other {
                reason: "test rewrite".to_string(),
            },
        }
    }
}

fn harness(
    can_use_tool: Option<Arc<dyn CanUseToolHandle>>,
) -> (
    coco_tool_runtime::ProgrammaticToolCallHandleRef,
    Arc<AtomicUsize>,
) {
    let executions = Arc::new(AtomicUsize::new(0));
    let registry = Arc::new(ToolRegistry::new());
    registry.register(Arc::new(DynamicReadTool {
        executions: executions.clone(),
    }));
    let mut ctx = ToolUseContext::test_default();
    ctx.tools = registry.clone();
    ctx.permission_context.mode = PermissionMode::BypassPermissions;
    ctx.can_use_tool = can_use_tool;
    let materialization = Arc::new(registry.materialize(&ctx));
    let engine = QueryEngine::new(
        crate::QueryEngineConfig::default(),
        coco_types::SessionId::try_new("programmatic-test").unwrap(),
        crate::test_support::model_runtime_registry(Arc::new(StubModel)),
        registry,
        CancellationToken::new(),
        None,
    );
    (
        engine.programmatic_tool_handle(ctx, materialization),
        executions,
    )
}

#[tokio::test]
async fn read_only_programmatic_call_runs_through_canonical_runner() {
    let (handle, executions) = harness(None);

    let result = handle
        .call_read_only("DynamicRead".to_string(), json!({ "mutate": false }))
        .await
        .expect("read-only call");

    assert_eq!(result, json!({ "mutate": false }));
    assert_eq!(executions.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn permission_rewrite_cannot_widen_programmatic_call_to_mutation() {
    let (handle, executions) = harness(Some(Arc::new(RewriteToMutation)));

    let error = handle
        .call_read_only("DynamicRead".to_string(), json!({ "mutate": false }))
        .await
        .expect_err("mutating rewrite must fail closed");

    assert!(matches!(error, ProgrammaticToolCallError::Failed { .. }));
    assert!(error.to_string().contains("read-only"));
    assert_eq!(executions.load(Ordering::SeqCst), 0);
}
