use std::collections::HashMap;

use serde_json::json;
use vercel_ai_provider::ProviderMetadata;

use super::*;

fn make_metadata(cache_control: serde_json::Value) -> Option<ProviderMetadata> {
    let mut meta = HashMap::new();
    meta.insert("anthropic".into(), json!({"cacheControl": cache_control}));
    Some(ProviderMetadata(meta))
}

#[test]
fn returns_none_when_no_provider_metadata() {
    let mut validator = CacheControlValidator::new();
    let result = validator.get_cache_control(
        &None,
        CacheContext {
            type_name: "test",
            can_cache: true,
        },
    );
    assert!(result.is_none());
    assert!(validator.into_warnings().is_empty());
}

#[test]
fn returns_none_when_no_cache_control_field() {
    let mut meta = HashMap::new();
    meta.insert("anthropic".into(), json!({"other": "value"}));
    let pm = Some(ProviderMetadata(meta));

    let mut validator = CacheControlValidator::new();
    let result = validator.get_cache_control(
        &pm,
        CacheContext {
            type_name: "test",
            can_cache: true,
        },
    );
    assert!(result.is_none());
}

#[test]
fn returns_cache_control_value() {
    let pm = make_metadata(json!({"type": "ephemeral"}));
    let mut validator = CacheControlValidator::new();
    let result = validator.get_cache_control(
        &pm,
        CacheContext {
            type_name: "system message",
            can_cache: true,
        },
    );
    assert_eq!(result, Some(json!({"type": "ephemeral"})));
}

#[test]
fn accepts_snake_case_cache_control() {
    let mut meta = HashMap::new();
    meta.insert(
        "anthropic".into(),
        json!({"cache_control": {"type": "ephemeral"}}),
    );
    let pm = Some(ProviderMetadata(meta));

    let mut validator = CacheControlValidator::new();
    let result = validator.get_cache_control(
        &pm,
        CacheContext {
            type_name: "test",
            can_cache: true,
        },
    );
    assert_eq!(result, Some(json!({"type": "ephemeral"})));
}

#[test]
fn warns_and_returns_none_for_non_cacheable_context() {
    let pm = make_metadata(json!({"type": "ephemeral"}));
    let mut validator = CacheControlValidator::new();
    let result = validator.get_cache_control(
        &pm,
        CacheContext {
            type_name: "thinking block",
            can_cache: false,
        },
    );
    assert!(result.is_none());
    let warnings = validator.into_warnings();
    assert_eq!(warnings.len(), 1);
}

#[test]
fn enforces_max_four_breakpoints() {
    let mut validator = CacheControlValidator::new();
    let pm = make_metadata(json!({"type": "ephemeral"}));

    // First 4 should succeed
    for i in 0..4 {
        let result = validator.get_cache_control(
            &pm,
            CacheContext {
                type_name: &format!("part {i}"),
                can_cache: true,
            },
        );
        assert!(result.is_some(), "breakpoint {i} should succeed");
    }

    // 5th should fail
    let result = validator.get_cache_control(
        &pm,
        CacheContext {
            type_name: "part 4",
            can_cache: true,
        },
    );
    assert!(result.is_none());

    let warnings = validator.into_warnings();
    assert_eq!(warnings.len(), 1);
}

#[test]
fn multiple_breakpoints_over_limit_produce_multiple_warnings() {
    let mut validator = CacheControlValidator::new();
    let pm = make_metadata(json!({"type": "ephemeral"}));

    // Use up 4 breakpoints
    for _ in 0..4 {
        validator.get_cache_control(
            &pm,
            CacheContext {
                type_name: "part",
                can_cache: true,
            },
        );
    }

    // 5th and 6th over limit
    validator.get_cache_control(
        &pm,
        CacheContext {
            type_name: "extra 1",
            can_cache: true,
        },
    );
    validator.get_cache_control(
        &pm,
        CacheContext {
            type_name: "extra 2",
            can_cache: true,
        },
    );

    let warnings = validator.into_warnings();
    assert_eq!(warnings.len(), 2);
}
