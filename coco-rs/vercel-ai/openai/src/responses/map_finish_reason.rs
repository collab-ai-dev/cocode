use vercel_ai_provider::FinishReason;
use vercel_ai_provider::UnifiedFinishReason;

/// Map an OpenAI Responses API finish reason to an SDK `FinishReason`.
///
/// The Responses API uses different reason strings than Chat:
/// - `"max_output_tokens"` instead of `"length"`
/// - `null` with function calls means `tool-calls`
pub fn map_openai_responses_finish_reason(
    finish_reason: Option<&str>,
    has_function_call: bool,
) -> FinishReason {
    let raw = finish_reason.map(String::from);
    // OpenAI Responses API terminal `status` values, cross-validated against
    // codex-rs/codex-api/src/sse/responses.rs: `completed` is the normal
    // end-of-turn marker, `incomplete` carries its real reason in
    // `incomplete_details.reason` (which is what gets passed in here), and
    // `failed` surfaces as an error before reaching this mapper.
    let unified = match finish_reason {
        None if has_function_call => UnifiedFinishReason::ToolUse,
        None => UnifiedFinishReason::EndTurn,
        Some("completed") if has_function_call => UnifiedFinishReason::ToolUse,
        Some("completed") => UnifiedFinishReason::EndTurn,
        Some("max_output_tokens") => UnifiedFinishReason::MaxTokens,
        Some("content_filter") => UnifiedFinishReason::ContentFilter,
        // A mid-stream `response.failed` with `error.code ==
        // "context_length_exceeded"` is surfaced through this status so the
        // synthesized Finish carries `ContextWindowExceeded`, which routes
        // `app/query` to reactive compaction (matching codex's
        // `ApiError::ContextWindowExceeded`). Without it the overflow
        // collapsed to a generic finish and recovery never fired.
        Some("context_length_exceeded") => UnifiedFinishReason::ContextWindowExceeded,
        // A `response.failed` that already pushed a `StreamPart::Error`
        // (quota/policy/overload) leaves this sentinel `status`. The Error
        // part terminates the turn before the synthesized Finish is ever
        // consumed, so this arm is defensive: a failure must classify as
        // `Error`, never fall through to `ToolUse` and re-dispatch the call.
        Some("error") => UnifiedFinishReason::Error,
        _ if has_function_call => UnifiedFinishReason::ToolUse,
        _ => UnifiedFinishReason::Other,
    };
    FinishReason { unified, raw }
}

#[cfg(test)]
#[path = "map_finish_reason.test.rs"]
mod tests;
