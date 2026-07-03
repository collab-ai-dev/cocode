use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

fn sel(provider: &str, model_id: &str) -> ProviderModelSelection {
    ProviderModelSelection {
        provider: provider.into(),
        model_id: model_id.into(),
    }
}

/// Model identities of the fallback chain, dropping per-slot effort —
/// used by the pre-effort tests that only assert on model identity.
fn fallback_models(slots: &RoleSlots<ProviderModelSelection>) -> Vec<ProviderModelSelection> {
    slots.fallbacks.iter().map(|s| s.model.clone()).collect()
}

#[test]
fn test_deserialize_bare_string_form() {
    let slots: RoleSlots<ProviderModelSelection> =
        serde_json::from_value(json!("anthropic/claude-opus-4-6")).unwrap();
    assert_eq!(slots.primary.model, sel("anthropic", "claude-opus-4-6"));
    assert_eq!(slots.primary.effort, None);
    assert!(slots.fallbacks.is_empty());
    assert_eq!(slots.policy, FallbackPolicy::default());
}

#[test]
fn test_deserialize_slot_effort_on_primary_and_fallback() {
    let slots: RoleSlots<ProviderModelSelection> = serde_json::from_value(json!({
        "primary":   { "provider": "openai-aipaas", "model_id": "gpt-5-5", "effort": "high" },
        "fallbacks": [ { "provider": "openai", "model_id": "gpt-5-4", "effort": "medium" } ]
    }))
    .unwrap();
    assert_eq!(slots.primary.model, sel("openai-aipaas", "gpt-5-5"));
    assert_eq!(slots.primary.effort, Some(ReasoningEffort::High));
    assert_eq!(slots.fallbacks.len(), 1);
    assert_eq!(slots.fallbacks[0].model, sel("openai", "gpt-5-4"));
    assert_eq!(slots.fallbacks[0].effort, Some(ReasoningEffort::Medium));
}

#[test]
fn test_deserialize_slot_effort_off_is_explicit() {
    // `effort: "off"` is a first-class explicit value, distinct from an
    // absent effort (which defers to the model default).
    let slots: RoleSlots<ProviderModelSelection> = serde_json::from_value(json!({
        "primary": { "provider": "openai", "model_id": "gpt-5-4", "effort": "off" }
    }))
    .unwrap();
    assert_eq!(slots.primary.effort, Some(ReasoningEffort::Off));
}

#[test]
fn test_deserialize_slot_effort_aliases() {
    for (wire, expected) in [
        ("disable", ReasoningEffort::Off),
        ("max", ReasoningEffort::XHigh),
        ("x_high", ReasoningEffort::XHigh),
    ] {
        let slots: RoleSlots<ProviderModelSelection> = serde_json::from_value(json!({
            "primary": { "provider": "openai", "model_id": "gpt-5-4", "effort": wire }
        }))
        .unwrap();
        assert_eq!(slots.primary.effort, Some(expected), "wire form `{wire}`");
    }
}

#[test]
fn test_deserialize_slot_effort_null_is_none() {
    let slots: RoleSlots<ProviderModelSelection> = serde_json::from_value(json!({
        "primary": { "provider": "openai", "model_id": "gpt-5-4", "effort": null }
    }))
    .unwrap();
    assert_eq!(slots.primary.effort, None);
}

#[test]
fn test_deserialize_slot_effort_rejects_invalid() {
    let err = serde_json::from_value::<RoleSlots<ProviderModelSelection>>(json!({
        "primary": { "provider": "openai", "model_id": "gpt-5-4", "effort": "turbo" }
    }))
    .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("effort") && msg.contains("turbo"),
        "expected actionable effort error, got: {msg}"
    );
}

#[test]
fn test_deserialize_role_level_effort_gets_guiding_error() {
    // The most likely misplacement — `effort` at the role level (next
    // to `primary`) instead of inside a slot object — must point the
    // user at the correct placement, not emit a generic unknown-field.
    let err = serde_json::from_value::<RoleSlots<ProviderModelSelection>>(json!({
        "primary": { "provider": "openai", "model_id": "gpt-5-4" },
        "effort": "high"
    }))
    .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("belongs on a slot object"),
        "expected guiding placement error, got: {msg}"
    );
}

#[test]
fn test_deserialize_slot_effort_rejects_non_string() {
    let err = serde_json::from_value::<RoleSlots<ProviderModelSelection>>(json!({
        "primary": { "provider": "openai", "model_id": "gpt-5-4", "effort": 3 }
    }))
    .unwrap_err();
    assert!(
        err.to_string().contains("effort"),
        "expected effort type error, got: {err}"
    );
}

#[test]
fn test_deserialize_bare_string_rejects_missing_slash() {
    let err = serde_json::from_value::<RoleSlots<ProviderModelSelection>>(json!("claude-opus-4-6"))
        .unwrap_err();
    assert!(
        err.to_string().contains("provider/model_id"),
        "expected actionable error, got: {err}"
    );
}

#[test]
fn test_deserialize_bare_string_rejects_empty_half() {
    let err = serde_json::from_value::<RoleSlots<ProviderModelSelection>>(json!("anthropic/"))
        .unwrap_err();
    assert!(err.to_string().contains("provider/model_id"));
    let err = serde_json::from_value::<RoleSlots<ProviderModelSelection>>(json!("/model-id"))
        .unwrap_err();
    assert!(err.to_string().contains("provider/model_id"));
}

#[test]
fn test_deserialize_nested_with_single_fallback() {
    let slots: RoleSlots<ProviderModelSelection> = serde_json::from_value(json!({
        "primary":  { "provider": "anthropic", "model_id": "claude-opus-4-6" },
        "fallback": { "provider": "anthropic", "model_id": "claude-sonnet-4-6" }
    }))
    .unwrap();
    assert_eq!(slots.primary.model, sel("anthropic", "claude-opus-4-6"));
    assert_eq!(
        fallback_models(&slots),
        vec![sel("anthropic", "claude-sonnet-4-6")]
    );
}

#[test]
fn test_deserialize_nested_with_plural_fallbacks() {
    let slots: RoleSlots<ProviderModelSelection> = serde_json::from_value(json!({
        "primary":   { "provider": "anthropic", "model_id": "claude-opus-4-6" },
        "fallbacks": [
            { "provider": "anthropic", "model_id": "claude-sonnet-4-6" },
            { "provider": "openai",    "model_id": "gpt-5" }
        ]
    }))
    .unwrap();
    assert_eq!(
        fallback_models(&slots),
        vec![
            sel("anthropic", "claude-sonnet-4-6"),
            sel("openai", "gpt-5"),
        ]
    );
}

#[test]
fn test_deserialize_rejects_flat_object_form() {
    let err = serde_json::from_value::<RoleSlots<ProviderModelSelection>>(json!({
        "provider": "anthropic",
        "model_id": "claude-opus-4-6"
    }))
    .unwrap_err();
    assert!(
        err.to_string().contains("nested form"),
        "expected nested-form error, got: {err}"
    );
}

#[test]
fn test_deserialize_nested_accepts_slash_string_primary() {
    // QoL: a nested form can use the `provider/model_id` shorthand for
    // its slots (no effort) — so a user adding only `effort` to a
    // fallback doesn't have to expand every slot into an object.
    let slots: RoleSlots<ProviderModelSelection> = serde_json::from_value(json!({
        "primary":   "anthropic/claude-opus-4-6",
        "fallbacks": [ "openai/gpt-5" ]
    }))
    .unwrap();
    assert_eq!(slots.primary.model, sel("anthropic", "claude-opus-4-6"));
    assert_eq!(slots.primary.effort, None);
    assert_eq!(fallback_models(&slots), vec![sel("openai", "gpt-5")]);
}

#[test]
fn test_deserialize_mixed_slash_string_and_object_slots() {
    // Primary as shorthand, fallback as object carrying an effort.
    let slots: RoleSlots<ProviderModelSelection> = serde_json::from_value(json!({
        "primary":   "openai/gpt-5-4",
        "fallbacks": [ { "provider": "openai", "model_id": "gpt-5-mini", "effort": "low" } ]
    }))
    .unwrap();
    assert_eq!(slots.primary.model, sel("openai", "gpt-5-4"));
    assert_eq!(slots.primary.effort, None);
    assert_eq!(slots.fallbacks[0].effort, Some(ReasoningEffort::Low));
}

#[test]
fn test_deserialize_nested_rejects_both_singular_and_plural() {
    let err = serde_json::from_value::<RoleSlots<ProviderModelSelection>>(json!({
        "primary":   { "provider": "anthropic", "model_id": "opus" },
        "fallback":  { "provider": "anthropic", "model_id": "sonnet" },
        "fallbacks": [{ "provider": "openai", "model_id": "gpt-5" }]
    }))
    .unwrap_err();
    assert!(
        err.to_string().contains("not both"),
        "expected not-both message, got: {err}"
    );
}

#[test]
fn test_deserialize_nested_rejects_unknown_field() {
    // `deny_unknown_fields` on the nested variant catches typos in
    // field names — this is the whole point of the custom
    // deserializer (vs raw untagged which would silently fall
    // through to the Legacy variant).
    let err = serde_json::from_value::<RoleSlots<ProviderModelSelection>>(json!({
        "primary":  { "provider": "anthropic", "model_id": "opus" },
        "fallbck":  { "provider": "anthropic", "model_id": "sonnet" }
    }))
    .unwrap_err();
    // Because the untagged enum tries variants in order, we expect
    // either "unknown field" or "did not match" — both indicate
    // the typo was caught.
    let msg = err.to_string();
    assert!(
        msg.contains("unknown field") || msg.contains("did not match"),
        "expected unknown-field or no-variant error, got: {msg}"
    );
}

#[test]
fn test_deserialize_policy_optional() {
    let slots: RoleSlots<ProviderModelSelection> = serde_json::from_value(json!({
        "primary":  { "provider": "anthropic", "model_id": "opus" },
        "policy": {
            "exhausted_retry": {
                "max_cycles": 4,
                "initial_backoff_secs": 3,
                "max_backoff_secs": 20
            },
            "recovery": {
                "initial_backoff_secs": 30,
                "max_backoff_secs": 600,
                "max_attempts": 5
            }
        }
    }))
    .unwrap();
    assert_eq!(slots.policy.exhausted_retry.max_cycles, 4);
    assert_eq!(slots.policy.exhausted_retry.initial_backoff_secs, 3);
    assert_eq!(slots.policy.exhausted_retry.max_backoff_secs, 20);
    assert_eq!(slots.policy.recovery.initial_backoff_secs, 30);
    assert_eq!(slots.policy.recovery.max_backoff_secs, 600);
    assert_eq!(slots.policy.recovery.max_attempts, 5);
}

#[test]
fn test_deserialize_rejects_old_recovery_field() {
    let err = serde_json::from_value::<RoleSlots<ProviderModelSelection>>(json!({
        "primary":  { "provider": "anthropic", "model_id": "opus" },
        "recovery": { "initial_backoff_secs": 30, "max_backoff_secs": 600, "max_attempts": 5 }
    }))
    .unwrap_err();
    assert!(
        err.to_string().contains("unknown field `recovery`"),
        "expected unknown recovery field, got: {err}"
    );
}

#[test]
fn test_fallback_policy_default_values() {
    let p = FallbackPolicy::default();
    assert_eq!(p.exhausted_retry.max_cycles, 2);
    assert_eq!(p.exhausted_retry.initial_backoff_secs, 2);
    assert_eq!(p.exhausted_retry.max_backoff_secs, 30);
    assert_eq!(p.recovery.initial_backoff_secs, 60);
    assert_eq!(p.recovery.max_backoff_secs, 1_800);
    assert_eq!(p.recovery.max_attempts, 10);
    assert_eq!(
        p.exhausted_retry.initial_backoff(),
        std::time::Duration::from_secs(2)
    );
    assert_eq!(
        p.exhausted_retry.max_backoff(),
        std::time::Duration::from_secs(30)
    );
    assert_eq!(
        p.recovery.initial_backoff(),
        std::time::Duration::from_secs(60)
    );
    assert_eq!(
        p.recovery.max_backoff(),
        std::time::Duration::from_secs(1_800)
    );
}

#[test]
fn test_fallback_policy_clamps_values() {
    let exhausted = ExhaustedRetryPolicy {
        max_cycles: 0,
        initial_backoff_secs: 10,
        max_backoff_secs: 1,
    };
    assert_eq!(exhausted.max_cycles(), 1);
    assert_eq!(exhausted.max_backoff(), std::time::Duration::from_secs(10));

    let recovery = RecoveryProbePolicy {
        initial_backoff_secs: 300,
        max_backoff_secs: 60,
        max_attempts: -1,
    };
    assert_eq!(recovery.max_attempts(), 0);
    assert_eq!(recovery.max_backoff(), std::time::Duration::from_secs(300));
}

#[test]
fn test_try_map_lifts_selection_to_spec_like_type() {
    // Smoke-test try_map by lifting ProviderModelSelection → a trivial
    // newtype; catches bugs in the primary+fallbacks mapping order
    // without needing ModelSpec + ProviderApi wiring here.
    let slots = RoleSlots::new(sel("anthropic", "opus"))
        .with_fallback(sel("anthropic", "sonnet"))
        .with_fallback(sel("openai", "gpt-5"));

    let mapped: RoleSlots<String> = slots
        .try_map::<_, std::convert::Infallible, _>(|s| {
            Ok(format!("{}::{}", s.provider, s.model_id))
        })
        .unwrap();

    assert_eq!(mapped.primary.model, "anthropic::opus");
    assert_eq!(
        mapped
            .fallbacks
            .iter()
            .map(|s| s.model.clone())
            .collect::<Vec<_>>(),
        vec!["anthropic::sonnet".to_string(), "openai::gpt-5".to_string()]
    );
}

#[test]
fn test_try_map_carries_slot_effort_through() {
    // The model maps but each slot's effort rides along unchanged.
    let slots = RoleSlots {
        primary: RoleSlot {
            model: sel("openai", "gpt-5-5"),
            effort: Some(ReasoningEffort::High),
        },
        fallbacks: vec![RoleSlot {
            model: sel("openai", "gpt-5-4"),
            effort: Some(ReasoningEffort::Medium),
        }],
        policy: FallbackPolicy::default(),
    };
    let mapped: RoleSlots<String> = slots
        .try_map::<_, std::convert::Infallible, _>(|s| Ok(s.model_id))
        .unwrap();
    assert_eq!(mapped.primary.effort, Some(ReasoningEffort::High));
    assert_eq!(mapped.fallbacks[0].effort, Some(ReasoningEffort::Medium));
}

#[test]
fn test_without_effort_strips_all_slots() {
    let slots = RoleSlots {
        primary: RoleSlot {
            model: sel("openai", "gpt-5-5"),
            effort: Some(ReasoningEffort::High),
        },
        fallbacks: vec![RoleSlot {
            model: sel("openai", "gpt-5-4"),
            effort: Some(ReasoningEffort::Medium),
        }],
        policy: FallbackPolicy::default(),
    };
    let stripped = slots.without_effort();
    assert_eq!(stripped.primary.effort, None);
    assert_eq!(stripped.fallbacks[0].effort, None);
    // Models are preserved.
    assert_eq!(stripped.primary.model, sel("openai", "gpt-5-5"));
    assert_eq!(stripped.fallbacks[0].model, sel("openai", "gpt-5-4"));
}

#[test]
fn test_try_map_propagates_first_error() {
    let slots = RoleSlots::new(sel("anthropic", "opus"))
        .with_fallback(sel("bad", "sonnet"))
        .with_fallback(sel("openai", "gpt-5"));
    let err: Result<RoleSlots<String>, &str> = slots.try_map(|s| {
        if s.provider == "bad" {
            Err("bad provider")
        } else {
            Ok(s.model_id)
        }
    });
    assert_eq!(err, Err("bad provider"));
}

#[test]
fn test_serialize_nested_form_skips_empty_fallbacks_and_recovery() {
    let slots = RoleSlots::new(sel("anthropic", "opus"));
    let json_val = serde_json::to_value(&slots).unwrap();
    assert_eq!(
        json_val,
        json!({ "primary": { "provider": "anthropic", "model_id": "opus" } })
    );
}

#[test]
fn test_serialize_roundtrip_preserves_multi_fallback_and_recovery() {
    let policy = FallbackPolicy {
        exhausted_retry: ExhaustedRetryPolicy {
            max_cycles: 3,
            initial_backoff_secs: 1,
            max_backoff_secs: 8,
        },
        recovery: RecoveryProbePolicy {
            initial_backoff_secs: 10,
            max_backoff_secs: 100,
            max_attempts: 2,
        },
    };
    let orig = RoleSlots::new(sel("anthropic", "opus"))
        .with_fallbacks(vec![sel("anthropic", "sonnet"), sel("openai", "gpt-5")])
        .with_policy(policy);
    let json_val = serde_json::to_value(&orig).unwrap();
    let back: RoleSlots<ProviderModelSelection> = serde_json::from_value(json_val).unwrap();
    assert_eq!(back, orig);
}

#[test]
fn test_serialize_roundtrip_preserves_slot_effort() {
    let orig = RoleSlots {
        primary: RoleSlot {
            model: sel("openai-aipaas", "gpt-5-5"),
            effort: Some(ReasoningEffort::High),
        },
        fallbacks: vec![RoleSlot {
            model: sel("openai", "gpt-5-4"),
            effort: Some(ReasoningEffort::Medium),
        }],
        policy: FallbackPolicy::default(),
    };
    let json_val = serde_json::to_value(&orig).unwrap();
    // Effort surfaces as a flat `effort` key on each slot object.
    assert_eq!(json_val["primary"]["effort"], json!("high"));
    assert_eq!(json_val["fallbacks"][0]["effort"], json!("medium"));
    let back: RoleSlots<ProviderModelSelection> = serde_json::from_value(json_val).unwrap();
    assert_eq!(back, orig);
}
