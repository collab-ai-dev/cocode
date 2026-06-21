//! Build a `RuntimeConfig` populated only with builtin providers and
//! resolve a `(provider, model)` pair into a `ModelRuntimeClient` —
//! the single seam-approved entry point into the AI SDK chain. Tests
//! call `client.query` / `client.query_stream` through the runtime;
//! provider-direct `vercel-ai` SDK access is forbidden by the seam guard.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;

use anyhow::Context;
use anyhow::Result;
use coco_config::CatalogPaths;
use coco_config::ProviderConfig;
use coco_config::RuntimeConfig;
use coco_config::RuntimeConfigBuilder;
use coco_inference::ModelRuntimeClient;
use coco_inference::ModelRuntimeRegistry;
use coco_inference::ModelRuntimeSource;
use coco_types::ModelSpec;

use crate::common::env::ensure_env_loaded;

/// Env-var prefix for per-provider builtin overrides driven from `.env`.
/// Form: `COCO_LIVE_TEST_<NAME>_<FIELD>` where `<NAME>` upper-cases the
/// builtin provider name with `-` → `_` (e.g. `OPENAI`, `DEEPSEEK_OPENAI`).
const PROVIDER_OVERRIDE_PREFIX: &str = "COCO_LIVE_TEST_";

/// Builtin provider names eligible for `.env`-driven overrides. Mirrors
/// the registry in `coco_config::builtin_providers()`. Keep this list
/// short — overrides only exist to point a builtin at an alternate
/// gateway (TikTok GPT proxy, custom Anthropic mirror, …) without
/// touching `~/.coco/providers.json`.
const OVERRIDABLE_PROVIDERS: &[&str] = &[
    "openai",
    "anthropic",
    "google",
    "volcengine",
    "zai",
    "deepseek-openai",
    "deepseek-anthropic",
];

/// Override fields recognised on each provider. Maps the env-var token
/// (right-hand side of `COCO_LIVE_TEST_<NAME>_<TOKEN>`) to the matching
/// `PartialProviderConfig` field name in the synthesized JSON overlay.
///
/// Surface area is intentionally narrow:
/// - `API_KEY` → `api_key` (test-isolated credential; native
///   `OPENAI_API_KEY` etc. still wins per
///   `ProviderConfig::resolve_api_key`)
/// - `BASE_URL` → `base_url` (alternate gateway / mirror)
/// - `WIRE_API` → `wire_api` (`responses` / `chat`; rare)
/// - `FULL_URL` → `client_options.full_url` (parsed as bool; needed
///   when the gateway embeds auth in the path/query, e.g. `?ak=…`,
///   so the SDK must use `base_url` verbatim instead of appending
///   `/responses` etc.)
///
/// Note: per-test `MODEL` is *not* an overlay field — it's a per-call
/// argument to `build_api_client`, not part of provider config. The
/// test framework reads `COCO_LIVE_TEST_<NAME>_MODEL` directly via
/// `crate::common::env::provider_model` and threads it through
/// `LiveTarget`.
const OVERRIDE_FIELDS: &[(&str, &str)] = &[
    ("API_KEY", "api_key"),
    ("BASE_URL", "base_url"),
    ("WIRE_API", "wire_api"),
];

/// Cached runtime — building it touches the filesystem (tempdir creation
/// and provider resolution), so we share one across all tests in a
/// process. Tests do not mutate it; `build_*` helpers only read.
static RUNTIME: OnceLock<TestRuntime> = OnceLock::new();

/// Owns the tempdir whose lifetime backs `RuntimeConfig.paths` so it
/// outlives every test in the process.
struct TestRuntime {
    runtime: Arc<RuntimeConfig>,
    _home: tempfile::TempDir,
}

/// Build (or fetch the cached) `RuntimeConfig` whose only providers are
/// the compile-time builtins. No `~/.coco/providers.json` overlay, no
/// `settings.json`, no managed policy file — the catalog paths point at
/// a fresh tempdir whose files don't exist.
///
/// Live `DEEPSEEK_API_KEY` (etc.) flows through because we use
/// `EnvSnapshot::from_current_process()`. The `.env` file loaded by
/// `ensure_env_loaded()` populates `std::env` first, so subsequent
/// `EnvSnapshot::from_current_process()` calls see those values.
pub fn shared_runtime() -> &'static Arc<RuntimeConfig> {
    let entry = RUNTIME.get_or_init(|| {
        ensure_env_loaded();
        let home = tempfile::tempdir().expect("create test tempdir");
        let catalogs = CatalogPaths::empty_in(home.path());
        materialize_provider_overlay(&catalogs.providers)
            .expect("write provider overlay JSON from .env overrides");
        // Multi-LLM SDK: Main has no implicit default. The shared
        // runtime is only used as a builtin-provider catalog (see
        // `provider_config` / `spec_for`); Main resolution is not
        // exercised, but `build()` still requires a value. Pin a
        // builtin so this passes even when the host has no
        // `~/.coco/settings.json`.
        let overrides = coco_config::RuntimeOverrides {
            model_override: Some(coco_types::ProviderModelSelection {
                provider: "anthropic".into(),
                model_id: "claude-opus-4-7".into(),
            }),
            ..Default::default()
        };
        let runtime = RuntimeConfigBuilder::from_process(home.path())
            .with_catalog_paths(catalogs)
            .with_overrides(overrides)
            .build()
            .expect("build empty-overlay runtime");
        TestRuntime {
            runtime: Arc::new(runtime),
            _home: home,
        }
    });
    &entry.runtime
}

/// Synthesize a `providers.json` overlay from `COCO_LIVE_TEST_<NAME>_<FIELD>`
/// env vars and write it to `path`. Skips when no overrides are set so
/// the empty-catalog default behavior is preserved.
///
/// The overlay shape is identical to user-authored `~/.coco/providers.json`
/// — we feed it through the same `apply_partial_layer` path the production
/// resolver uses, so anything legal there works here.
fn materialize_provider_overlay(path: &Path) -> Result<()> {
    let mut overlay = serde_json::Map::new();
    for &provider in OVERRIDABLE_PROVIDERS {
        let prefix = format!(
            "{PROVIDER_OVERRIDE_PREFIX}{}_",
            provider.to_uppercase().replace('-', "_")
        );
        let mut fields = serde_json::Map::new();
        for &(env_token, json_field) in OVERRIDE_FIELDS {
            let key = format!("{prefix}{env_token}");
            if let Ok(value) = std::env::var(&key)
                && !value.trim().is_empty()
            {
                fields.insert(
                    json_field.to_string(),
                    serde_json::Value::String(value.trim().to_string()),
                );
            }
        }
        // `FULL_URL` lives under nested `client_options.full_url` and is
        // a bool, so it can't share the flat string-only loop above.
        if let Ok(value) = std::env::var(format!("{prefix}FULL_URL"))
            && let Ok(parsed) = value.trim().parse::<bool>()
        {
            let mut client_opts = serde_json::Map::new();
            client_opts.insert("full_url".into(), serde_json::Value::Bool(parsed));
            fields.insert(
                "client_options".into(),
                serde_json::Value::Object(client_opts),
            );
        }
        if !fields.is_empty() {
            overlay.insert(provider.to_string(), serde_json::Value::Object(fields));
        }
    }
    if overlay.is_empty() {
        return Ok(());
    }

    let json = serde_json::to_string_pretty(&serde_json::Value::Object(overlay))
        .context("serialize provider overlay")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("mkdir {}", parent.display()))?;
    }
    std::fs::write(path, json).with_context(|| format!("write {}", path.display()))?;
    eprintln!(
        "[coco-tests-live] wrote provider overlay from .env to {}",
        path.display()
    );
    Ok(())
}

/// Borrow a builtin provider config from the shared runtime. Borrow is
/// tied to the static `RuntimeConfig` (lives for the whole process) so
/// no clone is needed.
pub fn provider_config(name: &str) -> Option<&'static ProviderConfig> {
    shared_runtime().providers.get(name)
}

/// Construct a `ModelSpec` whose `api` matches the provider's builtin
/// declaration. This is the same shape the production codepath produces
/// when resolving a `models.<role>` entry.
pub fn spec_for(provider_name: &str, model_id: &str) -> Result<ModelSpec> {
    let cfg = provider_config(provider_name)
        .with_context(|| format!("provider `{provider_name}` is not a builtin"))?;
    Ok(ModelSpec {
        provider: cfg.name.clone(),
        api: cfg.api,
        model_id: model_id.to_string(),
        display_name: model_id.to_string(),
    })
}

/// `true` when the provider's `env_key` is set to a non-empty value in
/// the live process environment. Mirrors `ProviderConfig::resolve_api_key`
/// without surfacing the secret.
pub fn provider_has_credentials(provider_name: &str) -> bool {
    ensure_env_loaded();
    provider_config(provider_name)
        .and_then(coco_config::ProviderConfig::resolve_api_key)
        .is_some()
}

/// Build a model-runtime client for `(provider, model)` so suites can exercise
/// `coco-inference` retry, usage accumulation, and cache-break wiring.
/// All AI SDK access flows through this — direct `vercel-ai` use is
/// blocked by the workspace seam guard.
pub fn build_client(provider_name: &str, model_id: &str) -> Result<Arc<ModelRuntimeClient>> {
    let runtime = shared_runtime();
    let _spec = spec_for(provider_name, model_id)?;
    let registry = Arc::new(ModelRuntimeRegistry::new(
        runtime.clone(),
        None,
        Arc::new(coco_inference::HeaderVars::empty()),
    )?);
    let source = ModelRuntimeSource::Explicit(coco_types::ProviderModelSelection {
        provider: provider_name.to_string(),
        model_id: model_id.to_string(),
    });
    Ok(Arc::new(ModelRuntimeClient::new(registry, source)))
}

/// Resolved test target — provider name, the model-id under test, and a
/// pre-built runtime client.
pub struct LiveTarget {
    pub provider: &'static str,
    pub model: String,
    pub client: Arc<ModelRuntimeClient>,
}

impl LiveTarget {
    /// Build a target for `(provider, model)`. `None` when credentials
    /// are missing or the provider is not on the allow-list.
    pub fn try_resolve(provider: &'static str, model: &str) -> Option<Result<Self>> {
        if !crate::common::env::provider_allowed(provider) {
            return None;
        }
        if !provider_has_credentials(provider) {
            return None;
        }
        Some(
            build_client(provider, model)
                .map(|client| LiveTarget {
                    provider,
                    model: model.to_string(),
                    client,
                })
                .with_context(|| format!("build_client({provider}, {model})")),
        )
    }

    /// Same as `try_resolve` but reads the model from
    /// `COCO_LIVE_TEST_<PROVIDER>_MODEL`. Returns `None` when either the
    /// model env var or the provider's API key is unset (so the test
    /// skips with a single-line message).
    pub fn try_resolve_env_model(provider: &'static str) -> Option<Result<Self>> {
        let model = crate::common::env::provider_model(provider)?;
        Self::try_resolve(provider, &model)
    }
}

/// Optional override path printed by skip messages so users know what
/// they need to set. Returned for diagnostic prints only.
pub fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}
