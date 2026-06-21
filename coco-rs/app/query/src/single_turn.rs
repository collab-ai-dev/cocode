//! Single-turn query execution.
//!
//! One-shot query without multi-turn loop. Used for compaction summaries,
//! memory extraction, etc.

use coco_inference::ModelRuntimeQueryOutcome;
use coco_inference::ModelRuntimeRegistry;
use coco_inference::ModelRuntimeSource;
use coco_inference::QueryParams;
use coco_llm_types::LlmPrompt;
use coco_types::TokenUsage;
use std::sync::Arc;

/// Result of a single-turn query.
#[derive(Debug, Clone)]
pub struct SingleTurnResult {
    pub text: String,
    pub usage: TokenUsage,
    pub model: String,
    pub duration_ms: i64,
}

/// Execute a single-turn query (no tool calls, no loop).
///
/// Used for side-queries like compaction summaries, memory extraction,
/// and classifier calls.
pub async fn single_turn_query(
    model_runtimes: &Arc<ModelRuntimeRegistry>,
    source: ModelRuntimeSource,
    system_prompt: &str,
    user_message: &str,
    max_tokens: Option<i64>,
) -> Result<SingleTurnResult, coco_error::BoxedError> {
    let start = std::time::Instant::now();

    let prompt: LlmPrompt = vec![
        coco_llm_types::LlmMessage::system(system_prompt),
        coco_llm_types::LlmMessage::user_text(user_message),
    ];

    let (result, snapshot) = loop {
        let params = QueryParams {
            prompt: prompt.clone(),
            max_tokens,
            thinking_level: None,
            fast_mode: false,
            tools: None,
            tool_choice: None,
            context_management: None,
            query_source: None,
            agent_id: None,
            time_since_last_assistant_ms: None,
            // One-shot helper call: not part of an agent loop, no cache
            // strategy plumbed at this layer.
            agentic: false,
            cache: None,
            stop_sequences: None,
            response_format: None,
            cancel: None,
            wire_tap: None,
        };
        match model_runtimes.query_once(source.clone(), &params).await {
            ModelRuntimeQueryOutcome::Success {
                result, snapshot, ..
            } => break (result, snapshot),
            ModelRuntimeQueryOutcome::Retry { .. } => continue,
            ModelRuntimeQueryOutcome::Failed { error, .. } => {
                return Err(Box::new(coco_error::PlainError::new(
                    format!("single-turn query failed: {error}"),
                    coco_error::StatusCode::ProviderError,
                )) as coco_error::BoxedError);
            }
        }
    };

    // Extract text from response
    let text = result
        .content
        .iter()
        .filter_map(|part| {
            if let coco_llm_types::AssistantContentPart::Text(t) = part {
                Some(t.text.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("");

    // Caller-side surfacing of unexpected outcomes — `SingleTurnResult`
    // doesn't carry stop_reason, so a length-truncated or
    // content-filtered response would otherwise reach the caller as
    // "just a short text" with no signal that the model didn't finish.
    let stop = result.stop_reason.as_ref();
    if text.is_empty() || stop.is_some_and(coco_messages::FinishReason::is_abnormal) {
        tracing::warn!(
            stop_reason = ?stop,
            tokens_out = result.usage.output_tokens.total,
            text_chars = text.len(),
            "single_turn unexpected outcome"
        );
    }

    let duration_ms = start.elapsed().as_millis() as i64;

    Ok(SingleTurnResult {
        text,
        usage: result.usage,
        model: snapshot.model_id,
        duration_ms,
    })
}

/// Execute a side query with a fast model (for classifiers, summaries).
///
/// Uses a smaller/faster model for non-critical queries.
pub async fn side_query(
    model_runtimes: &Arc<ModelRuntimeRegistry>,
    source: ModelRuntimeSource,
    system_prompt: &str,
    user_message: &str,
) -> Result<String, coco_error::BoxedError> {
    let result = single_turn_query(
        model_runtimes,
        source,
        system_prompt,
        user_message,
        Some(4096),
    )
    .await?;
    Ok(result.text)
}
