use pretty_assertions::assert_eq;

use super::*;

#[test]
fn test_thinking_level_disable() {
    let level = ThinkingLevel::disable();
    assert_eq!(level.effort, ReasoningEffort::Off);
    assert!(!level.is_enabled());
}

#[test]
fn test_thinking_level_auto_is_default_and_enabled() {
    let level = ThinkingLevel::auto();
    assert_eq!(level.effort, ReasoningEffort::Auto);
    assert!(
        level.is_enabled(),
        "Auto is opt-in: provider may still resolve to off, \
         but the user has not explicitly disabled thinking"
    );
    assert_eq!(level, ThinkingLevel::default());
}

#[test]
fn test_thinking_level_high() {
    let level = ThinkingLevel::high();
    assert_eq!(level.effort, ReasoningEffort::High);
    assert!(level.is_enabled());
    assert!(level.budget_tokens.is_none());
}

#[test]
fn test_thinking_level_with_budget() {
    let level = ThinkingLevel::with_budget(ReasoningEffort::Medium, 32000);
    assert_eq!(level.effort, ReasoningEffort::Medium);
    assert_eq!(level.budget_tokens, Some(32000));
}

#[test]
fn test_reasoning_effort_ordering() {
    assert!(ReasoningEffort::Off < ReasoningEffort::Auto);
    assert!(ReasoningEffort::Auto < ReasoningEffort::Low);
    assert!(ReasoningEffort::Low < ReasoningEffort::Medium);
    assert!(ReasoningEffort::Medium < ReasoningEffort::High);
    assert!(ReasoningEffort::High < ReasoningEffort::XHigh);
}

#[test]
fn test_is_explicit_level_only_numeric_efforts() {
    assert!(!ReasoningEffort::Off.is_explicit_level());
    assert!(!ReasoningEffort::Auto.is_explicit_level());
    assert!(ReasoningEffort::Minimal.is_explicit_level());
    assert!(ReasoningEffort::Low.is_explicit_level());
    assert!(ReasoningEffort::Medium.is_explicit_level());
    assert!(ReasoningEffort::High.is_explicit_level());
    assert!(ReasoningEffort::XHigh.is_explicit_level());
}

#[test]
fn test_thinking_level_from_str() {
    let level: ThinkingLevel = "high".parse().unwrap();
    assert_eq!(level.effort, ReasoningEffort::High);

    let level: ThinkingLevel = "disable".parse().unwrap();
    assert_eq!(level.effort, ReasoningEffort::Off);
    assert!(!level.is_enabled());

    let level: ThinkingLevel = "auto".parse().unwrap();
    assert_eq!(level.effort, ReasoningEffort::Auto);
    assert!(level.is_enabled());
}

#[test]
fn test_reasoning_effort_from_str_aliases() {
    assert_eq!(
        "max".parse::<ReasoningEffort>().unwrap(),
        ReasoningEffort::XHigh
    );
    assert_eq!(
        "xhigh".parse::<ReasoningEffort>().unwrap(),
        ReasoningEffort::XHigh
    );
    assert_eq!(
        "x_high".parse::<ReasoningEffort>().unwrap(),
        ReasoningEffort::XHigh
    );
    assert_eq!(
        "off".parse::<ReasoningEffort>().unwrap(),
        ReasoningEffort::Off
    );
    assert_eq!(
        "disabled".parse::<ReasoningEffort>().unwrap(),
        ReasoningEffort::Off
    );
}

#[test]
fn test_reasoning_effort_display_round_trip() {
    for variant in [
        ReasoningEffort::Off,
        ReasoningEffort::Auto,
        ReasoningEffort::Minimal,
        ReasoningEffort::Low,
        ReasoningEffort::Medium,
        ReasoningEffort::High,
        ReasoningEffort::XHigh,
    ] {
        let s = variant.to_string();
        assert_eq!(s.parse::<ReasoningEffort>().unwrap(), variant, "round-trip");
    }
}

#[test]
fn test_thinking_level_serde_roundtrip() {
    let level = ThinkingLevel::with_budget(ReasoningEffort::High, 16000);
    let json = serde_json::to_string(&level).unwrap();
    let parsed: ThinkingLevel = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, level);
}
