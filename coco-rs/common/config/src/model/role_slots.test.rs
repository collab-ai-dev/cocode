use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

fn sel(provider: &str, model_id: &str) -> ProviderModelSelection {
    ProviderModelSelection {
        provider: provider.into(),
        model_id: model_id.into(),
    }
}

#[test]
fn test_deserialize_bare_string_form() {
    let slots: RoleSlots<ProviderModelSelection> =
        serde_json::from_value(json!("anthropic/claude-opus-4-6")).unwrap();
    assert_eq!(slots.primary, sel("anthropic", "claude-opus-4-6"));
    assert!(slots.fallbacks.is_empty());
    assert_eq!(slots.policy, FallbackPolicy::default());
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
    assert_eq!(slots.primary, sel("anthropic", "claude-opus-4-6"));
    assert_eq!(slots.fallbacks, vec![sel("anthropic", "claude-sonnet-4-6")]);
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
        slots.fallbacks,
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
fn test_deserialize_nested_rejects_string_primary() {
    let err = serde_json::from_value::<RoleSlots<ProviderModelSelection>>(json!({
        "primary": "anthropic/claude-opus-4-6"
    }))
    .unwrap_err();
    assert!(
        err.to_string().contains("must be an object"),
        "expected object-shape error, got: {err}"
    );
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

    assert_eq!(mapped.primary, "anthropic::opus");
    assert_eq!(
        mapped.fallbacks,
        vec!["anthropic::sonnet".to_string(), "openai::gpt-5".to_string()]
    );
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
