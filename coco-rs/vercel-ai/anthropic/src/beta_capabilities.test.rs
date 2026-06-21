use super::*;

#[test]
fn maps_each_capability_to_kebab_case_with_date_suffix() {
    assert_eq!(
        map_capability(AdapterBetaCapability::Context1m),
        Some("context-1m-2025-08-07")
    );
    assert_eq!(
        map_capability(AdapterBetaCapability::InterleavedThinking),
        Some("interleaved-thinking-2025-05-14")
    );
    assert_eq!(
        map_capability(AdapterBetaCapability::ContextManagement),
        Some("context-management-2025-06-27")
    );
    assert_eq!(
        map_capability(AdapterBetaCapability::StructuredOutputs),
        Some("structured-outputs-2025-11-13")
    );
    assert_eq!(
        map_capability(AdapterBetaCapability::TokenEfficientTools),
        Some("token-efficient-tools-2026-03-28")
    );
    assert_eq!(
        map_capability(AdapterBetaCapability::FastMode),
        Some("fast-mode-2026-02-01")
    );
    assert_eq!(
        map_capability(AdapterBetaCapability::PromptCachingScope),
        Some("prompt-caching-scope-2026-01-05")
    );
    assert_eq!(
        map_capability(AdapterBetaCapability::RedactThinking),
        Some("redact-thinking-2026-02-12")
    );
    assert_eq!(
        map_capability(AdapterBetaCapability::Advisor),
        Some("advisor-2025-12-04")
    );
}

#[test]
fn baseline_header_is_claude_code_20250219() {
    assert_eq!(CLAUDE_CODE_BASELINE, "claude-code-20250219");
}
