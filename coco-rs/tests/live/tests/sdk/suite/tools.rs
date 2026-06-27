//! Tool-calling tests via `coco_inference::ModelRuntimeClient::query`.
//!
//! Validates the model emits a `get_weather` tool call. Full multi-step
//! tool-execution flow (call → execute → respond) is not covered here:
//! it requires an executable tool runtime, which lives in
//! `coco-tool-runtime` and is exercised end-to-end by the
//! `cli_deepseek` suite.

use anyhow::Result;
use coco_inference::QueryParams;
use coco_llm_types::LlmMessage;

use crate::common::LiveTarget;
use crate::common::has_tool_call_named;
use crate::common::query_client;
use crate::common::usage_report;
use crate::common::weather_tool_def;

/// Asserts the model emits a `get_weather` tool call when prompted.
pub async fn run(target: &LiveTarget) -> Result<()> {
    let params = QueryParams {
        // Same shape + rationale as `streaming::run_with_tools`:
        // 4096-token budget + imperative system prompt to keep
        // Gemini-3 reliably emitting the tool call.
        prompt: vec![
            LlmMessage::system(
                "You are a helpful assistant. For weather questions you MUST call \
                 the get_weather tool — do not answer with prose, do not refuse, \
                 do not return an empty message.",
            ),
            LlmMessage::user_text(
                "What's the weather in Tokyo? Call get_weather with city='Tokyo'.",
            ),
        ],
        // 16k removes max_tokens as a possible variable in the
        // assertion failure — see the assertion below: stop_reason
        // is surfaced so an intermittent gateway / model-side flake
        // (`Some(Error)` with 0 tokens = AIDP dropped the request;
        // `Some(Other)` with reasoning tokens but no text = Gemini
        // finished mid-thought without emitting content) is
        // distinguishable from a real wire-shape regression.
        max_tokens: Some(16_384),
        thinking_level: None,
        fast_mode: false,
        tools: Some(vec![weather_tool_def()]),
        tool_choice: None,
        context_management: None,
        query_source: Some("coco-tests-live::sdk::tools::run".into()),
        agent_id: None,
        time_since_last_assistant_ms: None,
        agentic: false,
        cache: None,
        stop_sequences: None,
        response_format: None,
        cancel: None,
        wire_tap: None,
    };
    let result = query_client(&target.client, params).await?;
    usage_report::record(target.provider, &target.model, "tools.run", &result.usage);

    // Surface the full `FinishReason` (typed `unified` + provider wire
    // `raw`, rendered together by its Debug) on failure so a flake is
    // interpretable:
    //   `Other` + raw="..."  → Gemini unmapped finish reason
    //   `Error` + 0/0 tokens → gateway dropped the request
    //   `MaxTokens`          → budget exhausted (real, bump cap)
    let content_summary = describe_content(&result.content);
    assert!(
        has_tool_call_named(&result, "get_weather"),
        "{}/{}: no get_weather tool call in response \
         (content count={}, summary={content_summary}, \
         stop_reason={:?}, \
         tokens_in={:?}, tokens_out={:?})",
        target.provider,
        target.model,
        result.content.len(),
        result.stop_reason,
        result.usage.input_tokens,
        result.usage.output_tokens,
    );
    Ok(())
}

/// Render `content` as a short per-part diagnostic string. Surfaced
/// in tool-call assertion failures so a no-tool-call result is
/// interpretable: an empty list (`[]`) means the model emitted nothing
/// before finish; `[text:..]` means the model answered in prose;
/// `[reasoning:..]` means the model only reasoned.
fn describe_content(parts: &[coco_llm_types::AssistantContentPart]) -> String {
    use coco_llm_types::AssistantContentPart;
    let mut out = String::from("[");
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        match p {
            AssistantContentPart::Text(t) => {
                let preview: String = t.text.chars().take(60).collect();
                out.push_str(&format!("text:{preview:?}"));
            }
            AssistantContentPart::Reasoning(r) => {
                let preview: String = r.text.chars().take(60).collect();
                out.push_str(&format!("reasoning:{preview:?}"));
            }
            AssistantContentPart::ToolCall(tc) => {
                out.push_str(&format!("tool_call:{}", tc.tool_name));
            }
            other => {
                out.push_str(&format!("{:?}", std::mem::discriminant(other)));
            }
        }
    }
    out.push(']');
    out
}
