use super::*;

#[test]
fn maps_stop() {
    let r = map_openai_chat_finish_reason(Some("stop"));
    assert_eq!(r.unified, UnifiedFinishReason::EndTurn);
    assert_eq!(r.raw.as_deref(), Some("stop"));
}

#[test]
fn maps_length() {
    let r = map_openai_chat_finish_reason(Some("length"));
    assert_eq!(r.unified, UnifiedFinishReason::MaxTokens);
}

#[test]
fn maps_tool_calls() {
    let r = map_openai_chat_finish_reason(Some("tool_calls"));
    assert_eq!(r.unified, UnifiedFinishReason::ToolUse);
}

#[test]
fn maps_function_call() {
    let r = map_openai_chat_finish_reason(Some("function_call"));
    assert_eq!(r.unified, UnifiedFinishReason::ToolUse);
}

#[test]
fn maps_content_filter() {
    let r = map_openai_chat_finish_reason(Some("content_filter"));
    assert_eq!(r.unified, UnifiedFinishReason::ContentFilter);
}

#[test]
fn maps_none_to_other() {
    let r = map_openai_chat_finish_reason(None);
    assert_eq!(r.unified, UnifiedFinishReason::Other);
    assert!(r.raw.is_none());
}

#[test]
fn maps_unknown_to_other() {
    let r = map_openai_chat_finish_reason(Some("whatever"));
    assert_eq!(r.unified, UnifiedFinishReason::Other);
}
