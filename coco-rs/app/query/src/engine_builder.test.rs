use pretty_assertions::assert_eq;

use super::*;
use coco_types::ReasoningEffort;
use coco_types::ThinkingLevel;

/// `CacheSafeParams.effort` capture precedence: the explicit engine
/// config level (layer 1, Ctrl+T / picker) wins over the serving
/// slot's bound effort (layer 2, `models.<role>.<slot>.effort`);
/// layer 2 fills when layer 1 is absent; both absent stays `None`
/// (parent ran on the model default — the fork resolves the same
/// default, so parity needs no snapshot).
#[test]
fn test_cache_safe_params_effort_capture_precedence() {
    let history = MessageHistory::new();

    // Layer 1 wins over layer 2.
    let config = QueryEngineConfig {
        thinking_level: Some(ThinkingLevel::high()),
        ..Default::default()
    };
    let params = QueryEngine::cache_safe_params_from_parts(
        &config,
        "anthropic".into(),
        Some(ReasoningEffort::Medium),
        &history,
    );
    assert_eq!(params.effort, Some(ReasoningEffort::High));

    // Layer 2 fills when layer 1 is absent.
    let config = QueryEngineConfig::default();
    let params = QueryEngine::cache_safe_params_from_parts(
        &config,
        "anthropic".into(),
        Some(ReasoningEffort::Medium),
        &history,
    );
    assert_eq!(params.effort, Some(ReasoningEffort::Medium));

    // Both absent → None (model default on both sides).
    let params = QueryEngine::cache_safe_params_from_parts(
        &config,
        "anthropic".into(),
        /*slot_effort*/ None,
        &history,
    );
    assert_eq!(params.effort, None);

    // An explicit Off is captured verbatim — the fork must mirror
    // "thinking disabled" too, since enable/disable is itself a
    // cache-keyed thinking param on Anthropic.
    let config = QueryEngineConfig {
        thinking_level: Some(ThinkingLevel::disable()),
        ..Default::default()
    };
    let params = QueryEngine::cache_safe_params_from_parts(
        &config,
        "anthropic".into(),
        Some(ReasoningEffort::Medium),
        &history,
    );
    assert_eq!(params.effort, Some(ReasoningEffort::Off));
}
