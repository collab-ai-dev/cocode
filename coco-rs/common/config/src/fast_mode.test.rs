use super::*;

#[test]
fn test_fast_mode_supported_model_gate() {
    // config#247: capability-driven, not a model-id substring. Every builtin
    // Anthropic model declares Capability::FastMode.
    assert!(is_fast_mode_supported_by_model("claude-opus-4-7"));
    assert!(is_fast_mode_supported_by_model("claude-sonnet-4-6"));
    assert!(is_fast_mode_supported_by_model("claude-haiku-4-5"));
    // Unregistered / unknown model ids are not fast-capable. (The old substring
    // wrongly returned true for the nonexistent "opus-4-6".)
    assert!(!is_fast_mode_supported_by_model("claude-opus-4-6-20250514"));
    assert!(!is_fast_mode_supported_by_model("gpt-4o"));
}
