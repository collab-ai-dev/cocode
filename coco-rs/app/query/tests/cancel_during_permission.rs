//! Regression test for the cancel → tool_result-synthesis invariant.
//!
//! When a permission bridge hangs (TUI never resolves, SDK client
//! disconnects, …) and the engine's cancel token fires, the engine
//! must:
//!
//! 1. Drop out of `bridge.request_permission().await` via the
//!    `tokio::select!` cancel arm in
//!    `permission_controller.rs:197-203`.
//! 2. Synthesize an error tool_result via `complete_tool_call_with_error`
//!    so the message history stays validly tool_use ↔ tool_result
//!    paired (Anthropic / OpenAI providers reject otherwise).
//! 3. Surface the cancellation in `QueryResult.cancelled = true`.
//!
//! This pins the cancel → tool_result synthesis behavior so future
//! refactors don't silently break it.
//!
//! Driven through the public `QueryEngine` API (`run` →
//! `run_internal_with_messages`) — no reaching into
//! `permission_controller` internals.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod mock_harness;

use std::sync::Arc;
use std::time::Duration;

use coco_query::QueryEngine;
use coco_query::QueryEngineConfig;
use coco_tool_runtime::ToolPermissionBridge;
use coco_tool_runtime::ToolPermissionBridgeRef;
use coco_tool_runtime::ToolPermissionRequest;
use coco_tool_runtime::ToolPermissionResolution;
use coco_types::PermissionMode;
use serde_json::json;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use mock_harness::MockModelBuilder;
use mock_harness::MockResponse;
use mock_harness::core_tools;

/// Permission bridge whose `request_permission` future never resolves.
/// Awaits a never-completing inner future so the only way out is the
/// engine's own `cancel.cancelled()` arm in
/// `permission_controller.rs:197-203`.
struct HangingPermissionBridge {
    entered: Arc<Notify>,
}

#[async_trait::async_trait]
impl ToolPermissionBridge for HangingPermissionBridge {
    async fn request_permission(
        &self,
        _request: ToolPermissionRequest,
    ) -> Result<ToolPermissionResolution, String> {
        self.entered.notify_waiters();
        // Never resolves. The engine's `tokio::select!` race with its
        // cancel token is the only escape — exactly the path under test.
        std::future::pending::<()>().await;
        unreachable!("HangingPermissionBridge: pending future resolved");
    }
}

fn hanging_bridge() -> (ToolPermissionBridgeRef, Arc<Notify>) {
    let entered = Arc::new(Notify::new());
    (
        Arc::new(HangingPermissionBridge {
            entered: entered.clone(),
        }),
        entered,
    )
}

/// Mock model that issues exactly one Bash tool call. Bash is not in
/// the Default-mode static auto-allow list, and this command is not
/// dynamically read-only, so the engine routes the call through the
/// permission bridge.
fn mock_model_calling_bash() -> Arc<dyn coco_inference::LanguageModel> {
    MockModelBuilder::new()
        .on_call(0, |_| {
            MockResponse::tool_call("Bash", json!({"command": "mkdir cancel-test-dir"}))
        })
        .build()
}

#[tokio::test]
async fn cancelled_permission_synthesizes_error_tool_result() {
    let cancel = CancellationToken::new();
    let model = mock_model_calling_bash();
    let client = coco_query::test_support::model_runtime_registry(model);
    let config = QueryEngineConfig {
        model_id: "scripted-mock".into(),
        // Default mode forces Bash through Ask — exactly what triggers
        // the bridge wait we want to cancel.
        permission_mode: PermissionMode::Default,
        max_turns: Some(4),
        session_id: "cancel-during-permission".into(),
        ..Default::default()
    };
    let (bridge, bridge_entered) = hanging_bridge();
    let engine = QueryEngine::new(config, client, core_tools(), cancel.clone(), None)
        .with_permission_bridge(bridge);

    // Spawn the engine on a background task so we can fire cancel
    // mid-flight. The hanging bridge means the run never returns
    // without external cancellation.
    let run_handle = tokio::spawn(async move { engine.run("run echo for me").await });

    // Wait until the bridge is definitely pending before firing cancel.
    // This avoids a timing race where the test cancels too early or too
    // late relative to permission resolution.
    tokio::time::timeout(Duration::from_secs(5), bridge_entered.notified())
        .await
        .expect("permission bridge should be entered before cancellation");
    cancel.cancel();

    // Bound the wait so a regression that breaks the cancel path
    // surfaces as a test timeout rather than hanging the suite.
    let result = tokio::time::timeout(Duration::from_secs(5), run_handle)
        .await
        .expect("engine should return within 5s of cancel — regression in cancel→synthesis path")
        .expect("join handle panicked")
        .expect("engine.run returned an unexpected error");

    // 1) Cancellation must be recorded.
    assert!(
        result.cancelled,
        "expected QueryResult.cancelled=true after firing the cancel token"
    );

    // 2) The history must contain a synthesized tool_result for the
    //    Bash tool_use_id. Without this the next API call would be
    //    rejected by the provider (unmatched tool_use). The synthesizer
    //    is `complete_tool_call_with_error` invoked from
    //    `permission_controller.rs:237-258` (the `Err` arm hit when
    //    cancel races the bridge).
    let tool_result_count = result
        .final_messages
        .iter()
        .filter(|m| matches!(m.as_ref(), coco_messages::Message::ToolResult(_)))
        .count();
    assert!(
        tool_result_count >= 1,
        "expected at least one synthesized tool_result in final_messages; \
         got {tool_result_count} ToolResult messages. Final messages: {:#?}",
        result
            .final_messages
            .iter()
            .map(|m| message_kind(m.as_ref()))
            .collect::<Vec<_>>(),
    );
}

/// Compact label for assertion-failure printing.
fn message_kind(m: &coco_messages::Message) -> &'static str {
    match m {
        coco_messages::Message::User(_) => "User",
        coco_messages::Message::Assistant(_) => "Assistant",
        coco_messages::Message::ToolResult(_) => "ToolResult",
        coco_messages::Message::Attachment(_) => "Attachment",
        coco_messages::Message::System(_) => "System",
        coco_messages::Message::Progress(_) => "Progress",
        coco_messages::Message::Tombstone(_) => "Tombstone",
    }
}
