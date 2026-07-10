use super::*;
use pretty_assertions::assert_eq;

#[test]
fn maps_stop() {
    let r = map_groq_finish_reason(Some("stop"));
    assert_eq!(r.unified, UnifiedFinishReason::EndTurn);
    assert_eq!(r.raw.as_deref(), Some("stop"));
}

#[test]
fn maps_length() {
    let r = map_groq_finish_reason(Some("length"));
    assert_eq!(r.unified, UnifiedFinishReason::MaxTokens);
}

#[test]
fn maps_tool_calls_and_function_call() {
    assert_eq!(
        map_groq_finish_reason(Some("tool_calls")).unified,
        UnifiedFinishReason::ToolUse
    );
    assert_eq!(
        map_groq_finish_reason(Some("function_call")).unified,
        UnifiedFinishReason::ToolUse
    );
}

#[test]
fn maps_content_filter() {
    assert_eq!(
        map_groq_finish_reason(Some("content_filter")).unified,
        UnifiedFinishReason::ContentFilter
    );
}

#[test]
fn maps_none_and_unknown_to_other() {
    let none = map_groq_finish_reason(None);
    assert_eq!(none.unified, UnifiedFinishReason::Other);
    assert!(none.raw.is_none());
    assert_eq!(
        map_groq_finish_reason(Some("whatever")).unified,
        UnifiedFinishReason::Other
    );
}
