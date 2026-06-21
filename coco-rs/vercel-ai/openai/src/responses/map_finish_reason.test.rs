use super::*;

#[test]
fn none_no_fn_call_is_stop() {
    let r = map_openai_responses_finish_reason(None, false);
    assert_eq!(r.unified, UnifiedFinishReason::EndTurn);
}

#[test]
fn none_with_fn_call_is_tool_calls() {
    let r = map_openai_responses_finish_reason(None, true);
    assert_eq!(r.unified, UnifiedFinishReason::ToolUse);
}

#[test]
fn max_output_tokens_is_length() {
    let r = map_openai_responses_finish_reason(Some("max_output_tokens"), false);
    assert_eq!(r.unified, UnifiedFinishReason::MaxTokens);
}

#[test]
fn content_filter() {
    let r = map_openai_responses_finish_reason(Some("content_filter"), false);
    assert_eq!(r.unified, UnifiedFinishReason::ContentFilter);
}

#[test]
fn completed_no_fn_call_is_end_turn() {
    let r = map_openai_responses_finish_reason(Some("completed"), false);
    assert_eq!(r.unified, UnifiedFinishReason::EndTurn);
    assert_eq!(r.raw.as_deref(), Some("completed"));
}

#[test]
fn completed_with_fn_call_is_tool_use() {
    let r = map_openai_responses_finish_reason(Some("completed"), true);
    assert_eq!(r.unified, UnifiedFinishReason::ToolUse);
}

#[test]
fn context_length_exceeded_is_context_window() {
    // The `response.failed` arm sets this status so reactive compaction
    // fires rather than the turn collapsing to a generic finish.
    let r = map_openai_responses_finish_reason(Some("context_length_exceeded"), false);
    assert_eq!(r.unified, UnifiedFinishReason::ContextWindowExceeded);
    // Even with a pending function call the context-window signal wins.
    let r = map_openai_responses_finish_reason(Some("context_length_exceeded"), true);
    assert_eq!(r.unified, UnifiedFinishReason::ContextWindowExceeded);
}

#[test]
fn error_status_classifies_as_error_not_tool_use() {
    // The `response.failed` arm leaves an `"error"` sentinel after pushing a
    // `StreamPart::Error`. A pending function call must NOT cause it to fall
    // through to `ToolUse` and re-dispatch the call.
    let r = map_openai_responses_finish_reason(Some("error"), true);
    assert_eq!(r.unified, UnifiedFinishReason::Error);
    let r = map_openai_responses_finish_reason(Some("error"), false);
    assert_eq!(r.unified, UnifiedFinishReason::Error);
}
