//! `coco-inference` model-runtime plumbing smoke test.
//!
//! Verifies fingerprint round-trip and usage accumulation. Builds its
//! own runtime client rather than reusing `LiveTarget.client` so the
//! snapshot-against-`(provider, model)` assertion stays a real check
//! rather than a tautology against the cached client.

use anyhow::Result;
use coco_inference::QueryParams;
use coco_llm_types::LlmMessage;

use crate::common::build_client;
use crate::common::query_client;
use crate::common::usage_report;

/// Builds a runtime client for `(provider, model)` and runs one
/// non-streaming query through it. Asserts: response text non-empty,
/// usage accumulator updated, snapshot matches the provider name.
pub async fn run(provider: &str, model_id: &str) -> Result<()> {
    let client = build_client(provider, model_id)?;
    let snapshot = client.snapshot()?;

    assert_eq!(
        snapshot.provider, provider,
        "runtime snapshot provider should match"
    );
    assert_eq!(
        snapshot.runtime_snapshot.api_model_name, model_id,
        "runtime snapshot api_model_name should match"
    );

    let prompt = vec![
        LlmMessage::system("You are a helpful assistant. Be concise."),
        LlmMessage::user_text("Reply with the single word: ok"),
    ];

    let params = QueryParams {
        prompt,
        // 1024 leaves headroom for reasoning models — see the same
        // rationale on `basic::params_for`.
        max_tokens: Some(1024),
        thinking_level: None,
        fast_mode: false,
        tools: None,
        tool_choice: None,
        context_management: None,
        query_source: Some("coco-tests-live::inference_smoke".into()),
        agent_id: None,
        time_since_last_assistant_ms: None,
        agentic: false,
        cache: None,
        stop_sequences: None,
        response_format: None,
        cancel: None,
        wire_tap: None,
    };

    let result = query_client(&client, params).await?;
    usage_report::record(provider, model_id, "inference_smoke.run", &result.usage);

    let usage = client.accumulated_usage().await?;
    assert!(
        usage.total.input_tokens.total > 0,
        "{provider}/{model_id}: usage accumulator did not record input tokens"
    );
    assert!(
        usage.total.output_tokens.total > 0,
        "{provider}/{model_id}: usage accumulator did not record output tokens"
    );
    assert!(
        result.usage.input_tokens.total > 0,
        "{provider}/{model_id}: per-call input tokens should be > 0"
    );
    assert!(
        !result.content.is_empty(),
        "{provider}/{model_id}: response content was empty"
    );
    Ok(())
}
