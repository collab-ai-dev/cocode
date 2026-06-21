use super::*;
use vercel_ai_provider::UnifiedFinishReason;

#[test]
fn maps_stop_without_tool_calls() {
    let reason = map_finish_reason(Some("STOP"), false);
    assert_eq!(reason.unified, UnifiedFinishReason::EndTurn);
    assert_eq!(reason.raw.as_deref(), Some("STOP"));
}

#[test]
fn maps_stop_with_tool_calls() {
    let reason = map_finish_reason(Some("STOP"), true);
    assert_eq!(reason.unified, UnifiedFinishReason::ToolUse);
}

#[test]
fn maps_max_tokens() {
    let reason = map_finish_reason(Some("MAX_TOKENS"), false);
    assert_eq!(reason.unified, UnifiedFinishReason::MaxTokens);
}

#[test]
fn maps_safety() {
    let reason = map_finish_reason(Some("SAFETY"), false);
    assert_eq!(reason.unified, UnifiedFinishReason::ContentFilter);
}

#[test]
fn maps_recitation() {
    let reason = map_finish_reason(Some("RECITATION"), false);
    assert_eq!(reason.unified, UnifiedFinishReason::ContentFilter);
}

#[test]
fn maps_malformed_function_call() {
    let reason = map_finish_reason(Some("MALFORMED_FUNCTION_CALL"), false);
    assert_eq!(reason.unified, UnifiedFinishReason::Error);
}

#[test]
fn maps_none() {
    let reason = map_finish_reason(None, false);
    assert_eq!(reason.unified, UnifiedFinishReason::Other);
}

#[test]
fn maps_unknown() {
    let reason = map_finish_reason(Some("UNKNOWN_REASON"), false);
    assert_eq!(reason.unified, UnifiedFinishReason::Other);
    assert_eq!(reason.raw.as_deref(), Some("UNKNOWN_REASON"));
}
