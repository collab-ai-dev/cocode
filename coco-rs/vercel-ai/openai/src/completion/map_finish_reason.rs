use vercel_ai_provider::FinishReason;
use vercel_ai_provider::UnifiedFinishReason;

/// Map a Completions API finish reason.
pub fn map_openai_completion_finish_reason(finish_reason: Option<&str>) -> FinishReason {
    let raw = finish_reason.map(String::from);
    let unified = match finish_reason {
        Some("stop") => UnifiedFinishReason::EndTurn,
        Some("length") => UnifiedFinishReason::MaxTokens,
        Some("content_filter") => UnifiedFinishReason::ContentFilter,
        Some("function_call") | Some("tool_calls") => UnifiedFinishReason::ToolUse,
        _ => UnifiedFinishReason::Other,
    };
    FinishReason { unified, raw }
}
