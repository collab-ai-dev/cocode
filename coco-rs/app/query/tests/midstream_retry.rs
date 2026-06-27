//! Regression tests for X.3: in-place retry of a mid-stream capacity
//! error on a SINGLE-model runtime (no fallback).
//!
//! Scenario: an OpenAI-compatible gateway returns HTTP 200, opens the SSE
//! stream, then delivers a 429 as an in-stream `error` frame
//! (`too_many_requests`). The stream OPENS fine, so the handshake retry in
//! `client.rs::query_stream_with_config` never engages; the error surfaces
//! during consumption. Because there is no fallback model,
//! `finish_call_transition` returns `Exhausted` — so before the fix the
//! turn bailed as a non-retryable `ProviderError`. The fix re-issues the
//! identical request in place when nothing was emitted to the user yet,
//! bounded by `MAX_MIDSTREAM_CAPACITY_RETRIES` and gated on `had_output`.
//!
//! These complement `fallback_recovery.rs`, which covers the stream-OPEN
//! capacity path and the multi-slot fallback chain.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

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

/// The verbatim in-stream 429 blob the aidp/Azure gateway emits — the
/// exact shape that regressed (HTTP 200 + SSE `error` frame, no HTTP 429).
const IN_STREAM_429: &str = r#"OpenAI responses error: {"type":"error","error":{"type":"too_many_requests","code":"too_many_requests","headers":{"x-ms-fe-error":"true"},"message":"Too Many Requests","param":null},"sequence_number":2}"#;

#[derive(Clone)]
enum CallOutcome {
    /// Stream opens, then errors mid-stream with a 429 and NO prior output.
    MidStream429,
    /// Stream opens, emits `text` to the user, THEN errors mid-stream.
    TextThen429(&'static str),
    /// Clean text reply.
    Text(&'static str),
}

/// Drives `do_stream` from a scripted list, counting calls for assertions.
struct MidStreamMock {
    id: &'static str,
    calls: Mutex<Vec<CallOutcome>>,
    next: AtomicUsize,
}

impl MidStreamMock {
    fn new(id: &'static str, outcomes: Vec<CallOutcome>) -> Arc<Self> {
        Arc::new(Self {
            id,
            calls: Mutex::new(outcomes),
            next: AtomicUsize::new(0),
        })
    }

    fn call_count(&self) -> usize {
        self.next.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl LanguageModel for MidStreamMock {
    fn provider(&self) -> &str {
        "mock"
    }
    fn model_id(&self) -> &str {
        self.id
    }

    async fn do_generate(
        &self,
        _options: &LanguageModelCallOptions,
        _abort_signal: Option<CancellationToken>,
    ) -> Result<LanguageModelGenerateResult, AISdkError> {
        // Streaming path only; a plain reply keeps the trait satisfiable.
        Ok(LanguageModelGenerateResult {
            content: vec![AssistantContentPart::Text(TextPart {
                text: "(non-stream)".to_string(),
                provider_metadata: None,
            })],
            usage: Usage::new(0, 0),
            finish_reason: FinishReason::new(StopReason::EndTurn),
            warnings: vec![],
            provider_metadata: None,
            request: None,
            response: None,
        })
    }

    async fn do_stream(
        &self,
        _options: &LanguageModelCallOptions,
        _abort_signal: Option<CancellationToken>,
    ) -> Result<LanguageModelStreamResult, AISdkError> {
        let idx = self.next.fetch_add(1, Ordering::SeqCst);
        let outcome = self
            .calls
            .lock()
            .unwrap()
            .get(idx)
            .cloned()
            // Past the script: never resolve, so an unexpected extra call
            // shows up as a wrong `call_count`, not a hang-masking success.
            .unwrap_or(CallOutcome::MidStream429);
        // `do_stream` returns Ok — the stream OPENS — for every outcome;
        // the 429 rides INSIDE the stream, never as a handshake error.
        let stream = match outcome {
            CallOutcome::MidStream429 => coco_inference::synthetic_error_stream(IN_STREAM_429),
            CallOutcome::TextThen429(text) => {
                coco_inference::synthetic_error_stream_after_text(text, IN_STREAM_429)
            }
            CallOutcome::Text(s) => coco_inference::synthetic_stream_from_content(
                vec![AssistantContentPart::Text(TextPart {
                    text: s.to_string(),
                    provider_metadata: None,
                })],
                Usage::new(0, 0),
                FinishReason::new(StopReason::EndTurn),
            ),
        };
        Ok(stream)
    }
}

/// Single-model runtime, NO fallback (`fallback_count == 0`) — the config
/// that previously bailed. `max_retries == 0` keeps the handshake retry
/// out of the way so the only retries observed are the mid-stream ones.
fn single_model_registry(model: Arc<dyn LanguageModel>) -> Arc<ModelRuntimeRegistry> {
    let slot = PrebuiltLanguageModelSlot::new(
        model,
        RetryConfig {
            max_retries: 0,
            ..RetryConfig::default()
        },
    );
    Arc::new(ModelRuntimeRegistry::from_prebuilt_language_models(
        coco_types::ModelRole::Main,
        slot,
        vec![],
    ))
}

fn minimal_config() -> QueryEngineConfig {
    QueryEngineConfig {
        model_id: "primary-model".to_string(),
        max_turns: Some(20),
        total_token_budget: Some(16_384),
        context_window: 200_000,
        max_output_tokens: 4_096,
        streaming_tool_execution: false,
        system_prompt: Some("you are a test assistant".into()),
        session_id: "s-midstream-retry-test".into(),
        ..Default::default()
    }
}

fn engine_for(model: Arc<MidStreamMock>) -> QueryEngine {
    QueryEngine::new(
        minimal_config(),
        single_model_registry(model),
        Arc::new(ToolRegistry::new()),
        CancellationToken::new(),
        None,
    )
}

#[tokio::test]
async fn test_midstream_429_retries_in_place_then_succeeds() {
    // 429 mid-stream with nothing emitted ⇒ re-issue the same request in
    // place; the second attempt returns text and the turn succeeds.
    let model = MidStreamMock::new(
        "primary-model",
        vec![CallOutcome::MidStream429, CallOutcome::Text("recovered")],
    );
    let engine = engine_for(model.clone());

    let result = engine
        .run("hello")
        .await
        .expect("a no-output mid-stream 429 must recover in place");

    assert_eq!(
        model.call_count(),
        2,
        "expected one 429 then one in-place retry that succeeds"
    );
    assert_eq!(result.response_text, "recovered");
}

#[tokio::test]
async fn test_midstream_429_bails_after_retry_cap() {
    // A persistently-throttled single model must surface the error after a
    // bounded number of in-place retries — not spin forever.
    // MAX_MIDSTREAM_CAPACITY_RETRIES (3) retries ⇒ 4 total attempts.
    let model = MidStreamMock::new(
        "primary-model",
        vec![
            CallOutcome::MidStream429,
            CallOutcome::MidStream429,
            CallOutcome::MidStream429,
            CallOutcome::MidStream429,
            CallOutcome::MidStream429,
        ],
    );
    let engine = engine_for(model.clone());

    let result = engine.run("hello").await;

    assert!(
        result.is_err(),
        "a never-clearing mid-stream 429 must eventually surface as an error"
    );
    assert_eq!(
        model.call_count(),
        4,
        "3 in-place retries (the cap) then bail = 4 attempts"
    );
}

#[tokio::test]
async fn test_midstream_429_after_output_does_not_retry() {
    // The no-duplicate-output guard: once a text delta has been streamed
    // to the user, a mid-stream 429 must NOT be re-issued in place (that
    // would duplicate the visible output). It bails on the first error.
    let model = MidStreamMock::new(
        "primary-model",
        vec![
            CallOutcome::TextThen429("partial answer"),
            CallOutcome::Text("must-not-reach"),
        ],
    );
    let engine = engine_for(model.clone());

    let result = engine.run("hello").await;

    assert!(
        result.is_err(),
        "a mid-stream error after emitted output must surface, not silently retry"
    );
    assert_eq!(
        model.call_count(),
        1,
        "output was already emitted ⇒ no in-place retry"
    );
}
