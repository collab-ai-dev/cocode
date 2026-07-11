use super::*;

#[test]
fn grok_420_reasoning_variants_reject_effort() {
    assert!(!supports_reasoning_effort("grok-4.20-reasoning"));
    assert!(!supports_reasoning_effort("grok-4.20-non-reasoning"));
    assert!(!supports_reasoning_effort("grok-4.20-0309-reasoning"));
    assert!(!supports_reasoning_effort("grok-4.20-1234-non-reasoning"));
}

#[test]
fn other_models_accept_effort() {
    assert!(supports_reasoning_effort("grok-4.3"));
    assert!(supports_reasoning_effort("grok-4.5"));
    assert!(supports_reasoning_effort("grok-latest"));
    assert!(supports_reasoning_effort("grok-4.20-multi-agent"));
    // A bare grok-4.20 without the reasoning suffix still accepts effort.
    assert!(supports_reasoning_effort("grok-4.20"));
}
