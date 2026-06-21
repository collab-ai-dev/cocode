use super::*;
use vercel_ai_provider::UnifiedFinishReason;

#[test]
fn maps_end_turn_to_stop() {
    let fr = map_anthropic_stop_reason(Some("end_turn"), false);
    assert_eq!(fr.unified, UnifiedFinishReason::EndTurn);
    assert_eq!(fr.raw.as_deref(), Some("end_turn"));
}

#[test]
fn maps_stop_sequence_to_typed_stop_sequence() {
    // Post-extension: `stop_sequence` is its own first-class
    // variant. Previously folded into `Stop`/`EndTurn`.
    let fr = map_anthropic_stop_reason(Some("stop_sequence"), false);
    assert_eq!(fr.unified, UnifiedFinishReason::StopSequence);
    assert_eq!(fr.raw.as_deref(), Some("stop_sequence"));
}

#[test]
fn maps_pause_turn_to_stop() {
    let fr = map_anthropic_stop_reason(Some("pause_turn"), false);
    assert_eq!(fr.unified, UnifiedFinishReason::EndTurn);
}

#[test]
fn maps_refusal_to_content_filter() {
    let fr = map_anthropic_stop_reason(Some("refusal"), false);
    assert_eq!(fr.unified, UnifiedFinishReason::ContentFilter);
}

#[test]
fn maps_tool_use_to_tool_calls() {
    let fr = map_anthropic_stop_reason(Some("tool_use"), false);
    assert_eq!(fr.unified, UnifiedFinishReason::ToolUse);
}

#[test]
fn maps_tool_use_to_stop_when_json_response() {
    let fr = map_anthropic_stop_reason(Some("tool_use"), true);
    assert_eq!(fr.unified, UnifiedFinishReason::EndTurn);
}

#[test]
fn maps_max_tokens_to_length() {
    let fr = map_anthropic_stop_reason(Some("max_tokens"), false);
    assert_eq!(fr.unified, UnifiedFinishReason::MaxTokens);
}

#[test]
fn maps_model_context_window_exceeded_to_typed_variant() {
    // Post-extension: `model_context_window_exceeded` is its own
    // first-class variant. The engine routes it to reactive
    // compaction (not the `MaxTokens` 64k-escalate path) — raising
    // the output budget can't help when the input already overflows.
    let fr = map_anthropic_stop_reason(Some("model_context_window_exceeded"), false);
    assert_eq!(fr.unified, UnifiedFinishReason::ContextWindowExceeded);
    assert!(fr.unified.is_abnormal());
    assert_eq!(fr.raw.as_deref(), Some("model_context_window_exceeded"));
}

#[test]
fn maps_unknown_to_other() {
    let fr = map_anthropic_stop_reason(Some("compaction"), false);
    assert_eq!(fr.unified, UnifiedFinishReason::Other);
}

#[test]
fn maps_none_to_other() {
    let fr = map_anthropic_stop_reason(None, false);
    assert_eq!(fr.unified, UnifiedFinishReason::Other);
    assert!(fr.raw.is_none());
}
