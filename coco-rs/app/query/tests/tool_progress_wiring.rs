//! Regression guard for the per-call `progress_tx` wiring.
//!
//! Foreground Bash streams incremental output via `ctx.progress_tx`
//! (`bash.rs`), which the session fans out as `ToolProgress` to the TUI
//! and protocol. The base `ToolUseContext` is built with a live sender
//! and `clone_for_tool_call` preserves it — but the per-call `run_one`
//! closures used to overwrite `call_ctx.progress_tx` with
//! `runtime.progress_tx` (always `None`), nulling the sender right
//! before `tool.execute` and silently disabling real-time progress.
//!
//! This drives a probe tool through a real `QueryEngine` under BOTH the
//! streaming and batch execution paths and asserts the tool observed a
//! live `progress_tx`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use coco_messages::ToolResult;
use coco_query::QueryEngine;
use coco_query::QueryEngineConfig;
use coco_tool_runtime::DescriptionOptions;
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

mod mock_harness;

use mock_harness::MockModelBuilder;

/// Tool that records whether `ctx.progress_tx` was live at execute time.
struct ProgressProbeTool {
    saw_progress_tx: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl Tool for ProgressProbeTool {
    type Input = Value;
    type Output = Value;

    fn runtime_validation_schema(&self) -> &ToolInputSchema {
        static SCHEMA: OnceLock<ToolInputSchema> = OnceLock::new();
        SCHEMA.get_or_init(|| {
            ToolInputSchema::from_value(json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            }))
            .expect("probe schema is valid")
        })
    }

    fn id(&self) -> ToolId {
        ToolId::Custom("progress_probe".into())
    }

    fn name(&self) -> &str {
        "progress_probe"
    }

    fn description(&self, _input: &Value, _options: &DescriptionOptions) -> String {
        "records ctx.progress_tx presence".into()
    }

    async fn prompt(&self, _options: &PromptOptions) -> String {
        "progress probe".into()
    }

    async fn execute(
        &self,
        _input: Value,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<Value>, ToolError> {
        self.saw_progress_tx
            .store(ctx.progress_tx.is_some(), Ordering::SeqCst);
        Ok(ToolResult {
            data: json!({ "ok": true }),
            new_messages: vec![],
            app_state_patch: None,
            permission_updates: Vec::new(),
            display_data: None,
        })
    }
}

/// Run one turn that calls the probe tool, returning whether the tool
/// saw a live `progress_tx`. `streaming` selects the streaming
/// (`StreamingHandle`) vs batch (`execute_with`) `run_one` path.
async fn probe_progress_tx(streaming: bool) -> bool {
    let saw_progress_tx = Arc::new(AtomicBool::new(false));
    let registry = ToolRegistry::new();
    registry.register(Arc::new(ProgressProbeTool {
        saw_progress_tx: saw_progress_tx.clone(),
    }));
    let tools = Arc::new(registry);

    let model = MockModelBuilder::new()
        .then_tool_call("progress_probe", json!({}))
        .then_text("done")
        .build();

    let client = coco_query::test_support::model_runtime_registry(model);
    let config = QueryEngineConfig {
        model_id: "scripted-mock".into(),
        permission_mode: PermissionMode::BypassPermissions,
        max_turns: Some(10),
        streaming_tool_execution: streaming,
        ..Default::default()
    };
    let engine = QueryEngine::new(config, client, tools, CancellationToken::new(), None);
    engine
        .run("probe")
        .await
        .expect("mock engine should not fail");

    saw_progress_tx.load(Ordering::SeqCst)
}

#[tokio::test]
async fn streaming_path_preserves_progress_tx() {
    assert!(
        probe_progress_tx(/*streaming*/ true).await,
        "streaming run_one must not clobber ctx.progress_tx with runtime's None"
    );
}

#[tokio::test]
async fn batch_path_preserves_progress_tx() {
    assert!(
        probe_progress_tx(/*streaming*/ false).await,
        "batch run_one must not clobber ctx.progress_tx with runtime's None"
    );
}
