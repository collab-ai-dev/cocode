use std::collections::HashMap;

use vercel_ai_provider::ReasoningLevel;
use vercel_ai_provider::Warning;

use super::*;

#[test]
fn is_custom_reasoning_returns_false_for_none() {
    assert!(!is_custom_reasoning(None));
}

#[test]
fn is_custom_reasoning_returns_false_for_provider_default() {
    assert!(!is_custom_reasoning(Some(ReasoningLevel::ProviderDefault)));
}

#[test]
fn is_custom_reasoning_returns_true_for_none_level() {
    assert!(is_custom_reasoning(Some(ReasoningLevel::Off)));
}

#[test]
fn is_custom_reasoning_returns_true_for_all_levels() {
    for level in [
        ReasoningLevel::Minimal,
        ReasoningLevel::Low,
        ReasoningLevel::Medium,
        ReasoningLevel::High,
        ReasoningLevel::Xhigh,
    ] {
        assert!(
            is_custom_reasoning(Some(level)),
            "expected true for {level:?}"
        );
    }
}

#[test]
fn effort_map_returns_mapped_value() {
    let map = HashMap::from([(ReasoningLevel::High, "high")]);
    let mut warnings = Vec::new();
    let result = map_reasoning_to_provider_effort(ReasoningLevel::High, &map, &mut warnings);
    assert_eq!(result, Some("high".to_string()));
    assert!(warnings.is_empty());
}

#[test]
fn effort_map_pushes_compatibility_warning_when_different() {
    let map = HashMap::from([(ReasoningLevel::Minimal, "low")]);
    let mut warnings = Vec::new();
    let result = map_reasoning_to_provider_effort(ReasoningLevel::Minimal, &map, &mut warnings);
    assert_eq!(result, Some("low".to_string()));
    assert_eq!(warnings.len(), 1);
    assert!(matches!(&warnings[0], Warning::Compatibility { .. }));
}

#[test]
fn effort_map_pushes_unsupported_warning_for_missing_level() {
    let map: HashMap<ReasoningLevel, &str> = HashMap::new();
    let mut warnings = Vec::new();
    let result = map_reasoning_to_provider_effort(ReasoningLevel::Xhigh, &map, &mut warnings);
    assert_eq!(result, None);
    assert_eq!(warnings.len(), 1);
    assert!(matches!(&warnings[0], Warning::Unsupported { .. }));
}

#[test]
fn budget_uses_default_percentages() {
    let mut warnings = Vec::new();
    let result = map_reasoning_to_provider_budget(
        ReasoningLevel::Medium,
        /*max_output_tokens*/ 100_000,
        /*max_reasoning_budget*/ 100_000,
        None,
        None,
        &mut warnings,
    );
    // 100_000 * 0.3 = 30_000
    assert_eq!(result, Some(30_000));
    assert!(warnings.is_empty());
}

#[test]
fn budget_clamps_to_min() {
    let mut warnings = Vec::new();
    let result = map_reasoning_to_provider_budget(
        ReasoningLevel::Minimal,
        /*max_output_tokens*/ 10_000,
        /*max_reasoning_budget*/ 10_000,
        Some(1024),
        None,
        &mut warnings,
    );
    // 10_000 * 0.02 = 200, clamped to min 1024
    assert_eq!(result, Some(1024));
}

#[test]
fn budget_clamps_to_max() {
    let mut warnings = Vec::new();
    let result = map_reasoning_to_provider_budget(
        ReasoningLevel::Xhigh,
        /*max_output_tokens*/ 100_000,
        /*max_reasoning_budget*/ 50_000,
        None,
        None,
        &mut warnings,
    );
    // 100_000 * 0.9 = 90_000, clamped to max 50_000
    assert_eq!(result, Some(50_000));
}

#[test]
fn budget_unsupported_level_returns_none() {
    let empty: HashMap<ReasoningLevel, f64> = HashMap::new();
    let mut warnings = Vec::new();
    let result = map_reasoning_to_provider_budget(
        ReasoningLevel::High,
        100_000,
        100_000,
        None,
        Some(&empty),
        &mut warnings,
    );
    assert_eq!(result, None);
    assert_eq!(warnings.len(), 1);
}
