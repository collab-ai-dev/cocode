use super::*;
use pretty_assertions::assert_eq;

#[test]
fn maps_stop_and_completed_to_end_turn() {
    assert_eq!(
        map_xai_responses_finish_reason(Some("stop")),
        UnifiedFinishReason::EndTurn
    );
    assert_eq!(
        map_xai_responses_finish_reason(Some("completed")),
        UnifiedFinishReason::EndTurn
    );
}

#[test]
fn maps_length_and_max_output_tokens_to_max_tokens() {
    assert_eq!(
        map_xai_responses_finish_reason(Some("length")),
        UnifiedFinishReason::MaxTokens
    );
    assert_eq!(
        map_xai_responses_finish_reason(Some("max_output_tokens")),
        UnifiedFinishReason::MaxTokens
    );
}

#[test]
fn maps_tool_calls_and_function_call_to_tool_use() {
    assert_eq!(
        map_xai_responses_finish_reason(Some("tool_calls")),
        UnifiedFinishReason::ToolUse
    );
    assert_eq!(
        map_xai_responses_finish_reason(Some("function_call")),
        UnifiedFinishReason::ToolUse
    );
}

#[test]
fn maps_content_filter() {
    assert_eq!(
        map_xai_responses_finish_reason(Some("content_filter")),
        UnifiedFinishReason::ContentFilter
    );
}

#[test]
fn maps_none_and_unknown_and_error_to_other() {
    assert_eq!(
        map_xai_responses_finish_reason(None),
        UnifiedFinishReason::Other
    );
    assert_eq!(
        map_xai_responses_finish_reason(Some("error")),
        UnifiedFinishReason::Other
    );
    assert_eq!(
        map_xai_responses_finish_reason(Some("whatever")),
        UnifiedFinishReason::Other
    );
}
