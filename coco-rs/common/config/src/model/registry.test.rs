use super::*;
use crate::positive::PositiveTokens;
use crate::provider::PartialProviderConfig;
use crate::provider::ProviderConfig;
use crate::provider::model_override::PartialProviderModelOverride;
use coco_types::ProviderApi;
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;
use std::fs;
use tempfile::TempDir;

fn empty_catalog() -> BTreeMap<String, PartialModelInfo> {
    BTreeMap::new()
}

fn provider_with_model(
    name: &str,
    api: ProviderApi,
    model_id: &str,
    entry: PartialProviderModelOverride,
) -> ProviderConfig {
    let mut models = BTreeMap::new();
    models.insert(model_id.into(), entry);
    let partial = PartialProviderConfig {
        api: Some(api),
        env_key: Some("FOO_KEY".into()),
        base_url: Some("https://example".into()),
        models: Some(models),
        ..Default::default()
    };
    ProviderConfig::from_partial(name, &partial).unwrap()
}

#[test]
fn missing_context_window_surfaces_typed_error() {
    let mut providers = BTreeMap::new();
    providers.insert(
        "openai".into(),
        provider_with_model(
            "openai",
            ProviderApi::Openai,
            "gpt-99",
            PartialProviderModelOverride::default(),
        ),
    );
    let coco_home = TempDir::new().unwrap();
    let err = build_model_registry(&providers, &empty_catalog(), coco_home.path()).unwrap_err();
    assert!(
        matches!(
            err,
            ConfigError::IncompleteModelEntry {
                ref provider,
                ref model,
                field: crate::error::ConfigField::ContextWindow,
            }
            if provider == "openai" && model == "gpt-99"
        ),
        "expected IncompleteModelEntry {{ ContextWindow }}, got: {err:?}"
    );
}

#[test]
fn entry_supplies_required_fields() {
    let mut providers = BTreeMap::new();
    let entry = PartialProviderModelOverride {
        api_model_name: Some("gpt-5".into()),
        overrides: PartialModelInfo {
            context_window: Some(PositiveTokens::new(272_000)),
            max_output_tokens: Some(PositiveTokens::new(16_384)),
            ..Default::default()
        },
    };
    providers.insert(
        "openai".into(),
        provider_with_model("openai", ProviderApi::Openai, "gpt-5", entry),
    );
    let coco_home = TempDir::new().unwrap();
    let registry = build_model_registry(&providers, &empty_catalog(), coco_home.path()).unwrap();
    let resolved = registry.resolve("openai", "gpt-5").unwrap();
    assert_eq!(resolved.info.context_window.get(), 272_000);
    assert_eq!(resolved.info.max_output_tokens.get(), 16_384);
    assert_eq!(
        resolved.provider_model.api_model_name.as_deref(),
        Some("gpt-5")
    );
}

#[test]
fn lazy_resolve_from_user_catalog_when_provider_has_no_explicit_entry() {
    // `models.json` is a provider-agnostic catalog. Users can declare a
    // model there and bind via `settings.models.main = "anthropic/<id>"`
    // without mirroring into `providers.anthropic.models`. The registry
    // must lazy-synth from `user_catalog` for this case.
    let mut user_catalog = BTreeMap::new();
    user_catalog.insert(
        "deepseek-r1".into(),
        PartialModelInfo {
            context_window: Some(PositiveTokens::new(64_000)),
            max_output_tokens: Some(PositiveTokens::new(8_192)),
            temperature: Some(0.3),
            ..Default::default()
        },
    );
    // No provider declares `deepseek-r1` in cfg.models.
    let mut providers = BTreeMap::new();
    let partial = crate::provider::PartialProviderConfig {
        api: Some(ProviderApi::OpenaiCompat),
        env_key: Some("DS_KEY".into()),
        base_url: Some("https://api.deepseek.com".into()),
        ..Default::default()
    };
    providers.insert(
        "deepseek".into(),
        ProviderConfig::from_partial("deepseek", &partial).unwrap(),
    );
    let coco_home = TempDir::new().unwrap();
    let registry = build_model_registry(&providers, &user_catalog, coco_home.path()).unwrap();
    let resolved = registry
        .resolve("deepseek", "deepseek-r1")
        .expect("lazy synth");
    assert_eq!(resolved.info.context_window.get(), 64_000);
    assert_eq!(resolved.info.temperature, Some(0.3));
}

#[test]
fn lazy_resolve_from_builtin_when_neither_user_catalog_nor_cfg_entry() {
    // Even with no user_catalog and no cfg.models entry, builtin
    // model metadata (e.g. `claude-sonnet-4-6`) is reachable via
    // lazy synth.
    let mut providers = BTreeMap::new();
    let partial = crate::provider::PartialProviderConfig {
        api: Some(ProviderApi::Anthropic),
        env_key: Some("ANTHROPIC_API_KEY".into()),
        base_url: Some("https://api.anthropic.com/v1".into()),
        ..Default::default()
    };
    providers.insert(
        "anthropic".into(),
        ProviderConfig::from_partial("anthropic", &partial).unwrap(),
    );
    let coco_home = TempDir::new().unwrap();
    let registry = build_model_registry(&providers, &empty_catalog(), coco_home.path()).unwrap();
    let resolved = registry
        .resolve("anthropic", "claude-sonnet-4-6")
        .expect("builtin lazy synth");
    assert_eq!(resolved.info.context_window.get(), 1_000_000);
    assert!(
        resolved
            .info
            .has_capability(coco_types::Capability::ExtendedThinking)
    );
}

#[test]
fn user_base_instructions_override_builtin_for_lazy_resolution() {
    let mut user_catalog = BTreeMap::new();
    user_catalog.insert(
        "gpt-5-4".into(),
        PartialModelInfo {
            base_instructions: Some("User override instructions".into()),
            ..Default::default()
        },
    );
    let mut providers = BTreeMap::new();
    let partial = crate::provider::PartialProviderConfig {
        api: Some(ProviderApi::Openai),
        env_key: Some("OPENAI_API_KEY".into()),
        base_url: Some("https://api.openai.com/v1".into()),
        ..Default::default()
    };
    providers.insert(
        "openai".into(),
        ProviderConfig::from_partial("openai", &partial).unwrap(),
    );
    let coco_home = TempDir::new().unwrap();
    let registry = build_model_registry(&providers, &user_catalog, coco_home.path()).unwrap();
    let resolved = registry.resolve("openai", "gpt-5-4").unwrap();
    assert_eq!(
        resolved.info.base_instructions.as_deref(),
        Some("User override instructions")
    );
}

#[test]
fn user_base_instructions_override_builtin_for_provider_entry() {
    let mut user_catalog = BTreeMap::new();
    user_catalog.insert(
        "gpt-5-5".into(),
        PartialModelInfo {
            base_instructions: Some("Provider-backed user override".into()),
            ..Default::default()
        },
    );
    let mut providers = BTreeMap::new();
    providers.insert(
        "openai".into(),
        provider_with_model(
            "openai",
            ProviderApi::Openai,
            "gpt-5-5",
            PartialProviderModelOverride::default(),
        ),
    );
    let coco_home = TempDir::new().unwrap();
    let registry = build_model_registry(&providers, &user_catalog, coco_home.path()).unwrap();
    let resolved = registry.resolve("openai", "gpt-5-5").unwrap();
    assert_eq!(
        resolved.info.base_instructions.as_deref(),
        Some("Provider-backed user override")
    );
}

#[test]
fn provider_backed_base_instructions_file_resolves() {
    let coco_home = TempDir::new().unwrap();
    fs::write(
        coco_home.path().join("provider-instructions.md"),
        "Provider file instructions",
    )
    .unwrap();
    let mut providers = BTreeMap::new();
    providers.insert(
        "openai".into(),
        provider_with_model(
            "openai",
            ProviderApi::Openai,
            "custom-provider-model",
            PartialProviderModelOverride {
                overrides: PartialModelInfo {
                    context_window: Some(PositiveTokens::new(64_000)),
                    max_output_tokens: Some(PositiveTokens::new(8_192)),
                    base_instructions_file: Some("provider-instructions.md".into()),
                    ..Default::default()
                },
                ..Default::default()
            },
        ),
    );
    let registry = build_model_registry(&providers, &empty_catalog(), coco_home.path()).unwrap();
    let resolved = registry.resolve("openai", "custom-provider-model").unwrap();
    assert_eq!(
        resolved.info.base_instructions.as_deref(),
        Some("Provider file instructions")
    );
    assert!(resolved.info.base_instructions_file.is_none());
}

#[test]
fn lazy_user_catalog_base_instructions_file_resolves() {
    let coco_home = TempDir::new().unwrap();
    fs::write(
        coco_home.path().join("lazy-instructions.md"),
        "Lazy file instructions",
    )
    .unwrap();
    let mut user_catalog = BTreeMap::new();
    user_catalog.insert(
        "lazy-model".into(),
        PartialModelInfo {
            context_window: Some(PositiveTokens::new(64_000)),
            max_output_tokens: Some(PositiveTokens::new(8_192)),
            base_instructions_file: Some("lazy-instructions.md".into()),
            ..Default::default()
        },
    );
    let mut providers = BTreeMap::new();
    let partial = crate::provider::PartialProviderConfig {
        api: Some(ProviderApi::OpenaiCompat),
        env_key: Some("LAZY_KEY".into()),
        base_url: Some("https://lazy.example".into()),
        ..Default::default()
    };
    providers.insert(
        "lazy".into(),
        ProviderConfig::from_partial("lazy", &partial).unwrap(),
    );
    let registry = build_model_registry(&providers, &user_catalog, coco_home.path()).unwrap();
    let resolved = registry.resolve("lazy", "lazy-model").unwrap();
    assert_eq!(
        resolved.info.base_instructions.as_deref(),
        Some("Lazy file instructions")
    );
    assert!(resolved.info.base_instructions_file.is_none());
    assert!(
        registry
            .user_catalog
            .get("lazy-model")
            .unwrap()
            .base_instructions_file
            .is_none(),
        "lazy catalog should store normalized inline instructions"
    );
}

#[test]
fn lazy_resolve_returns_none_when_unknown_model() {
    let mut providers = BTreeMap::new();
    let partial = crate::provider::PartialProviderConfig {
        api: Some(ProviderApi::Anthropic),
        env_key: Some("ANTHROPIC_API_KEY".into()),
        base_url: Some("https://api.anthropic.com/v1".into()),
        ..Default::default()
    };
    providers.insert(
        "anthropic".into(),
        ProviderConfig::from_partial("anthropic", &partial).unwrap(),
    );
    let coco_home = TempDir::new().unwrap();
    let registry = build_model_registry(&providers, &empty_catalog(), coco_home.path()).unwrap();
    assert!(
        registry
            .resolve("anthropic", "nonexistent-model-xyz")
            .is_none()
    );
}

#[test]
fn user_catalog_layers_under_entry() {
    let mut user_catalog = BTreeMap::new();
    user_catalog.insert(
        "gpt-5".into(),
        PartialModelInfo {
            context_window: Some(PositiveTokens::new(272_000)),
            max_output_tokens: Some(PositiveTokens::new(16_384)),
            temperature: Some(0.5),
            ..Default::default()
        },
    );
    let mut providers = BTreeMap::new();
    providers.insert(
        "openai".into(),
        provider_with_model(
            "openai",
            ProviderApi::Openai,
            "gpt-5",
            PartialProviderModelOverride {
                overrides: PartialModelInfo {
                    temperature: Some(0.7),
                    ..Default::default()
                },
                ..Default::default()
            },
        ),
    );
    let coco_home = TempDir::new().unwrap();
    let registry = build_model_registry(&providers, &user_catalog, coco_home.path()).unwrap();
    let resolved = registry.resolve("openai", "gpt-5").unwrap();
    // Per-entry overrides win over user catalog.
    assert_eq!(resolved.info.temperature, Some(0.7));
}
