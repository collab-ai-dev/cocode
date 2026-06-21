use vercel_ai_provider::FinishReason;
use vercel_ai_provider::UnifiedFinishReason;

/// Map an OpenAI-compatible Chat Completions finish_reason to an SDK `FinishReason`.
pub fn map_openai_compatible_chat_finish_reason(finish_reason: Option<&str>) -> FinishReason {
    let raw = finish_reason.map(String::from);
    let unified = match finish_reason {
        Some("stop") => UnifiedFinishReason::EndTurn,
        Some("length") => UnifiedFinishReason::MaxTokens,
        Some("content_filter") => UnifiedFinishReason::ContentFilter,
        Some("function_call" | "tool_calls") => UnifiedFinishReason::ToolUse,
        _ => UnifiedFinishReason::Other,
    };
    FinishReason { unified, raw }
}

#[cfg(test)]
#[path = "map_finish_reason.test.rs"]
mod tests;
