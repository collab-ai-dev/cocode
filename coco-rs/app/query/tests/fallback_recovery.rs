//! End-to-end integration tests for the multi-slot fallback +
//! half-open recovery wiring on `QueryEngine`.
//!
//! Unit coverage of the state machine lives in
//! `app/query/src/model_runtime.test.rs`; these tests exercise the
//! engine's integration points: constructing the runtime with a
//! chain, walking slots on capacity errors, transparently
//! recovering from probe failures, and surfacing the final provider
//! error when all configured cycles are spent.
//!
//! The mock model is deliberately minimal — it returns either a
//! capacity error or a text response based on a per-call counter.
//! We bypass the shared `mock_harness` because that harness
//! models only success paths.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::sync::Mutex;

use coco_inference::AISdkError;
use coco_inference::LanguageModel;
use coco_inference::LanguageModelCallOptions;
use coco_inference::LanguageModelGenerateResult;
use coco_inference::LanguageModelStreamResult;
use coco_inference::ModelRuntimeRegistry;
use coco_inference::PrebuiltLanguageModelSlot;
use coco_inference::RetryConfig;
use coco_llm_types::AssistantContentPart;
use coco_llm_types::FinishReason;
use coco_llm_types::StopReason;
use coco_llm_types::TextPart;
use coco_llm_types::Usage;
use coco_query::QueryEngine;
use coco_query::QueryEngineConfig;
use coco_tool_runtime::ToolRegistry;
use tokio_util::sync::CancellationToken;

/// Each scripted call returns either a capacity error or a
/// successful text reply. Tracks call count for assertions.
struct ScriptedCapacityMock {
    id: &'static str,
    calls: Arc<Mutex<Vec<CallOutcome>>>,
    next: std::sync::atomic::AtomicUsize,
}

#[derive(Clone, Debug)]
enum CallOutcome {
    Capacity,
    Text(&'static str),
}

impl ScriptedCapacityMock {
    fn new(id: &'static str, outcomes: Vec<CallOutcome>) -> Arc<Self> {
        Arc::new(Self {
            id,
            calls: Arc::new(Mutex::new(outcomes)),
            next: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    fn call_count(&self) -> usize {
        self.next.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl LanguageModel for ScriptedCapacityMock {
    fn provider(&self) -> &str {
        "mock"
    }
    fn model_id(&self) -> &str {
        self.id
    }
    async fn do_generate(
        &self,
        _options: &LanguageModelCallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelGenerateResult, AISdkError> {
        let idx = self.next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let outcome = self
            .calls
            .lock()
            .unwrap()
            .get(idx)
            .cloned()
            .unwrap_or(CallOutcome::Text("(exhausted)"));
        match outcome {
            CallOutcome::Capacity => {
                // "status: 529" matches the capacity classifier in engine.rs.
                Err(AISdkError::new("provider overloaded (status: 529)"))
            }
            CallOutcome::Text(s) => Ok(LanguageModelGenerateResult {
                content: vec![AssistantContentPart::Text(TextPart {
                    text: s.to_string(),
                    provider_metadata: None,
                })],
                usage: Usage::new(0, 0),
                finish_reason: FinishReason::new(StopReason::EndTurn),
                warnings: vec![],
                provider_metadata: None,
                request: None,
                response: None,
            }),
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

/// Build a prebuilt runtime slot with NO internal retries — each engine-level
/// `query_stream` corresponds to exactly one mock `do_generate`
/// call, so runtime fallback decisions map 1-to-1 to scripted
/// capacity outcomes. The default `max_retries=3` would
/// swallow 3 capacity outcomes per engine call and make the test
/// non-deterministic without more outcomes to burn.
fn runtime_slot(model: Arc<dyn LanguageModel>) -> PrebuiltLanguageModelSlot {
    let retry = RetryConfig {
        max_retries: 0,
        ..RetryConfig::default()
    };
    PrebuiltLanguageModelSlot::new(model, retry)
}

fn registry_with_main_slots(
    primary: PrebuiltLanguageModelSlot,
    fallback: PrebuiltLanguageModelSlot,
) -> Arc<ModelRuntimeRegistry> {
    Arc::new(ModelRuntimeRegistry::from_prebuilt_language_models(
        coco_types::ModelRole::Main,
        primary,
        vec![fallback],
    ))
}

fn minimal_config(model_id: &str) -> QueryEngineConfig {
    // `max_turns` bounds retry iterations. Keep it high enough for
    // two full fallback-chain cycles plus a successful call.
    QueryEngineConfig {
        model_id: model_id.to_string(),
        max_turns: Some(20),
        total_token_budget: Some(16_384),
        context_window: 200_000,
        max_output_tokens: 4_096,
        streaming_tool_execution: false,
        system_prompt: Some("you are a test assistant".into()),
        session_id: "s-fallback-test".into(),
        ..Default::default()
    }
}

#[tokio::test]
async fn test_engine_advances_to_fallback_after_capacity_failure() {
    // ApiClient owns same-slot retry. Once a capacity error reaches
    // the runtime, the runtime immediately advances to fallback.
    let primary = ScriptedCapacityMock::new("primary-model", vec![CallOutcome::Capacity]);
    let fallback = ScriptedCapacityMock::new("fallback-model", vec![CallOutcome::Text("done")]);

    let primary_slot = runtime_slot(primary.clone());
    let fallback_slot = runtime_slot(fallback.clone());

    let model_runtimes = registry_with_main_slots(primary_slot, fallback_slot);
    let engine = QueryEngine::new(
        minimal_config("primary-model"),
        model_runtimes,
        Arc::new(ToolRegistry::new()),
        CancellationToken::new(),
        None,
    );

    let result = engine.run("hello").await.expect("engine must recover");
    assert_eq!(
        primary.call_count(),
        1,
        "primary fails once before fallback"
    );
    assert_eq!(fallback.call_count(), 1, "fallback must serve recovery");
    assert_eq!(result.response_text, "done");
}

#[tokio::test]
async fn test_engine_surfaces_error_when_chain_exhausted() {
    // Primary + fallback both return capacity errors. The default
    // policy allows two full chain cycles; after that, the runtime
    // surfaces the last provider limit error with no caller-facing
    // fallback-exhausted event.
    let primary = ScriptedCapacityMock::new(
        "primary",
        vec![CallOutcome::Capacity, CallOutcome::Capacity],
    );
    let fallback = ScriptedCapacityMock::new(
        "fallback",
        vec![CallOutcome::Capacity, CallOutcome::Capacity],
    );
    let primary_slot = runtime_slot(primary.clone());
    let fallback_slot = runtime_slot(fallback.clone());

    let model_runtimes = registry_with_main_slots(primary_slot, fallback_slot);
    let engine = QueryEngine::new(
        minimal_config("primary"),
        model_runtimes,
        Arc::new(ToolRegistry::new()),
        CancellationToken::new(),
        None,
    );

    // Capture emitted events to verify chain exhaustion is not a
    // user-facing notice.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<coco_query::CoreEvent>(64);
    let events_handle = tokio::spawn(async move {
        let mut collected = Vec::new();
        while let Some(e) = rx.recv().await {
            collected.push(e);
        }
        collected
    });

    let result = engine
        .run_with_messages(
            vec![std::sync::Arc::new(coco_messages::create_user_message(
                "hello",
            ))],
            tx,
            coco_types::TurnId::generate(),
        )
        .await;
    assert!(result.is_err(), "exhausted chain must surface error");
    assert_eq!(primary.call_count(), 2);
    assert_eq!(fallback.call_count(), 2);

    let events = events_handle.await.unwrap();
    let saw_exhausted = events.iter().any(|e| {
        matches!(
            e,
            coco_query::CoreEvent::Stream(coco_query::AgentStreamEvent::TextDelta { delta, .. })
                if delta.contains("exhausted")
        )
    });
    assert!(
        !saw_exhausted,
        "chain exhaustion must not emit a user-facing exhausted notice; events: {events:?}"
    );
}
