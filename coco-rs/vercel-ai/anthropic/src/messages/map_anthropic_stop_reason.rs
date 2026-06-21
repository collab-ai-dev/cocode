use vercel_ai_provider::FinishReason;

/// Map an Anthropic stop reason to a unified `FinishReason`.
///
/// When `is_json_response_from_tool` is true, a `tool_use` stop reason
/// is mapped to `stop` instead of `tool_calls` (because the tool call
/// was used to produce a JSON structured output, not an actual tool invocation).
pub fn map_anthropic_stop_reason(
    reason: Option<&str>,
    is_json_response_from_tool: bool,
) -> FinishReason {
    let raw = reason.map(String::from);
    let unified = match reason {
        Some("end_turn" | "pause_turn") => vercel_ai_provider::UnifiedFinishReason::EndTurn,
        // Refinement: stop_sequence is its own variant (post-extension).
        Some("stop_sequence") => vercel_ai_provider::UnifiedFinishReason::StopSequence,
        Some("refusal") => vercel_ai_provider::UnifiedFinishReason::ContentFilter,
        Some("tool_use") => {
            if is_json_response_from_tool {
                // Structured-output-via-tool: the model used a tool to
                // emit JSON, not to invoke a real tool, so the turn ended
                // normally. `raw` stays `"tool_use"`, so `FinishReason`'s
                // Display renders `end_turn(tool_use)` in logs — this is
                // an INTENTIONAL remap, not a mis-mapping; don't "fix" it.
                vercel_ai_provider::UnifiedFinishReason::EndTurn
            } else {
                vercel_ai_provider::UnifiedFinishReason::ToolUse
            }
        }
        Some("max_tokens") => vercel_ai_provider::UnifiedFinishReason::MaxTokens,
        // Refinement: context_window_exceeded is its own variant (post-extension).
        Some("model_context_window_exceeded") => {
            vercel_ai_provider::UnifiedFinishReason::ContextWindowExceeded
        }
        Some("compaction") => vercel_ai_provider::UnifiedFinishReason::Other,
        _ => vercel_ai_provider::UnifiedFinishReason::Other,
    };
    FinishReason { unified, raw }
}

#[cfg(test)]
#[path = "map_anthropic_stop_reason.test.rs"]
mod tests;
