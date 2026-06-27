//! Unit tests for the RuntimeConfig + ModelSpec → LanguageModel dispatch factory.
//!
//! These don't make real network calls — they just verify the dispatch
//! routes each `ProviderApi` variant to the right provider crate and
//! returns an `Arc<dyn LanguageModel>` without panicking.

use super::*;
use coco_config::EnvSnapshot;
use coco_config::PartialProviderConfig;
use coco_config::RoleSlots;
use coco_config::RuntimeOverrides;
use coco_config::Settings;
use coco_config::settings::SettingsWithSource;
use coco_types::ProviderApi;
use coco_types::ProviderModelSelection;
use std::collections::BTreeMap;
use std::collections::HashMap;
use tempfile::TempDir;

fn settings_with(settings: Settings) -> SettingsWithSource {
    SettingsWithSource {
        merged: settings,
        per_source: HashMap::new(),
        source_paths: HashMap::new(),
    }
}

fn build_runtime_with(extra_provider: Option<(String, PartialProviderConfig)>) -> RuntimeConfig {
    let mut providers = BTreeMap::new();
    if let Some((name, partial)) = extra_provider {
        providers.insert(name, partial);
    }
    let settings = Settings {
        // Multi-LLM SDK: Main has no implicit default. These tests
        // exercise model_factory for arbitrary (provider, model) pairs
        // — Main itself is irrelevant, just needs to satisfy the
        // mandatory-field check.
        models: coco_config::ModelSelectionSettings {
            main: Some(RoleSlots::new(ProviderModelSelection {
                provider: "anthropic".into(),
                model_id: "claude-opus-4-7".into(),
            })),
            ..Default::default()
        },
        providers,
        ..Default::default()
    };
    let tmp = TempDir::new().expect("tempdir");
    let catalogs = coco_config::CatalogPaths::empty_in(tmp.path());
    coco_config::build_runtime_config_with(
        settings_with(settings),
        EnvSnapshot::default(),
        RuntimeOverrides::default(),
        catalogs,
        coco_config::parse_enabled_setting_sources(None),
    )
    .expect("runtime")
}

fn spec(provider: &str, api: ProviderApi, model_id: &str) -> ModelSpec {
    ModelSpec {
        provider: provider.into(),
        api,
        model_id: model_id.into(),
        display_name: model_id.into(),
    }
}

#[test]
fn build_anthropic_succeeds_via_runtime() {
    let runtime = build_runtime_with(None);
    let s = spec("anthropic", ProviderApi::Anthropic, "claude-opus-4-7");
    let result = build_language_model_from_runtime(&runtime, &s, None, None);
    assert!(
        result.is_ok(),
        "anthropic factory must succeed (err: {:?})",
        result.err().map(|e| e.to_string())
    );
}

#[test]
fn build_openai_succeeds_via_runtime() {
    let runtime = build_runtime_with(None);
    let s = spec("openai", ProviderApi::Openai, "gpt-4o-mini");
    let result = build_language_model_from_runtime(&runtime, &s, None, None);
    assert!(
        result.is_ok(),
        "openai factory must succeed (err: {:?})",
        result.err().map(|e| e.to_string())
    );
}

#[test]
fn build_gemini_dispatches_to_google_provider() {
    let runtime = build_runtime_with(None);
    let s = spec("google", ProviderApi::Gemini, "gemini-2.5-flash");
    let result = build_language_model_from_runtime(&runtime, &s, None, None);
    match result {
        Ok(_) => {}
        Err(e) => {
            let s = e.to_string().to_lowercase();
            assert!(
                s.contains("google") || s.contains("api key"),
                "gemini dispatch must route to google provider; got: {s}"
            );
        }
    }
}

#[test]
fn build_volcengine_routes_to_openai_compat_via_runtime() {
    // Volcengine is a builtin provider with `api: Volcengine`. With G1 it
    // routes through openai-compatible and constructs successfully — no
    // longer rejected with an error.
    let runtime = build_runtime_with(None);
    let s = spec("volcengine", ProviderApi::Volcengine, "doubao-1.5");
    let result = build_language_model_from_runtime(&runtime, &s, None, None);
    assert!(
        result.is_ok(),
        "volcengine should route via openai-compat (err: {:?})",
        result.err().map(|e| e.to_string())
    );
}

#[test]
fn build_openai_compat_routes_for_user_defined_instance() {
    let mut models = BTreeMap::new();
    models.insert(
        "local-model".to_string(),
        coco_config::PartialProviderModelOverride {
            overrides: coco_config::PartialModelInfo {
                context_window: Some(coco_config::PositiveTokens::new(128_000)),
                max_output_tokens: Some(coco_config::PositiveTokens::new(8_192)),
                ..Default::default()
            },
            ..Default::default()
        },
    );
    let partial = PartialProviderConfig {
        api: Some(ProviderApi::OpenaiCompat),
        env_key: Some("LOCAL_KEY".into()),
        base_url: Some("http://localhost:8080/v1".into()),
        models: Some(models),
        ..Default::default()
    };
    let runtime = build_runtime_with(Some(("local-router".into(), partial)));
    let s = spec("local-router", ProviderApi::OpenaiCompat, "local-model");
    let result = build_language_model_from_runtime(&runtime, &s, None, None);
    assert!(
        result.is_ok(),
        "openai-compat instance should construct (err: {:?})",
        result.err().map(|e| e.to_string())
    );
    let model = result.unwrap();
    // openai-compat reports `<name>.<sub-provider>` from `model.provider()`
    // (e.g. `"local-router.chat"`), while the namespace key in
    // `provider_options` is the bare `<name>` — the SDK strips the
    // suffix before the lookup. Assert the prefix matches so callers
    // who rely on `model.provider()` still see the configured name.
    assert!(
        model.provider().starts_with("local-router"),
        "openai-compat must surface the runtime instance name via model.provider(); got: {}",
        model.provider()
    );
}

/// Build an openai-compat `local-router` instance carrying a single custom
/// header `X-Trace` with the given value.
fn compat_with_header(value: coco_config::HeaderValue) -> RuntimeConfig {
    let mut models = BTreeMap::new();
    models.insert(
        "local-model".to_string(),
        coco_config::PartialProviderModelOverride {
            overrides: coco_config::PartialModelInfo {
                context_window: Some(coco_config::PositiveTokens::new(128_000)),
                max_output_tokens: Some(coco_config::PositiveTokens::new(8_192)),
                ..Default::default()
            },
            ..Default::default()
        },
    );
    let partial = PartialProviderConfig {
        api: Some(ProviderApi::OpenaiCompat),
        env_key: Some("LOCAL_KEY".into()),
        base_url: Some("http://localhost:8080/v1".into()),
        models: Some(models),
        client_options: Some(coco_config::PartialProviderClientOptions {
            headers: Some(BTreeMap::from([("X-Trace".to_string(), value)])),
            ..Default::default()
        }),
        ..Default::default()
    };
    build_runtime_with(Some(("local-router".into(), partial)))
}

#[test]
fn templated_header_unknown_var_fails_build() {
    let runtime = compat_with_header(coco_config::HeaderValue::templated("${NOPE}"));
    let s = spec("local-router", ProviderApi::OpenaiCompat, "local-model");
    let err = build_language_model_from_runtime(&runtime, &s, None, None)
        .map(|_| ())
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("X-Trace") && err.contains("NOPE"),
        "error names the offending header + variable: {err}"
    );
}

#[test]
fn templated_header_known_var_builds_with_session_context() {
    let runtime = compat_with_header(coco_config::HeaderValue::templated(
        "${PROVIDER}/${SESSION_ID}",
    ));
    let s = spec("local-router", ProviderApi::OpenaiCompat, "local-model");
    let vars = crate::header_template::HeaderVars {
        session_id: "sess-1".into(),
        cwd: "/tmp".into(),
        app_version: "1.2.3".into(),
    };
    let result = build_language_model_from_runtime(&runtime, &s, None, Some(&vars));
    assert!(
        result.is_ok(),
        "templated header with known vars should build (err: {:?})",
        result.err().map(|e| e.to_string())
    );
}

#[test]
fn build_unknown_provider_fails_with_typed_error() {
    let runtime = build_runtime_with(None);
    let s = spec("unknown-provider", ProviderApi::OpenaiCompat, "x");
    let err = build_language_model_from_runtime(&runtime, &s, None, None)
        .map(|_| ())
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("unknown-provider"),
        "error names the provider: {err}"
    );
}

#[test]
fn build_api_client_uses_real_fingerprint_for_anthropic() {
    let runtime = build_runtime_with(None);
    let s = spec("anthropic", ProviderApi::Anthropic, "claude-sonnet-4-6");
    let client =
        build_api_client(&runtime, &s, RetryConfig::default(), None, None).expect("api client");
    let fp = client.fingerprint();
    assert_eq!(fp.api, ProviderApi::Anthropic);
    assert_eq!(fp.provider, "anthropic");
    assert_eq!(fp.api_model_name, "claude-sonnet-4-6");
    // R1 canary — builtin must include the `/v1` segment so the SDK's
    // path-append yields `https://api.anthropic.com/v1/messages` not
    // `/messages`. The Rust port has no auto-detect for missing version
    // segments; this assertion locks in the corrected URL.
    assert_eq!(fp.base_url, "https://api.anthropic.com/v1");
    assert!(
        fp.base_url.ends_with("/v1"),
        "Anthropic builtin base_url must end with /v1; got {}",
        fp.base_url
    );
}

#[test]
fn build_api_client_anthropic_url_composes_to_messages_endpoint() {
    // R4 canary — guard against a future regression that drops the
    // version segment. Asserts the composed URL via the same logic the
    // SDK uses (`base_url.ends_with("/messages") ? base_url :
    // format!("{base_url}/messages")`).
    let runtime = build_runtime_with(None);
    let s = spec("anthropic", ProviderApi::Anthropic, "claude-sonnet-4-6");
    let client =
        build_api_client(&runtime, &s, RetryConfig::default(), None, None).expect("api client");
    let base_url = &client.fingerprint().base_url;
    let composed = if base_url.ends_with("/messages") {
        base_url.clone()
    } else {
        format!("{base_url}/messages")
    };
    assert_eq!(composed, "https://api.anthropic.com/v1/messages");
}

#[test]
fn builtin_google_base_url_composes_to_models_endpoint() {
    // Google's SDK validates API keys at construction time, so we
    // can't go through `build_api_client` without `GOOGLE_GENERATIVE_AI_API_KEY`
    // in env. Read `base_url` directly from `runtime.providers` and
    // assert the composition matches what the SDK does internally
    // (`<base_url>/models/<id>:generateContent` in
    // `google_generative_ai_language_model.rs:843`).
    let runtime = build_runtime_with(None);
    let base_url = runtime
        .providers
        .get("google")
        .expect("google builtin")
        .base_url
        .clone();
    assert!(
        base_url.ends_with("/v1beta"),
        "Google builtin base_url must end with /v1beta; got {base_url}"
    );
    let composed = format!("{base_url}/models/gemini-2.5-pro:generateContent");
    assert_eq!(
        composed,
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-pro:generateContent"
    );
}

#[test]
fn namespace_round_trip_for_openai_compat_instance() {
    // Plan §15 invariant: the runtime `model.provider()` string equals
    // the outer key in `call.provider_options.0` after `build_call_options`.
    // For OpenAI-compat instances the namespace is the `ProviderConfig.name`,
    // which the SDK passes through to `model.provider()`.
    use crate::PerCallOverrides;
    use crate::build_call_options;
    let mut models = BTreeMap::new();
    models.insert(
        "local-model".to_string(),
        coco_config::PartialProviderModelOverride {
            overrides: coco_config::PartialModelInfo {
                context_window: Some(coco_config::PositiveTokens::new(128_000)),
                max_output_tokens: Some(coco_config::PositiveTokens::new(8_192)),
                ..Default::default()
            },
            ..Default::default()
        },
    );
    let partial = PartialProviderConfig {
        api: Some(ProviderApi::OpenaiCompat),
        env_key: Some("LOCAL_KEY".into()),
        base_url: Some("http://localhost:8080/v1".into()),
        models: Some(models),
        ..Default::default()
    };
    let runtime = build_runtime_with(Some(("internal-router".into(), partial)));
    let s = spec("internal-router", ProviderApi::OpenaiCompat, "local-model");
    let model = build_language_model_from_runtime(&runtime, &s, None, None).expect("model");

    let info_partial = coco_config::PartialModelInfo {
        context_window: Some(coco_config::PositiveTokens::new(128_000)),
        max_output_tokens: Some(coco_config::PositiveTokens::new(8_192)),
        extra_body: Some(
            [("myCustomField".to_string(), serde_json::json!("x"))]
                .into_iter()
                .collect(),
        ),
        ..Default::default()
    };
    let info = coco_config::ModelInfo::from_partial("internal-router", "local-model", info_partial)
        .expect("info");
    let call = build_call_options(
        &info,
        ProviderApi::OpenaiCompat,
        "internal-router",
        &PerCallOverrides::default(),
        Vec::new(),
        None,
    );
    let po = call.provider_options.expect("provider_options set");
    assert_eq!(po.0.len(), 1, "exactly one outer namespace key");
    assert!(po.0.contains_key("internal-router"));
    // OpenAI-compat reports `"<name>.<sub-provider>"` from
    // `model.provider()`. The SDK strips the suffix to obtain the
    // namespace key, so the round-trip works in practice — assert the
    // prefix matches the wrap key.
    assert!(
        model.provider().starts_with("internal-router"),
        "model.provider() prefix must match the wrap key; got: {}",
        model.provider()
    );
}

#[test]
fn build_api_client_resolves_api_model_name_override() {
    let mut models = BTreeMap::new();
    models.insert(
        "internal/coder-v3".to_string(),
        coco_config::PartialProviderModelOverride {
            api_model_name: Some("ep-internal-v3-prod".into()),
            overrides: coco_config::PartialModelInfo {
                context_window: Some(coco_config::PositiveTokens::new(128_000)),
                max_output_tokens: Some(coco_config::PositiveTokens::new(8_192)),
                ..Default::default()
            },
        },
    );
    let partial = PartialProviderConfig {
        api: Some(ProviderApi::OpenaiCompat),
        env_key: Some("INTERNAL_KEY".into()),
        base_url: Some("https://internal/v1".into()),
        models: Some(models),
        ..Default::default()
    };
    let runtime = build_runtime_with(Some(("internal-router".into(), partial)));
    let s = spec(
        "internal-router",
        ProviderApi::OpenaiCompat,
        "internal/coder-v3",
    );
    let client =
        build_api_client(&runtime, &s, RetryConfig::default(), None, None).expect("api client");
    assert_eq!(
        client.fingerprint().api_model_name,
        "ep-internal-v3-prod",
        "api_model_name override flows into the fingerprint",
    );
    assert_eq!(
        client.model_identity().model_id,
        "internal/coder-v3",
        "usage and session accounting keep the configured model identity",
    );
}

// ── Subscription-OAuth auth-mode mapping (the neutral→wire seam) ───────────

/// Test double for [`crate::credentials::ProviderCredentialResolver`]: reports a
/// fixed supplier (or none) regardless of the provider name.
struct FakeResolver {
    creds: Option<crate::credentials::SubscriptionCreds>,
}

impl crate::credentials::ProviderCredentialResolver for FakeResolver {
    fn subscription_creds(
        &self,
        _provider_name: &str,
    ) -> Option<crate::credentials::SubscriptionCredsSupplier> {
        let creds = self.creds.clone()?;
        Some(Arc::new(move || Some(creds.clone())))
    }
}

fn resolver_with(
    creds: Option<crate::credentials::SubscriptionCreds>,
) -> Arc<dyn crate::credentials::ProviderCredentialResolver> {
    Arc::new(FakeResolver { creds })
}

fn oauth_openai_chatgpt_provider() -> ProviderConfig {
    let partial = PartialProviderConfig {
        api: Some(ProviderApi::Openai),
        auth: Some(coco_config::ProviderAuth::OAuth {
            flow: coco_types::OAuthFlowId::OpenAiChatGpt,
        }),
        base_url: Some("https://chatgpt.com/backend-api/codex".into()),
        ..Default::default()
    };
    ProviderConfig::from_partial("openai-chatgpt", &partial).expect("oauth provider")
}

#[test]
fn build_openai_auth_oauth_logged_in_yields_subscription() {
    let cfg = oauth_openai_chatgpt_provider();
    let resolver = resolver_with(Some(crate::credentials::SubscriptionCreds {
        access_token: "tok".into(),
        account_id: Some("acct".into()),
        ..Default::default()
    }));
    let auth = build_openai_auth(&cfg, Some(&resolver)).expect("auth maps");
    assert!(
        auth.is_chatgpt_subscription(),
        "a logged-in OAuth provider must map to ChatGptSubscription"
    );
}

#[test]
fn build_openai_auth_oauth_not_logged_in_errors_with_login_hint() {
    let cfg = oauth_openai_chatgpt_provider();
    let resolver = resolver_with(None);
    // `OpenAIAuth` holds a closure and isn't `Debug`, so `expect_err` is out.
    let err = match build_openai_auth(&cfg, Some(&resolver)) {
        Ok(_) => panic!("must error when logged out"),
        Err(e) => e,
    };
    assert!(
        err.to_string().to_lowercase().contains("login"),
        "error must tell the user to log in; got: {err}"
    );
}

#[test]
fn build_openai_auth_apikey_yields_apikey() {
    let partial = PartialProviderConfig {
        api: Some(ProviderApi::Openai),
        env_key: Some("OPENAI_API_KEY".into()),
        base_url: Some("https://api.openai.com/v1".into()),
        ..Default::default()
    };
    let cfg = ProviderConfig::from_partial("openai", &partial).expect("apikey provider");
    let auth = build_openai_auth(&cfg, None).expect("auth maps");
    assert!(
        !auth.is_chatgpt_subscription(),
        "an api-key provider must map to ApiKey, never subscription"
    );
}

#[test]
fn build_openai_auth_rejects_gemini_flow_on_openai_provider() {
    let partial = PartialProviderConfig {
        api: Some(ProviderApi::Openai),
        auth: Some(coco_config::ProviderAuth::OAuth {
            flow: coco_types::OAuthFlowId::GeminiCodeAssist,
        }),
        base_url: Some("https://api.openai.com/v1".into()),
        ..Default::default()
    };
    let cfg = ProviderConfig::from_partial("misrouted", &partial).expect("provider");
    let resolver = resolver_with(Some(crate::credentials::SubscriptionCreds::default()));
    assert!(
        build_openai_auth(&cfg, Some(&resolver)).is_err(),
        "a Gemini flow on an OpenAI provider is a misconfiguration"
    );
}

fn gemini_code_assist_provider() -> ProviderConfig {
    let partial = PartialProviderConfig {
        api: Some(ProviderApi::Gemini),
        auth: Some(coco_config::ProviderAuth::OAuth {
            flow: coco_types::OAuthFlowId::GeminiCodeAssist,
        }),
        base_url: Some("https://cloudcode-pa.googleapis.com/v1internal".into()),
        ..Default::default()
    };
    ProviderConfig::from_partial("gemini-code-assist", &partial).expect("provider")
}

#[test]
fn build_google_oauth_not_logged_in_errors_with_login_hint() {
    let cfg = gemini_code_assist_provider();
    // No resolver / not logged in → actionable error, never a silent wrong-key build.
    let err = match build_google(&cfg, "gemini-3.1-pro-preview", None, None) {
        Ok(_) => panic!("OAuth Gemini without a logged-in resolver must error"),
        Err(e) => e,
    };
    assert!(
        err.to_string().to_lowercase().contains("login"),
        "error must tell the user to log in; got: {err}"
    );
}

#[test]
fn build_google_oauth_logged_in_builds_code_assist_model() {
    let cfg = gemini_code_assist_provider();
    let resolver = resolver_with(Some(crate::credentials::SubscriptionCreds {
        access_token: "tok".into(),
        project_id: Some("proj-1".into()),
        ..Default::default()
    }));
    let model = build_google(&cfg, "gemini-3.1-pro-preview", Some(&resolver), None)
        .expect("a logged-in Code Assist provider builds");
    // The provider label is the configured instance name (set from the config,
    // like every other provider), not the crate default.
    assert_eq!(model.provider(), "gemini-code-assist");
    assert_eq!(model.model_id(), "gemini-3.1-pro-preview");
}

#[test]
fn provider_credential_present_oauth_tracks_login_state() {
    let cfg = oauth_openai_chatgpt_provider();
    let logged_in = resolver_with(Some(crate::credentials::SubscriptionCreds {
        access_token: "t".into(),
        ..Default::default()
    }));
    let logged_out = resolver_with(None);
    assert!(provider_credential_present(&cfg, Some(&logged_in)));
    assert!(!provider_credential_present(&cfg, Some(&logged_out)));
    assert!(
        !provider_credential_present(&cfg, None),
        "no resolver ⇒ an OAuth provider is not credentialed"
    );
}
