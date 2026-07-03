//! Regression: BOTH tool-execution paths (streaming — the default —
//! and batch) must emit `ServerNotification::TaskPanelChanged` when a
//! task tool patches the shared `ToolAppState`.
//!
//! The streaming executor was once constructed without the event sink
//! (`engine_turn_request`), so `apply_side_effects` applied the patch —
//! the model kept seeing tasks via `TaskList` / reminders — but the
//! snapshot never reached the TUI and the tasks/todo panel rendered
//! nothing for the whole session. The batch runner
//! (`tool_call_runner::create_executor`) was wired correctly, which is
//! why only the default path was broken. Both paths now share that one
//! construction seam; the parametrized probe pins both halves so
//! neither can drift again.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::sync::atomic::AtomicI32;
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
use coco_llm_types::ToolCallPart;
use coco_llm_types::Usage;
use coco_query::CoreEvent;
use coco_query::QueryEngine;
use coco_query::QueryEngineConfig;
use coco_query::ServerNotification;
use coco_tool_runtime::InMemoryTaskListHandle;
use coco_tool_runtime::ToolRegistry;
use coco_types::PermissionMode;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const TASK_SUBJECT: &str = "PANEL-MARKER-4471";

/// Turn 1 emits a `TaskCreate` tool call; turn 2 ends the conversation.
struct TaskCreateMock {
    call_count: AtomicI32,
}

#[async_trait::async_trait]
impl LanguageModel for TaskCreateMock {
    fn provider(&self) -> &str {
        "mock"
    }
    fn model_id(&self) -> &str {
        "task-panel-mock"
    }

    async fn do_generate(
        &self,
        _options: &LanguageModelCallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelGenerateResult, AISdkError> {
        let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
        if idx == 0 {
            Ok(LanguageModelGenerateResult {
                content: vec![AssistantContentPart::ToolCall(ToolCallPart {
                    tool_call_id: "task_create_0".into(),
                    tool_name: "TaskCreate".into(),
                    input: serde_json::json!({
                        "subject": TASK_SUBJECT,
                        "description": "streaming TaskPanelChanged regression probe",
                    }),
                    provider_executed: None,
                    provider_metadata: None,
                    invalid: false,
                    invalid_reason: None,
                })],
                usage: Usage::new(50, 20),
                finish_reason: FinishReason::new(StopReason::ToolUse),
                warnings: vec![],
                provider_metadata: None,
                request: None,
                response: None,
            })
        } else {
            Ok(LanguageModelGenerateResult {
                content: vec![AssistantContentPart::Text(TextPart {
                    text: "task created".into(),
                    provider_metadata: None,
                })],
                usage: Usage::new(40, 15),
                finish_reason: FinishReason::new(StopReason::EndTurn),
                warnings: vec![],
                provider_metadata: None,
                request: None,
                response: None,
            })
        }
    }

    async fn do_stream(
        &self,
        options: &LanguageModelCallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelStreamResult, AISdkError> {
        let result = self.do_generate(options, None).await?;
        Ok(coco_inference::synthetic_stream_from_content(
            result.content,
            result.usage,
            result.finish_reason,
        ))
    }
}

fn task_tools() -> Arc<ToolRegistry> {
    let registry = ToolRegistry::new();
    registry.register(Arc::new(coco_tools::TaskCreateTool));
    Arc::new(registry)
}

async fn probe_task_panel_events(streaming: bool) {
    let model = Arc::new(TaskCreateMock {
        call_count: AtomicI32::new(0),
    });
    let client = coco_query::test_support::model_runtime_registry(model);
    let cancel = CancellationToken::new();
    let config = QueryEngineConfig {
        model_id: "task-panel-mock".into(),
        permission_mode: PermissionMode::BypassPermissions,
        max_turns: Some(4),
        streaming_tool_execution: streaming,
        ..Default::default()
    };

    let app_state = Arc::new(tokio::sync::RwLock::new(coco_types::ToolAppState::default()));
    let engine = QueryEngine::new(config, client, task_tools(), cancel, None)
        .with_app_state(app_state.clone())
        .with_task_list(Arc::new(InMemoryTaskListHandle::new()));

    let (event_tx, mut event_rx) = mpsc::channel::<CoreEvent>(64);
    let event_collector = tokio::spawn(async move {
        let mut events = Vec::new();
        while let Some(e) = event_rx.recv().await {
            events.push(e);
        }
        events
    });

    let initial_messages = vec![Arc::new(coco_messages::create_user_message(
        "track this work as a task",
    ))];
    engine
        .run_with_messages(initial_messages, event_tx, coco_types::TurnId::generate())
        .await
        .expect("engine should complete");
    let events = event_collector.await.expect("event collector exited");

    // Sanity: the patch itself must have applied to the shared state —
    // this is what kept working even while the event was dropped, and it
    // distinguishes "emission broken" from "tool broken".
    {
        let state = app_state.read().await;
        assert!(
            state.plan_tasks.iter().any(|t| t.subject == TASK_SUBJECT),
            "TaskCreate must patch ToolAppState.plan_tasks; got {:?}",
            state.plan_tasks
        );
    }

    // The regression assertion: the snapshot must reach the event sink.
    let panels: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            CoreEvent::Protocol(ServerNotification::TaskPanelChanged(p)) => Some(p),
            _ => None,
        })
        .collect();
    assert!(
        !panels.is_empty(),
        "tool execution (streaming={streaming}) must emit TaskPanelChanged \
         (the executor needs its event sink wired — see \
         tool_call_runner::create_executor)"
    );
    let last = panels.last().unwrap();
    assert!(
        last.plan_tasks.iter().any(|t| t.subject == TASK_SUBJECT),
        "TaskPanelChanged snapshot must carry the created task; got {:?}",
        last.plan_tasks
    );
    assert!(
        matches!(last.expanded_view, coco_types::ExpandedView::Tasks),
        "TaskCreate auto-expands the panel (build_task_list_patch sets \
         expanded_view = Tasks); got {:?}",
        last.expanded_view
    );
    // Generations must be positive and strictly increasing — consumers
    // rely on them to drop out-of-order deliveries across producer
    // channels (leader executor vs subagent/teammate bridges).
    let generations: Vec<i64> = panels.iter().map(|p| p.generation).collect();
    assert!(
        generations.windows(2).all(|w| 0 < w[0] && w[0] < w[1]) && generations[0] > 0,
        "snapshot generations must be positive and strictly increasing; \
         got {generations:?}"
    );
}

/// The DEFAULT path — this is the half that shipped broken (executor
/// built without the event sink).
#[tokio::test]
async fn e2e_task_create_emits_task_panel_changed_streaming() {
    probe_task_panel_events(/*streaming*/ true).await;
}

#[tokio::test]
async fn e2e_task_create_emits_task_panel_changed_batch() {
    probe_task_panel_events(/*streaming*/ false).await;
}
