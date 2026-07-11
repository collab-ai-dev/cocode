use vercel_ai_provider::UnifiedFinishReason;

/// Map an xAI Responses API status / finish reason to the unified variant.
///
/// Mirrors `map-xai-responses-finish-reason.ts`. The Responses API uses
/// terminal `status` strings (`completed`, `max_output_tokens`) rather than the
/// Chat `finish_reason` strings, so the mapping differs from
/// [`crate::map_xai_finish_reason`]. The caller keeps the raw string separately
/// and applies the `has_function_call` → `ToolUse` override.
pub fn map_xai_responses_finish_reason(finish_reason: Option<&str>) -> UnifiedFinishReason {
    match finish_reason {
        Some("stop" | "completed") => UnifiedFinishReason::EndTurn,
        Some("length" | "max_output_tokens") => UnifiedFinishReason::MaxTokens,
        Some("tool_calls" | "function_call") => UnifiedFinishReason::ToolUse,
        Some("content_filter") => UnifiedFinishReason::ContentFilter,
        _ => UnifiedFinishReason::Other,
    }
}

#[cfg(test)]
#[path = "map_xai_responses_finish_reason.test.rs"]
mod tests;
