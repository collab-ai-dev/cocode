use super::*;
use crate::secret::RedactedSecret;

#[test]
fn partial_rejects_user_written_name() {
    // Plan §15 Group B claim #2: identity is the parent map key. The
    // partial overlay MUST NOT accept a `name` field at parse time.
    let json = r#"{"name": "azure-east", "api": "openai", "env_key": "OPENAI_API_KEY", "base_url": "https://api"}"#;
    let err = serde_json::from_str::<PartialProviderConfig>(json).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("name") || msg.contains("unknown field"),
        "expected unknown-field rejection of `name`, got: {msg}"
    );
}

#[test]
fn from_partial_writes_name_from_map_key() {
    // The single write site for `ProviderConfig.name`.
    let partial = PartialProviderConfig {
        api: Some(coco_types::ProviderApi::Openai),
        env_key: Some("OPENAI_API_KEY".into()),
        base_url: Some("https://api.openai.com".into()),
        ..Default::default()
    };
    let resolved = ProviderConfig::from_partial("my-instance", &partial).unwrap();
    assert_eq!(resolved.name, "my-instance");
}

#[test]
fn merge_partial_does_not_touch_name() {
    let partial = PartialProviderConfig {
        api: Some(coco_types::ProviderApi::Openai),
        env_key: Some("OPENAI_API_KEY".into()),
        base_url: Some("https://api.openai.com".into()),
        ..Default::default()
    };
    let mut resolved = ProviderConfig::from_partial("anchor", &partial).unwrap();
    let overlay = PartialProviderConfig {
        base_url: Some("https://corp-proxy".into()),
        ..Default::default()
    };
    resolved.merge_partial(&overlay).unwrap();
    assert_eq!(resolved.name, "anchor");
    assert_eq!(resolved.base_url, "https://corp-proxy");
}

#[test]
fn debug_redacts_api_key() {
    let partial = PartialProviderConfig {
        api: Some(coco_types::ProviderApi::Openai),
        env_key: Some("OPENAI_API_KEY".into()),
        api_key: Some(RedactedSecret::new("sk-real-secret-bytes")),
        base_url: Some("https://api.openai.com".into()),
        ..Default::default()
    };
    let cfg = ProviderConfig::from_partial("openai", &partial).unwrap();
    let rendered = format!("{cfg:?}");
    assert!(!rendered.contains("sk-real-secret-bytes"));
    assert!(rendered.contains("<redacted>"));
}

#[test]
fn merge_partial_does_not_coerce_api() {
    // Plan §15 invariant: an overlay that omits `api` does NOT
    // silently coerce the resolved `api` to a serde default.
    let mut resolved = ProviderConfig::from_partial(
        "openai",
        &PartialProviderConfig {
            api: Some(coco_types::ProviderApi::Openai),
            env_key: Some("OPENAI_API_KEY".into()),
            base_url: Some("https://api.openai.com".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let overlay = PartialProviderConfig {
        // api is None — do not coerce
        base_url: Some("https://corp-proxy".into()),
        ..Default::default()
    };
    resolved.merge_partial(&overlay).unwrap();
    assert_eq!(resolved.api, coco_types::ProviderApi::Openai);
}

#[test]
fn from_partial_rejects_negative_timeout() {
    // Negative `timeout_secs` is unambiguously a typo; surface as a
    // typed startup error rather than silently disabling the timeout.
    let partial = PartialProviderConfig {
        api: Some(coco_types::ProviderApi::Openai),
        env_key: Some("OPENAI_API_KEY".into()),
        base_url: Some("https://api.openai.com".into()),
        timeout_secs: Some(-600),
        ..Default::default()
    };
    let err = ProviderConfig::from_partial("openai", &partial).unwrap_err();
    assert!(matches!(
        err,
        crate::error::ConfigError::InvalidTimeoutSecs { ref name, value: -600 }
            if name == "openai"
    ));
}

#[test]
fn merge_partial_rejects_negative_timeout() {
    let mut resolved = ProviderConfig::from_partial(
        "openai",
        &PartialProviderConfig {
            api: Some(coco_types::ProviderApi::Openai),
            env_key: Some("OPENAI_API_KEY".into()),
            base_url: Some("https://api.openai.com".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let overlay = PartialProviderConfig {
        timeout_secs: Some(-1),
        ..Default::default()
    };
    let err = resolved.merge_partial(&overlay).unwrap_err();
    assert!(matches!(
        err,
        crate::error::ConfigError::InvalidTimeoutSecs { value: -1, .. }
    ));
}

#[test]
fn providers_json_round_trip_is_byte_stable() {
    // Plan §15 Group B claim #7: BTreeMap on disk produces stable
    // serialisation. Round-trip 100x and assert byte-identical.
    use std::collections::BTreeMap;

    let mut catalog: BTreeMap<String, PartialProviderConfig> = BTreeMap::new();
    catalog.insert(
        "anthropic-corp".into(),
        PartialProviderConfig {
            api: Some(coco_types::ProviderApi::Anthropic),
            env_key: Some("CORP_KEY".into()),
            base_url: Some("https://corp.example".into()),
            ..Default::default()
        },
    );
    catalog.insert(
        "azure-east".into(),
        PartialProviderConfig {
            api: Some(coco_types::ProviderApi::Openai),
            env_key: Some("AZURE_KEY".into()),
            base_url: Some("https://azure.example/v1".into()),
            ..Default::default()
        },
    );

    let mut current = serde_json::to_string_pretty(&catalog).unwrap();
    for _ in 0..100 {
        let parsed: BTreeMap<String, PartialProviderConfig> =
            serde_json::from_str(&current).unwrap();
        let next = serde_json::to_string_pretty(&parsed).unwrap();
        assert_eq!(current, next, "providers.json must be byte-stable");
        current = next;
    }
}

#[test]
fn from_partial_missing_api_returns_typed_error() {
    let partial = PartialProviderConfig {
        env_key: Some("X".into()),
        base_url: Some("https://x".into()),
        ..Default::default()
    };
    let err = ProviderConfig::from_partial("unknown", &partial).unwrap_err();
    matches!(
        err,
        crate::error::ConfigError::IncompleteProviderEntry {
            field: crate::error::ConfigField::Api,
            ..
        }
    );
}

#[test]
fn merge_partial_provider_options_is_key_by_key() {
    // Same shape semantics as `client_options.headers`: overlay wins
    // per key, never wholesale-replaces. A consumer downstream
    // (e.g. `vercel-ai-anthropic::parse_provider_options`) sees the
    // merged result.
    use serde_json::json;
    let mut resolved = ProviderConfig::from_partial(
        "anthropic",
        &PartialProviderConfig {
            api: Some(coco_types::ProviderApi::Anthropic),
            env_key: Some("ANTHROPIC_API_KEY".into()),
            base_url: Some("https://api.anthropic.com".into()),
            provider_options: Some(
                [
                    ("experimental_betas".to_string(), json!(false)),
                    ("non_interactive".to_string(), json!(true)),
                ]
                .into_iter()
                .collect(),
            ),
            ..Default::default()
        },
    )
    .unwrap();
    let overlay = PartialProviderConfig {
        // Update one key, add another, leave the third untouched.
        provider_options: Some(
            [
                ("experimental_betas".to_string(), json!(true)),
                ("show_thinking_summaries".to_string(), json!(true)),
            ]
            .into_iter()
            .collect(),
        ),
        ..Default::default()
    };
    resolved.merge_partial(&overlay).unwrap();
    assert_eq!(
        resolved.provider_options.get("experimental_betas"),
        Some(&json!(true))
    );
    assert_eq!(
        resolved.provider_options.get("non_interactive"),
        Some(&json!(true)),
        "untouched key must survive the overlay"
    );
    assert_eq!(
        resolved.provider_options.get("show_thinking_summaries"),
        Some(&json!(true))
    );
}

#[test]
fn merge_partial_provider_options_null_removes_key() {
    // `Value::Null` is the only opt-out signal a downstream overlay
    // can use to remove a key set higher up — same convention as
    // `client_options.headers`.
    use serde_json::json;
    let mut resolved = ProviderConfig::from_partial(
        "anthropic",
        &PartialProviderConfig {
            api: Some(coco_types::ProviderApi::Anthropic),
            env_key: Some("ANTHROPIC_API_KEY".into()),
            base_url: Some("https://api.anthropic.com".into()),
            provider_options: Some(
                [("experimental_betas".to_string(), json!(false))]
                    .into_iter()
                    .collect(),
            ),
            ..Default::default()
        },
    )
    .unwrap();
    let overlay = PartialProviderConfig {
        provider_options: Some(
            [("experimental_betas".to_string(), serde_json::Value::Null)]
                .into_iter()
                .collect(),
        ),
        ..Default::default()
    };
    resolved.merge_partial(&overlay).unwrap();
    assert!(
        !resolved.provider_options.contains_key("experimental_betas"),
        "Value::Null must remove the key"
    );
}
