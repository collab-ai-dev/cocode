use std::collections::BTreeMap;
use std::collections::HashMap;

use coco_types::ModelRole;
use coco_types::ModelSpec;
use coco_types::ProviderApi;
use coco_types::ProviderModelSelection;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::*;
use crate::EnvKey;
use crate::EnvSnapshot;
use crate::PartialProviderConfig;
use crate::RuntimeOverrides;
use crate::Settings;
use crate::settings::SettingsWithSource;

fn settings_with(settings: Settings) -> SettingsWithSource {
    SettingsWithSource {
        merged: settings,
        per_source: HashMap::new(),
        source_paths: HashMap::new(),
    }
}

/// Build a `RuntimeConfig` with isolated catalog paths so tests don't
/// pick up a stray `~/.coco/providers.json` (or `settings.json`,
/// `models.json`, managed-settings) on the developer's host. The
/// `TempDir` is dropped after `build_runtime_config_with` returns —
/// the resolved `RuntimeConfig` doesn't retain any path references,
/// so the tempdir's lifetime can end before the test asserts on the
/// returned snapshot.
fn build_isolated(
    settings: SettingsWithSource,
    env: EnvSnapshot,
    overrides: RuntimeOverrides,
) -> anyhow::Result<RuntimeConfig> {
    let tmp = TempDir::new().expect("tempdir");
    let catalogs = CatalogPaths::empty_in(tmp.path());
    let runtime = build_runtime_config_with(
        settings,
        env,
        overrides,
        catalogs,
        crate::parse_enabled_setting_sources(None),
    );
    drop(tmp);
    Ok(runtime?)
}

fn model_spec(provider: &str, api: ProviderApi, model_id: &str) -> ModelSpec {
    ModelSpec {
        provider: provider.to_string(),
        api,
        model_id: model_id.to_string(),
        display_name: model_id.to_string(),
    }
}

fn role_slots_of(provider: &str, model_id: &str) -> crate::RoleSlots<ProviderModelSelection> {
    crate::RoleSlots::new(model_selection(provider, model_id))
}

fn settings_with_main(provider: &str, model_id: &str) -> SettingsWithSource {
    settings_with(Settings {
        models: crate::ModelSelectionSettings {
            main: Some(role_slots_of(provider, model_id)),
            ..Default::default()
        },
        ..Default::default()
    })
}

fn model_selection(provider: &str, model_id: &str) -> ProviderModelSelection {
    ProviderModelSelection {
        provider: provider.to_string(),
        model_id: model_id.to_string(),
    }
}

#[test]
fn test_runtime_config_resolves_provider_overrides() {
    use crate::PartialModelInfo;
    use crate::positive::PositiveTokens;
    use crate::provider::PartialProviderModelOverride;

    let mut model_entries = BTreeMap::new();
    model_entries.insert(
        "local-model".to_string(),
        PartialProviderModelOverride {
            api_model_name: None,
            overrides: PartialModelInfo {
                context_window: Some(PositiveTokens::new(128_000)),
                max_output_tokens: Some(PositiveTokens::new(4_096)),
                ..Default::default()
            },
        },
    );
    let mut providers = BTreeMap::new();
    providers.insert(
        "local".to_string(),
        PartialProviderConfig {
            api: Some(ProviderApi::OpenaiCompat),
            env_key: Some("LOCAL_API_KEY".to_string()),
            base_url: Some("http://localhost:8080/v1".to_string()),
            models: Some(model_entries),
            ..Default::default()
        },
    );
    let settings = settings_with(Settings {
        models: crate::ModelSelectionSettings {
            main: Some(role_slots_of("local", "local-model")),
            ..Default::default()
        },
        providers,
        ..Default::default()
    });

    let runtime = build_isolated(
        settings,
        EnvSnapshot::default(),
        RuntimeOverrides::default(),
    )
    .expect("runtime config");

    assert_eq!(runtime.providers["local"].name, "local");
    let main = runtime
        .model_roles
        .get(ModelRole::Main)
        .expect("main model role");
    assert_eq!(main.provider, "local");
    assert_eq!(main.api, ProviderApi::OpenaiCompat);
    assert_eq!(main.model_id, "local-model");
}

#[test]
fn test_runtime_providers_satisfy_identity_invariant() {
    // Plan §15 Group B claim #2 (release-build invariant). Need an
    // explicit Main model — there is no implicit default after the
    // multi-LLM fallback removal.
    let runtime = build_isolated(
        settings_with_main("anthropic", "claude-opus-4-7"),
        EnvSnapshot::default(),
        RuntimeOverrides::default(),
    )
    .expect("runtime config");
    for (key, cfg) in &runtime.providers {
        assert_eq!(key, &cfg.name, "providers map key must equal cfg.name");
    }
}

#[test]
fn test_partial_provider_overlay_does_not_coerce_api() {
    // settings.json supplies only `client_options` for the builtin
    // `openai` provider. The `api` field MUST remain `Openai` from the
    // builtin layer — it is NOT silently coerced by serde default.
    let mut providers = BTreeMap::new();
    providers.insert(
        "openai".to_string(),
        PartialProviderConfig {
            client_options: Some(crate::PartialProviderClientOptions {
                organization_id: Some("org-myown".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    let settings = settings_with(Settings {
        // Main is mandatory now — the test asserts provider overlay
        // behavior, so any valid model selection works.
        models: crate::ModelSelectionSettings {
            main: Some(role_slots_of("openai", "gpt-5-5")),
            ..Default::default()
        },
        providers,
        ..Default::default()
    });
    let runtime = build_isolated(
        settings,
        EnvSnapshot::default(),
        RuntimeOverrides::default(),
    )
    .expect("runtime config");
    let openai = runtime.providers.get("openai").expect("openai entry");
    assert_eq!(openai.api, ProviderApi::Openai);
    assert_eq!(
        openai.client_options.organization_id.as_deref(),
        Some("org-myown")
    );
}

#[test]
fn test_incomplete_new_provider_overlay_returns_typed_error() {
    let mut providers = BTreeMap::new();
    providers.insert(
        "brand-new".to_string(),
        PartialProviderConfig {
            // Missing api / env_key / base_url — the partial declares a
            // new provider without the required identity fields.
            client_options: Some(crate::PartialProviderClientOptions::default()),
            ..Default::default()
        },
    );
    let settings = settings_with(Settings {
        providers,
        ..Default::default()
    });
    let err = build_isolated(
        settings,
        EnvSnapshot::default(),
        RuntimeOverrides::default(),
    )
    .expect_err("incomplete partial must fail");
    assert!(
        err.to_string().contains("brand-new"),
        "expected error to name the provider, got: {err}"
    );
}

#[test]
fn test_runtime_config_env_model_override_beats_json() {
    let settings = settings_with_main("anthropic", "claude-opus-4-7");
    let env = EnvSnapshot::from_pairs([(EnvKey::CocoModelMain, "openai/gpt-5-5")]);

    let runtime =
        build_isolated(settings, env, RuntimeOverrides::default()).expect("runtime config");

    let main = runtime
        .model_roles
        .get(ModelRole::Main)
        .expect("main model role");
    assert_eq!(main.provider, "openai");
    assert_eq!(main.api, ProviderApi::Openai);
    assert_eq!(main.model_id, "gpt-5-5");
}

#[test]
fn test_runtime_config_resolves_structured_model_roles() {
    let settings = settings_with(Settings {
        models: crate::ModelSelectionSettings {
            main: Some(role_slots_of("openai", "gpt-5-5")),
            fast: Some(role_slots_of("google", "gemini-3.1-pro-preview")),
            ..Default::default()
        },
        ..Default::default()
    });

    let runtime = build_isolated(
        settings,
        EnvSnapshot::default(),
        RuntimeOverrides::default(),
    )
    .expect("runtime config");

    let main = runtime
        .model_roles
        .get(ModelRole::Main)
        .expect("main model role");
    let fast = runtime
        .model_roles
        .get(ModelRole::Fast)
        .expect("fast model role");
    assert_eq!(main, &model_spec("openai", ProviderApi::Openai, "gpt-5-5"));
    assert_eq!(
        fast,
        &model_spec("google", ProviderApi::Gemini, "gemini-3.1-pro-preview")
    );
}

#[test]
fn test_runtime_config_rejects_bare_env_model_override() {
    let settings = settings_with(Settings::default());
    let env = EnvSnapshot::from_pairs([(EnvKey::CocoModelMain, "gpt-5-5")]);

    let err = build_isolated(settings, env, RuntimeOverrides::default())
        .expect_err("bare model override should fail");

    assert!(
        err.to_string()
            .contains("must use explicit `provider/model_id` format")
    );
}

#[test]
fn test_cli_fallback_model_overrides_populate_main_chain_in_order() {
    // `--fallback-model X --fallback-model Y` produces an ordered
    // Main role fallback chain. Flag order = chain priority. Main
    // must be set explicitly (no implicit default in the multi-LLM
    // SDK); pick an id distinct from the fallbacks so the chain has
    // no duplicates with the primary.
    let settings = settings_with_main("anthropic", "claude-haiku-4-5");
    let env = EnvSnapshot::default();
    let overrides = RuntimeOverrides {
        fallback_model_overrides: vec![
            ProviderModelSelection {
                provider: "anthropic".into(),
                model_id: "claude-opus-4-7".into(),
            },
            ProviderModelSelection {
                provider: "openai".into(),
                model_id: "gpt-5-5".into(),
            },
        ],
        ..Default::default()
    };
    let runtime = build_isolated(settings, env, overrides).expect("runtime with fallback chain");
    let fallbacks = runtime.model_roles.fallbacks(ModelRole::Main);
    assert_eq!(fallbacks.len(), 2);
    assert_eq!(fallbacks[0].provider, "anthropic");
    assert_eq!(fallbacks[0].model_id, "claude-opus-4-7");
    assert_eq!(fallbacks[1].provider, "openai");
    assert_eq!(fallbacks[1].model_id, "gpt-5-5");
}

#[test]
fn test_cli_fallback_model_rejects_duplicate_of_primary() {
    // Configuring primary + fallback to the same slug makes the
    // fallback useless; hard-fail at startup rather than silently
    // accept a degenerate config.
    let settings = settings_with_main("anthropic", "claude-opus-4-7");
    let env = EnvSnapshot::default();
    let overrides = RuntimeOverrides {
        fallback_model_overrides: vec![ProviderModelSelection {
            provider: "anthropic".into(),
            model_id: "claude-opus-4-7".into(),
        }],
        ..Default::default()
    };
    let err =
        build_isolated(settings, env, overrides).expect_err("duplicate primary+fallback must fail");
    assert!(
        err.to_string().contains("duplicates primary"),
        "expected duplicate-primary error, got: {err}"
    );
}

#[test]
fn test_cli_fallback_model_rejects_duplicate_within_chain() {
    // Use an explicit primary so the auto-default doesn't collide
    // with one of the duplicated fallbacks.
    let settings = settings_with_main("openai", "gpt-5-5");
    let env = EnvSnapshot::default();
    let overrides = RuntimeOverrides {
        fallback_model_overrides: vec![
            ProviderModelSelection {
                provider: "anthropic".into(),
                model_id: "claude-sonnet-4-6".into(),
            },
            ProviderModelSelection {
                provider: "anthropic".into(),
                model_id: "claude-sonnet-4-6".into(),
            },
        ],
        ..Default::default()
    };
    let err = build_isolated(settings, env, overrides)
        .expect_err("duplicate fallback within chain must fail");
    assert!(
        err.to_string().contains("duplicates earlier fallback"),
        "expected duplicate-earlier-fallback error, got: {err}"
    );
}

#[test]
fn test_cli_fallback_model_with_unknown_provider_fails_startup() {
    // Main is configured with a valid spec so resolution reaches the
    // fallback-chain validation step (which is what this test is
    // actually exercising).
    let settings = settings_with_main("anthropic", "claude-opus-4-7");
    let env = EnvSnapshot::default();
    let overrides = RuntimeOverrides {
        fallback_model_overrides: vec![ProviderModelSelection {
            provider: "nonexistent".into(),
            model_id: "some-model".into(),
        }],
        ..Default::default()
    };
    let err = build_isolated(settings, env, overrides)
        .expect_err("unknown provider must fail fast at startup");
    let msg = err.to_string();
    assert!(
        msg.contains("unknown provider") || msg.contains("nonexistent"),
        "expected unknown-provider error, got: {msg}",
    );
}

#[test]
fn settings_inline_provider_block_equals_providers_json_split() {
    // Plan §15 invariant: a `settings.json` containing the full
    // `providers` block produces the same `RuntimeConfig` as the same
    // data hoisted into sibling `providers.json` + minimal
    // `settings.json`. Asserts no surprise ordering, key-set, or
    // resolution divergence between the two on-disk shapes.
    use std::fs;

    let mut providers_inline = BTreeMap::new();
    providers_inline.insert(
        "azure-east".to_string(),
        PartialProviderConfig {
            api: Some(ProviderApi::Openai),
            env_key: Some("AZURE_KEY".into()),
            base_url: Some("https://azure.example/v1".into()),
            ..Default::default()
        },
    );

    // Both shapes need an explicit Main model — the multi-LLM SDK
    // intentionally has no implicit default. Pick a builtin
    // (anthropic) so neither shape needs the user-defined provider
    // to resolve Main.
    // Path A: providers declared inline in settings.json.
    let inline_runtime = build_isolated(
        settings_with(Settings {
            models: crate::ModelSelectionSettings {
                main: Some(role_slots_of("anthropic", "claude-opus-4-7")),
                ..Default::default()
            },
            providers: providers_inline.clone(),
            ..Default::default()
        }),
        EnvSnapshot::default(),
        RuntimeOverrides::default(),
    )
    .expect("inline runtime");

    // Path B: providers in sibling providers.json, settings minimal.
    let tmp = TempDir::new().unwrap();
    let providers_json_path = tmp.path().join("providers.json");
    fs::write(
        &providers_json_path,
        serde_json::to_string_pretty(&providers_inline).unwrap(),
    )
    .unwrap();
    let split_runtime = build_runtime_config_with(
        settings_with_main("anthropic", "claude-opus-4-7"),
        EnvSnapshot::default(),
        RuntimeOverrides::default(),
        CatalogPaths::rooted(tmp.path()),
        crate::parse_enabled_setting_sources(None),
    )
    .expect("split runtime");

    let inline_keys: std::collections::BTreeSet<_> = inline_runtime.providers.keys().collect();
    let split_keys: std::collections::BTreeSet<_> = split_runtime.providers.keys().collect();
    assert_eq!(
        inline_keys, split_keys,
        "provider key set must match between inline and split shapes"
    );
    let inline_az = inline_runtime.providers.get("azure-east").unwrap();
    let split_az = split_runtime.providers.get("azure-east").unwrap();
    assert_eq!(inline_az.api, split_az.api);
    assert_eq!(inline_az.base_url, split_az.base_url);
    assert_eq!(inline_az.env_key, split_az.env_key);
    assert_eq!(inline_az.name, split_az.name);
}

#[test]
fn test_role_validation_rejects_unknown_model_id() {
    // Plan §11: typo'd model_id in `settings.models.main` must
    // surface at config build, not silently degrade runtime construction
    // to the legacy mock path.
    let settings = settings_with_main("openai", "gpt-typo");
    let err = build_isolated(
        settings,
        EnvSnapshot::default(),
        RuntimeOverrides::default(),
    )
    .expect_err("unknown model id must fail at config build");
    assert!(
        err.to_string().contains("unknown model"),
        "expected UnknownModel error, got: {err}"
    );
}

#[test]
fn test_runtime_loads_jsonc_provider_catalog() {
    use std::fs;

    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("providers.json"),
        r#"{
            // Provider catalog files accept JSONC syntax.
            local: {
                api: "openai_compat",
                "env_key": "LOCAL_KEY",
                "base_url": "https://local.invalid/v1",
            },
        }"#,
    )
    .unwrap();

    let runtime = build_runtime_config_with(
        settings_with_main("anthropic", "claude-opus-4-7"),
        EnvSnapshot::default(),
        RuntimeOverrides::default(),
        CatalogPaths::rooted(tmp.path()),
        crate::parse_enabled_setting_sources(None),
    )
    .expect("runtime config");

    let provider = runtime.providers.get("local").expect("local provider");
    assert_eq!(provider.env_key, "LOCAL_KEY");
    assert_eq!(provider.base_url, "https://local.invalid/v1");
}

#[test]
fn test_role_validation_rejects_incomplete_user_catalog_entry() {
    // A `models.json` entry that sets only `max_output_tokens` (no
    // `context_window`) is incomplete. The validation pass surfaces
    // it as `IncompleteModelEntry` at startup instead of letting it
    // silently disappear under `Option<ModelInfo> = None` later.
    use crate::positive::PositiveTokens;
    use std::fs;

    let tmp = TempDir::new().unwrap();
    let models_json = tmp.path().join("models.json");
    let entries: BTreeMap<String, crate::PartialModelInfo> = [(
        "custom-llm".to_string(),
        crate::PartialModelInfo {
            max_output_tokens: Some(PositiveTokens::new(2048)),
            // intentionally no context_window
            ..Default::default()
        },
    )]
    .into_iter()
    .collect();
    fs::write(
        &models_json,
        serde_json::to_string_pretty(&entries).unwrap(),
    )
    .unwrap();

    let settings = settings_with_main("openai", "custom-llm");
    let err = build_runtime_config_with(
        settings,
        EnvSnapshot::default(),
        RuntimeOverrides::default(),
        CatalogPaths::rooted(tmp.path()),
        crate::parse_enabled_setting_sources(None),
    )
    .expect_err("incomplete user_catalog entry must fail at config build");
    let msg = err.to_string();
    assert!(
        msg.contains("context_window") || msg.contains("missing required field"),
        "expected IncompleteModelEntry on context_window, got: {msg}"
    );
}

#[test]
fn test_cli_fallback_overrides_take_precedence_over_settings_fallbacks() {
    // When settings.models.main has fallbacks AND --fallback-model
    // is supplied, CLI wins. Ensures users can override a config
    // they can't edit (project/policy level) from the command line.
    use crate::model::RoleSlots;
    use coco_types::ProviderModelSelection;
    let settings = settings_with(Settings {
        models: crate::ModelSelectionSettings {
            main: Some(
                RoleSlots::new(ProviderModelSelection {
                    provider: "anthropic".into(),
                    model_id: "claude-opus-4-7".into(),
                })
                .with_fallback(ProviderModelSelection {
                    provider: "anthropic".into(),
                    model_id: "claude-sonnet-4-6".into(),
                }),
            ),
            ..Default::default()
        },
        ..Default::default()
    });
    let overrides = RuntimeOverrides {
        fallback_model_overrides: vec![ProviderModelSelection {
            provider: "openai".into(),
            model_id: "gpt-5-5".into(),
        }],
        ..Default::default()
    };
    let runtime = build_isolated(settings, EnvSnapshot::default(), overrides)
        .expect("CLI should override settings fallbacks");
    let fallbacks = runtime.model_roles.fallbacks(ModelRole::Main);
    assert_eq!(fallbacks.len(), 1, "CLI replaces settings chain entirely");
    assert_eq!(fallbacks[0].provider, "openai");
    assert_eq!(fallbacks[0].model_id, "gpt-5-5");
}

#[test]
fn test_unconfigured_roles_default_to_main() {
    // No settings.models.{fast,memory,compact,plan,explore,review,hook_agent}
    // → resolver should fall back to Main so consumer-side
    // `.get(role)` is total.
    let settings = settings_with_main("anthropic", "claude-opus-4-7");
    let runtime = build_isolated(
        settings,
        EnvSnapshot::default(),
        RuntimeOverrides::default(),
    )
    .expect("resolve");

    let main_spec = runtime
        .model_roles
        .get(ModelRole::Main)
        .expect("Main always set");

    for role in [
        ModelRole::Fast,
        ModelRole::Plan,
        ModelRole::Explore,
        ModelRole::Review,
        ModelRole::HookAgent,
        ModelRole::Memory,
        ModelRole::Subagent,
    ] {
        let got = runtime
            .model_roles
            .get(role)
            .unwrap_or_else(|| panic!("{role:?} should default to Main"));
        assert_eq!(
            got.model_id, main_spec.model_id,
            "{role:?} did not inherit Main"
        );
        assert_eq!(got.provider, main_spec.provider);
    }
}

#[test]
fn test_explicit_role_overrides_main_default() {
    // settings.models.memory present → keep that, do NOT overwrite
    // with Main.
    use crate::model::RoleSlots;
    use coco_types::ProviderModelSelection;
    let settings = settings_with(Settings {
        models: crate::ModelSelectionSettings {
            main: Some(role_slots_of("anthropic", "claude-opus-4-7")),
            memory: Some(RoleSlots::new(ProviderModelSelection {
                provider: "anthropic".into(),
                model_id: "claude-haiku-4-5".into(),
            })),
            ..Default::default()
        },
        ..Default::default()
    });
    let runtime = build_isolated(
        settings,
        EnvSnapshot::default(),
        RuntimeOverrides::default(),
    )
    .expect("resolve");
    let memory = runtime
        .model_roles
        .get(ModelRole::Memory)
        .expect("Memory configured");
    assert_eq!(memory.model_id, "claude-haiku-4-5");
}

#[test]
fn test_subagent_role_resolves_from_settings() {
    // settings.models.subagent → ModelRole::Subagent. Env-only
    // overrides for this role have been removed; settings.json is
    // the single source.
    use crate::model::RoleSlots;
    use coco_types::ProviderModelSelection;
    let settings = settings_with(Settings {
        models: crate::ModelSelectionSettings {
            main: Some(role_slots_of("anthropic", "claude-opus-4-7")),
            subagent: Some(RoleSlots::new(ProviderModelSelection {
                provider: "anthropic".into(),
                model_id: "claude-haiku-4-5".into(),
            })),
            ..Default::default()
        },
        ..Default::default()
    });
    let runtime = build_isolated(
        settings,
        EnvSnapshot::default(),
        RuntimeOverrides::default(),
    )
    .expect("resolve");
    let subagent = runtime
        .model_roles
        .get(ModelRole::Subagent)
        .expect("Subagent configured");
    assert_eq!(subagent.model_id, "claude-haiku-4-5");
}

#[test]
fn test_subagent_role_defaults_to_main_when_unset() {
    let settings = settings_with_main("anthropic", "claude-opus-4-7");
    let runtime = build_isolated(
        settings,
        EnvSnapshot::default(),
        RuntimeOverrides::default(),
    )
    .expect("resolve");
    let main = runtime.model_roles.get(ModelRole::Main).expect("Main");
    let subagent = runtime
        .model_roles
        .get(ModelRole::Subagent)
        .expect("Subagent defaulted from Main");
    assert_eq!(subagent.model_id, main.model_id);
    assert_eq!(subagent.provider, main.provider);
}

#[test]
fn test_parse_enabled_setting_sources() {
    use crate::SettingSource;

    // None ⇒ all five sources.
    let all = parse_enabled_setting_sources(None);
    assert!(all.contains(&SettingSource::User));
    assert!(all.contains(&SettingSource::Project));
    assert!(all.contains(&SettingSource::Local));
    assert!(all.contains(&SettingSource::Flag));
    assert!(all.contains(&SettingSource::Policy));
    assert_eq!(all.len(), 5);

    // "user,project" ⇒ those two + always-on Policy + Flag.
    let some = parse_enabled_setting_sources(Some("user,project"));
    assert!(some.contains(&SettingSource::User));
    assert!(some.contains(&SettingSource::Project));
    assert!(!some.contains(&SettingSource::Local));
    assert!(some.contains(&SettingSource::Flag));
    assert!(some.contains(&SettingSource::Policy));
    assert_eq!(some.len(), 4);

    // "" (empty) ⇒ only the always-on Policy + Flag.
    let empty = parse_enabled_setting_sources(Some(""));
    assert!(!empty.contains(&SettingSource::User));
    assert!(!empty.contains(&SettingSource::Project));
    assert!(!empty.contains(&SettingSource::Local));
    assert!(empty.contains(&SettingSource::Flag));
    assert!(empty.contains(&SettingSource::Policy));
    assert_eq!(empty.len(), 2);

    // Whitespace + unknown tokens are tolerated.
    let trimmed = parse_enabled_setting_sources(Some(" local , bogus "));
    assert!(trimmed.contains(&SettingSource::Local));
    assert!(trimmed.contains(&SettingSource::Flag));
    assert!(trimmed.contains(&SettingSource::Policy));
    assert_eq!(trimmed.len(), 3);
}
