//! Multi-provider `RuntimeConfig + ModelSpec → Arc<dyn LanguageModel>` factory.
//!
//! The inference layer (`services/inference`) is deliberately
//! provider-agnostic — only the `vercel-ai-provider` trait + the
//! high-level `vercel-ai` SDK are in scope there. Concrete provider
//! construction has to happen in a layer that's allowed to depend on
//! each `vercel-ai-{anthropic,openai,google,openai-compatible}` crate,
//! which is the CLI.
//!
//! `build_language_model_from_runtime` is the single binding point
//! between `RuntimeConfig` (Layer 1) and the `vercel-ai-*` crates
//! (Layer 3) — see `multi-provider-plan.md` §6. It threads the typed
//! `ProviderClientOptions` (`headers`, `auth_token`, `organization_id`,
//! `project_id`, `include_usage`, `full_url`,
//! `supports_structured_outputs`) into each provider's
//! `*ProviderSettings`. The match is exhaustive across all six
//! `ProviderApi` variants — adding a new variant is a compile error
//! here.
//!
//! `build_api_client` produces an `Arc<ApiClient>` whose
//! `ProviderClientFingerprint` is computed from the resolved
//! `ProviderConfig` via `ProviderClientFingerprint::compute`. The
//! turn-boundary coherence check (multi-provider-plan §11.1) compares
//! against this fingerprint after hot-reload.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use coco_config::HeaderValue;
use coco_config::ModelInfo;
use coco_config::ProviderClientOptions;
use coco_config::ProviderConfig;
use coco_config::RuntimeConfig;
use coco_types::Capability;
use coco_types::ModelSpec;
use coco_types::ProviderApi;
use coco_types::ProviderModelSelection;
use coco_types::WireApi;
use tracing::warn;

use crate::InferenceError;
use crate::LanguageModel;
use crate::Provider;
use crate::ProviderClientFingerprint;
use crate::RetryConfig;
use crate::client::ApiClient;
use crate::credentials::ProviderCredentialResolver;
use crate::header_template::HeaderVars;
use crate::header_template::PerBuildVars;

/// Surface a `tracing::warn` when a `ProviderClientOptions` field is
/// set to a non-default value but the chosen provider doesn't consume
/// it. Helps users catch typos like setting `auth_token` on an OpenAI
/// instance (which expects `api_key`) or `organization_id` on an
/// Anthropic instance.
fn warn_unused_client_options(provider_name: &str, api: ProviderApi, opts: &ProviderClientOptions) {
    let provider_label = format!("{provider_name} (api={api:?})");

    // auth_token: Anthropic only.
    if opts.auth_token.is_some() && api != ProviderApi::Anthropic {
        warn!(
            provider = %provider_label,
            "client_options.auth_token is Anthropic-specific; ignored on this provider"
        );
    }
    // organization_id / project_id: OpenAI direct only.
    if (opts.organization_id.is_some() || opts.project_id.is_some()) && api != ProviderApi::Openai {
        warn!(
            provider = %provider_label,
            "client_options.organization_id / project_id are OpenAI-specific; ignored on this provider"
        );
    }
    // include_usage / supports_structured_outputs: OpenAI-compat family only.
    let is_compat = matches!(
        api,
        ProviderApi::OpenaiCompat | ProviderApi::Volcengine | ProviderApi::Zai
    );
    if (opts.include_usage.is_some() || opts.supports_structured_outputs) && !is_compat {
        warn!(
            provider = %provider_label,
            "client_options.include_usage / supports_structured_outputs are OpenAI-compat-specific; ignored on this provider"
        );
    }
    // full_url: not honored by Google.
    if opts.full_url && api == ProviderApi::Gemini {
        warn!(
            provider = %provider_label,
            "client_options.full_url is not honored by the Google SDK; ignored"
        );
    }
}

/// Build an `Arc<dyn LanguageModel>` for the (provider, model) pair
/// referenced by `spec`. All six `ProviderApi` variants are wired:
/// `Anthropic` / `Openai` / `Gemini` go through their direct SDKs;
/// `Volcengine` / `Zai` / `OpenaiCompat` route through
/// `vercel-ai-openai-compatible` with the runtime instance name as the
/// SDK's `provider_id` so `model.provider()` round-trips back to
/// `ProviderConfig.name` (closes §7.2). Per-model
/// `info.timeout_secs` (when set) overrides the provider-level
/// timeout for the HTTP client.
/// Whether a usable credential is present for this provider: a non-empty API
/// key for `ApiKey` providers, or a logged-in OAuth supplier for `OAuth`
/// providers. The single source of truth for "is this provider credentialed?"
/// — availability gates (`create_api_client`, the TUI model picker) consume it
/// instead of re-deriving the auth-mode match + resolver consultation, which
/// previously drifted between the two layers.
pub fn provider_credential_present(
    provider_cfg: &ProviderConfig,
    resolver: Option<&Arc<dyn ProviderCredentialResolver>>,
) -> bool {
    match provider_cfg.auth {
        coco_config::ProviderAuth::ApiKey => provider_cfg
            .resolve_api_key()
            .is_some_and(|k| !k.trim().is_empty()),
        coco_config::ProviderAuth::OAuth { .. } => resolver
            .and_then(|r| r.subscription_creds(&provider_cfg.name))
            .is_some(),
    }
}

pub fn build_language_model_from_runtime(
    runtime: &RuntimeConfig,
    spec: &ModelSpec,
    resolver: Option<&Arc<dyn ProviderCredentialResolver>>,
    header_vars: Option<&HeaderVars>,
) -> Result<Arc<dyn LanguageModel>, InferenceError> {
    let provider_cfg = runtime.providers.get(&spec.provider).ok_or_else(|| {
        crate::errors::UnknownProviderSnafu {
            provider: spec.provider.clone(),
        }
        .build()
    })?;
    let api_model_name = resolve_api_model_name(provider_cfg, &spec.model_id);
    warn_unused_client_options(&provider_cfg.name, spec.api, &provider_cfg.client_options);
    let model_info = runtime
        .model_registry
        .resolve(&spec.provider, &spec.model_id)
        .map(|r| r.info.clone());
    let timeout_secs = effective_timeout_secs(provider_cfg, model_info.as_ref());

    // Expand custom-header templates once, here — the resolved `(provider,
    // model, base_url, account_kind)` per-build vars come from scope, the
    // session vars (`session_id`, …) are threaded in via `header_vars`.
    let per_build = PerBuildVars {
        provider: provider_cfg.name.clone(),
        model_id: api_model_name.clone(),
        api: api_family_str(spec.api),
        base_url: provider_cfg.base_url.clone(),
        account_kind: account_kind_str(runtime.account.kind),
    };
    let headers = header_map(&provider_cfg.client_options, header_vars, &per_build)?;

    match spec.api {
        ProviderApi::Anthropic => build_anthropic(
            runtime,
            provider_cfg,
            &api_model_name,
            timeout_secs,
            model_info.as_ref(),
            headers,
        ),
        ProviderApi::Openai => build_openai(
            provider_cfg,
            &api_model_name,
            timeout_secs,
            resolver,
            headers,
        ),
        ProviderApi::Gemini => build_google(provider_cfg, &api_model_name, resolver, headers),
        ProviderApi::Volcengine | ProviderApi::Zai | ProviderApi::OpenaiCompat => {
            build_openai_compat(provider_cfg, &api_model_name, timeout_secs, headers)
        }
    }
}

/// Stable lowercase tag for the `${API}` header variable.
fn api_family_str(api: ProviderApi) -> &'static str {
    match api {
        ProviderApi::Anthropic => "anthropic",
        ProviderApi::Openai => "openai",
        ProviderApi::Gemini => "gemini",
        ProviderApi::Volcengine => "volcengine",
        ProviderApi::Zai => "zai",
        ProviderApi::OpenaiCompat => "openai_compat",
    }
}

/// Stable tag for the `${ACCOUNT_KIND}` header variable.
fn account_kind_str(kind: coco_types::AccountKind) -> &'static str {
    match kind {
        coco_types::AccountKind::ApiKey => "api_key",
        coco_types::AccountKind::ClaudeAiSubscriber => "subscriber",
    }
}

/// Build an `ApiClient` from a `RuntimeConfig + ModelSpec`.
///
/// - The `ProviderClientFingerprint` is computed from the resolved
///   `ProviderConfig` so the turn-boundary coherence check
///   (multi-provider-plan §11.1) detects a stale cached client
///   after hot-reload.
/// - The resolved `ModelInfo` (via `runtime.model_registry.resolve`)
///   is threaded into the client so `query` / `query_stream` can
///   route through `build_call_options` — without this, every
///   `extra_body` / typed-sampling / thinking config the user
///   wrote in `models.json` would be silently dropped.
pub(crate) fn build_api_client(
    runtime: &RuntimeConfig,
    spec: &ModelSpec,
    retry: RetryConfig,
    resolver: Option<&Arc<dyn ProviderCredentialResolver>>,
    header_vars: Option<&HeaderVars>,
) -> Result<Arc<ApiClient>, InferenceError> {
    let provider_cfg = runtime.providers.get(&spec.provider).ok_or_else(|| {
        crate::errors::UnknownProviderSnafu {
            provider: spec.provider.clone(),
        }
        .build()
    })?;
    let api_model_name = resolve_api_model_name(provider_cfg, &spec.model_id);
    let model_info = runtime
        .model_registry
        .resolve(&spec.provider, &spec.model_id)
        .map(|r| r.info.clone());
    let model = build_language_model_from_runtime(runtime, spec, resolver, header_vars)?;
    let fingerprint = ProviderClientFingerprint::compute_with_runtime_state(
        provider_cfg,
        &api_model_name,
        &runtime.account,
        &runtime.prompt_cache,
    );
    // Per-slot cache-break detector: each `ApiClient` (primary +
    // fallbacks) gets its own detector instance. When the runtime
    // switches between slots (`ModelRuntime::on_switch_i13`), the
    // tracked state stays with the originating slot, so re-activating
    // an old slot resumes its baseline rather than restarting Cold.
    let detector = std::sync::Arc::new(tokio::sync::Mutex::new(
        crate::cache_detection::CacheBreakDetector::new(),
    ));
    let model_identity = ProviderModelSelection {
        provider: spec.provider.clone(),
        model_id: spec.model_id.clone(),
    };
    let mut client = ApiClient::new(model, fingerprint, model_info, model_identity, retry)
        .with_cache_break_detector(detector)
        .with_stream_idle_timeout(provider_cfg.client_options.stream_idle_timeout_secs);
    // Reactive-401 hook for OAuth-subscription providers: bind a
    // `refresh_now(provider)` callback so an expired access token recovers
    // (refresh + retry) instead of surfacing the 401.
    if let coco_config::ProviderAuth::OAuth { .. } = provider_cfg.auth
        && let Some(resolver) = resolver
    {
        let resolver = resolver.clone();
        let provider_name = provider_cfg.name.clone();
        client = client.with_refresh_hook(Arc::new(move || resolver.refresh_now(&provider_name)));
    }
    Ok(Arc::new(client))
}

/// Resolve the wire-side model name from the per-(provider, model)
/// override entry. Falls back to the `ModelSpec.model_id` when no
/// `api_model_name` is configured.
fn resolve_api_model_name(provider_cfg: &ProviderConfig, model_id: &str) -> String {
    provider_cfg
        .models
        .get(model_id)
        .and_then(|m| m.api_model_name.clone())
        .unwrap_or_else(|| model_id.to_string())
}

fn build_anthropic(
    runtime: &RuntimeConfig,
    provider_cfg: &ProviderConfig,
    api_model: &str,
    timeout_secs: i64,
    model_info: Option<&ModelInfo>,
    headers: Option<HashMap<String, String>>,
) -> Result<Arc<dyn LanguageModel>, InferenceError> {
    let opts = &provider_cfg.client_options;
    let capabilities = anthropic_caps_from(model_info.and_then(|i| i.capabilities.as_ref()));
    let account_kind = match runtime.account.kind {
        coco_types::AccountKind::ApiKey => vercel_ai_anthropic::AdapterAccountKind::ApiKey,
        coco_types::AccountKind::ClaudeAiSubscriber => {
            vercel_ai_anthropic::AdapterAccountKind::ClaudeAiSubscriber
        }
    };
    // Parse the opaque `provider_options` map through the
    // adapter-owned schema. `deny_unknown_fields` surfaces typos at
    // process startup rather than the next request.
    let knobs = vercel_ai_anthropic::parse_provider_options(&provider_cfg.provider_options)
        .map_err(|e| {
            crate::errors::ProviderBuildFailedSnafu {
                provider: "anthropic",
                provider_name: provider_cfg.name.clone(),
                message: format!("provider_options: {e}"),
            }
            .build()
        })?;
    let settings = vercel_ai_anthropic::AnthropicProviderSettings {
        base_url: Some(provider_cfg.base_url.clone()),
        api_key: provider_cfg.resolve_api_key(),
        auth_token: opts.auth_token.as_ref().map(|t| t.expose().to_string()),
        headers,
        name: Some(provider_cfg.name.clone()),
        client: build_http_client(timeout_secs),
        supports_native_structured_output: None,
        supports_strict_tools: None,
        full_url: Some(opts.full_url),
        capabilities,
        // Single-variant by design. Bedrock / Vertex / Foundry are explicit
        // non-goals — see root `CLAUDE.md` Multi-Provider Boundaries.
        provider_topology: vercel_ai_anthropic::ProviderTopology::FirstParty,
        experimental_betas_enabled: knobs.experimental_betas_enabled,
        disable_interleaved_thinking: knobs.disable_interleaved_thinking,
        show_thinking_summaries: knobs.show_thinking_summaries,
        non_interactive: knobs.non_interactive,
        prompt_cache_allowlist: runtime.prompt_cache.allowlist.clone(),
        account_kind,
        in_overage: runtime.account.in_overage,
    };
    let provider = vercel_ai_anthropic::create_anthropic(settings);
    provider.language_model(api_model).map_err(|e| {
        crate::errors::ProviderBuildFailedSnafu {
            provider: "anthropic",
            provider_name: provider_cfg.name.clone(),
            message: e.to_string(),
        }
        .build()
    })
}

/// Translate `coco_types::Capability` flags into the adapter-local
/// bool-per-feature struct. Unknown capabilities (e.g. `Vision`,
/// `ToolCalling`) are ignored — only prompt-cache + beta-relevant
/// flags matter at the adapter boundary. `None` (model unknown to the
/// registry) → all-false safe default.
fn anthropic_caps_from(
    capabilities: Option<&Vec<Capability>>,
) -> vercel_ai_anthropic::AnthropicModelCapabilities {
    let mut out = vercel_ai_anthropic::AnthropicModelCapabilities::default();
    let Some(caps) = capabilities else {
        return out;
    };
    for cap in caps {
        match cap {
            Capability::PromptCache => out.prompt_cache = true,
            Capability::Context1m => out.context_1m = true,
            Capability::InterleavedThinking => out.interleaved_thinking = true,
            Capability::ContextManagement => out.context_management = true,
            Capability::TokenEfficientTools => out.token_efficient_tools = true,
            Capability::ServerSideToolReference => out.tool_reference = true,
            // Other capabilities are unrelated to Anthropic adapter policy.
            // `AdaptiveThinking` is consumed by the convert layer (gates
            // `thinking: {type:adaptive}` emission), not the adapter — the
            // adapter has its own model-name-pattern `supports_adaptive_thinking`
            // for the typed-reasoning fallback path.
            Capability::TextGeneration
            | Capability::Streaming
            | Capability::Vision
            | Capability::Audio
            | Capability::ToolCalling
            | Capability::Embedding
            | Capability::ExtendedThinking
            | Capability::AdaptiveThinking
            | Capability::StructuredOutput
            | Capability::ReasoningSummaries
            | Capability::ParallelToolCalls
            | Capability::FastMode
            // `ClientSideToolSearch` is a coco-rs-side decision flag
            // (whether the engine should run client-side promotion
            // for this model) — Anthropic adapter never reads it.
            | Capability::ClientSideToolSearch => {}
        }
    }
    out
}

fn build_openai(
    provider_cfg: &ProviderConfig,
    api_model: &str,
    timeout_secs: i64,
    resolver: Option<&Arc<dyn ProviderCredentialResolver>>,
    headers: Option<HashMap<String, String>>,
) -> Result<Arc<dyn LanguageModel>, InferenceError> {
    let opts = &provider_cfg.client_options;
    let auth = build_openai_auth(provider_cfg, resolver)?;
    // Parse the opaque `provider_options` map through the adapter-owned
    // schema. `deny_unknown_fields` surfaces typos at startup rather than the
    // next request.
    let knobs =
        vercel_ai_openai::parse_provider_options(&provider_cfg.provider_options).map_err(|e| {
            crate::errors::ProviderBuildFailedSnafu {
                provider: "openai",
                provider_name: provider_cfg.name.clone(),
                message: format!("provider_options: {e}"),
            }
            .build()
        })?;
    let settings = vercel_ai_openai::OpenAIProviderSettings {
        base_url: Some(provider_cfg.base_url.clone()),
        auth,
        organization: opts.organization_id.clone(),
        project: opts.project_id.clone(),
        headers,
        name: Some(provider_cfg.name.clone()),
        client: build_http_client(timeout_secs),
        full_url: Some(opts.full_url),
        reasoning_store: knobs.reasoning_store,
    };
    let provider = vercel_ai_openai::create_openai(settings);
    // Honor `provider_cfg.wire_api`. The SDK's
    // `Provider::language_model` defaults to Responses, but users
    // who configure `wire_api: chat` (e.g. for Azure Chat Completions
    // routing) expect Chat. Dispatch explicitly.
    let model: Arc<dyn LanguageModel> = match provider_cfg.wire_api {
        WireApi::Chat => Arc::new(provider.chat(api_model)),
        WireApi::Responses => Arc::new(provider.responses(api_model)),
    };
    Ok(model)
}

/// Map `ProviderConfig.auth` + the credential resolver into the OpenAI provider
/// crate's wire-auth mode. This is the seam-legal neutral→wire conversion:
/// `coco-inference` already depends on `vercel-ai-openai`, so the coco-neutral
/// `SubscriptionCreds` supplier is wrapped here into the crate's
/// `ChatGptCreds` closure — no `vercel-ai` type ever crosses into
/// `coco-provider-auth`.
fn build_openai_auth(
    provider_cfg: &ProviderConfig,
    resolver: Option<&Arc<dyn ProviderCredentialResolver>>,
) -> Result<vercel_ai_openai::OpenAIAuth, InferenceError> {
    match provider_cfg.auth {
        coco_config::ProviderAuth::ApiKey => Ok(vercel_ai_openai::OpenAIAuth::ApiKey(
            provider_cfg.resolve_api_key(),
        )),
        coco_config::ProviderAuth::OAuth { flow } => match flow {
            coco_types::OAuthFlowId::OpenAiChatGpt => {
                let supplier = resolver
                    .and_then(|r| r.subscription_creds(&provider_cfg.name))
                    .ok_or_else(|| {
                        crate::errors::ProviderBuildFailedSnafu {
                            provider: "openai",
                            provider_name: provider_cfg.name.clone(),
                            message: "ChatGPT subscription not logged in — run `coco login openai`"
                                .to_string(),
                        }
                        .build()
                    })?;
                let creds: vercel_ai_openai::ChatGptCredsSupplier = Arc::new(move || {
                    supplier().map(|c| vercel_ai_openai::ChatGptCreds {
                        access_token: c.access_token,
                        account_id: c.account_id,
                    })
                });
                Ok(vercel_ai_openai::OpenAIAuth::ChatGptSubscription {
                    creds,
                    originator: vercel_ai_openai::DEFAULT_ORIGINATOR.into(),
                })
            }
            // A Gemini flow on an OpenAI provider is a misconfiguration —
            // GeminiCodeAssist routes through `build_google`, not here.
            coco_types::OAuthFlowId::GeminiCodeAssist => {
                Err(crate::errors::ProviderBuildFailedSnafu {
                    provider: "openai",
                    provider_name: provider_cfg.name.clone(),
                    message: "GeminiCodeAssist OAuth is not valid for an OpenAI provider"
                        .to_string(),
                }
                .build())
            }
        },
    }
}

fn build_google(
    provider_cfg: &ProviderConfig,
    api_model: &str,
    resolver: Option<&Arc<dyn ProviderCredentialResolver>>,
    headers: Option<HashMap<String, String>>,
) -> Result<Arc<dyn LanguageModel>, InferenceError> {
    // Gemini Code Assist subscription (`auth: OAuth`) uses a distinct wire
    // contract (Bearer + `{project, request}` envelope + `:method` RPC +
    // onboarding) served by `vercel-ai-google-codeassist`. API-key Gemini stays
    // on the standard generativelanguage provider below.
    if let coco_config::ProviderAuth::OAuth { flow } = provider_cfg.auth {
        return build_google_code_assist(provider_cfg, api_model, flow, resolver, headers);
    }
    let settings = vercel_ai_google::GoogleGenerativeAIProviderSettings {
        base_url: Some(provider_cfg.base_url.clone()),
        api_key: provider_cfg.resolve_api_key(),
        headers,
        name: Some(provider_cfg.name.clone()),
    };
    let provider = vercel_ai_google::create_google_generative_ai(settings);
    provider.language_model(api_model).map_err(|e| {
        crate::errors::ProviderBuildFailedSnafu {
            provider: "google",
            provider_name: provider_cfg.name.clone(),
            message: e.to_string(),
        }
        .build()
    })
}

/// Map a Code Assist OAuth provider config + the credential resolver into the
/// `vercel-ai-google-codeassist` transport. The seam-legal neutral→wire
/// conversion (mirrors `build_openai_auth`): the coco-neutral
/// `SubscriptionCreds` supplier is wrapped into the crate's
/// `CodeAssistCreds` closure — no `vercel-ai` type crosses into
/// `coco-provider-auth`.
fn build_google_code_assist(
    provider_cfg: &ProviderConfig,
    api_model: &str,
    flow: coco_types::OAuthFlowId,
    resolver: Option<&Arc<dyn ProviderCredentialResolver>>,
    headers: Option<HashMap<String, String>>,
) -> Result<Arc<dyn LanguageModel>, InferenceError> {
    // Only the Gemini flow is valid on a Google provider.
    if !matches!(flow, coco_types::OAuthFlowId::GeminiCodeAssist) {
        return Err(crate::errors::ProviderBuildFailedSnafu {
            provider: "google",
            provider_name: provider_cfg.name.clone(),
            message: format!("{flow} OAuth is not valid for a Google provider"),
        }
        .build());
    }
    let supplier = resolver
        .and_then(|r| r.subscription_creds(&provider_cfg.name))
        .ok_or_else(|| {
            crate::errors::ProviderBuildFailedSnafu {
                provider: "google",
                provider_name: provider_cfg.name.clone(),
                message: "Gemini Code Assist not logged in — run `coco login gemini`".to_string(),
            }
            .build()
        })?;
    let creds: vercel_ai_google_codeassist::CodeAssistCredsSupplier = Arc::new(move || {
        supplier().map(|c| vercel_ai_google_codeassist::CodeAssistCreds {
            access_token: c.access_token,
            project_id: c.project_id,
        })
    });
    let settings = vercel_ai_google_codeassist::GoogleCodeAssistProviderSettings {
        base_url: Some(provider_cfg.base_url.clone()),
        creds,
        headers,
        name: Some(provider_cfg.name.clone()),
        client: None,
    };
    let provider = vercel_ai_google_codeassist::create_google_code_assist(settings);
    provider.language_model(api_model).map_err(|e| {
        crate::errors::ProviderBuildFailedSnafu {
            provider: "google",
            provider_name: provider_cfg.name.clone(),
            message: e.to_string(),
        }
        .build()
    })
}

fn build_openai_compat(
    provider_cfg: &ProviderConfig,
    api_model: &str,
    timeout_secs: i64,
    headers: Option<HashMap<String, String>>,
) -> Result<Arc<dyn LanguageModel>, InferenceError> {
    let opts = &provider_cfg.client_options;
    let knobs = vercel_ai_openai_compatible::parse_provider_options(&provider_cfg.provider_options)
        .map_err(|e| {
            crate::errors::ProviderBuildFailedSnafu {
                provider: "openai-compat",
                provider_name: provider_cfg.name.clone(),
                message: format!("provider_options: {e}"),
            }
            .build()
        })?;
    let settings = vercel_ai_openai_compatible::OpenAICompatibleProviderSettings {
        base_url: Some(provider_cfg.base_url.clone()),
        api_key: provider_cfg.resolve_api_key(),
        api_key_env_var: Some(provider_cfg.env_key.clone()),
        api_key_description: Some(provider_cfg.name.clone()),
        headers,
        query_params: None,
        // The runtime instance name (e.g. `"xai"`, `"volcengine"`,
        // `"internal-router"`) is the namespace key the OpenAI-compat
        // SDK passes through; `model.provider()` answers this back so
        // it round-trips with `build_call_options`'s namespace wrap.
        name: Some(provider_cfg.name.clone()),
        client: build_http_client(timeout_secs),
        include_usage: opts.include_usage,
        supports_structured_outputs: Some(opts.supports_structured_outputs),
        transform_request_body: None,
        metadata_extractor: None,
        error_handler: None,
        prompt_tokens_total_semantics: knobs.prompt_tokens_total_semantics,
        provider_profile: if provider_cfg.name == "deepseek-openai" {
            Some(vercel_ai_openai_compatible::OpenAICompatibleProviderProfile::DeepSeek)
        } else {
            None
        },
        full_url: Some(opts.full_url),
    };
    let provider = vercel_ai_openai_compatible::create_openai_compatible(settings);
    provider.language_model(api_model).map_err(|e| {
        crate::errors::ProviderBuildFailedSnafu {
            provider: "openai-compat",
            provider_name: provider_cfg.name.clone(),
            message: e.to_string(),
        }
        .build()
    })
}

/// Convert `ProviderClientOptions.headers` (BTreeMap, deterministic)
/// into the `HashMap` shape every `vercel-ai-*ProviderSettings.headers`
/// expects, expanding `{ "template": "..." }` values against the built-in
/// variables (see [`crate::header_template`]). Literal values pass through
/// verbatim. Empty maps surface as `None` so the SDK's "no extra headers"
/// path applies. A bad template (unknown variable / unterminated `${`) fails
/// the provider build — surfacing the config error at startup.
fn header_map(
    opts: &ProviderClientOptions,
    vars: Option<&HeaderVars>,
    per_build: &PerBuildVars,
) -> Result<Option<HashMap<String, String>>, InferenceError> {
    if opts.headers.is_empty() {
        return Ok(None);
    }
    let mut out = HashMap::with_capacity(opts.headers.len());
    for (key, value) in &opts.headers {
        let resolved = match value {
            HeaderValue::Literal(literal) => literal.clone(),
            HeaderValue::Templated { template } => {
                crate::header_template::expand(template, vars, per_build).map_err(|e| {
                    crate::errors::ProviderBuildFailedSnafu {
                        provider: "header_template",
                        provider_name: per_build.provider.clone(),
                        message: format!("header `{key}`: {e}"),
                    }
                    .build()
                })?
            }
        };
        out.insert(key.clone(), resolved);
    }
    Ok(Some(out))
}

/// Effective per-request timeout for a (provider, model) pair.
/// Per-model `info.timeout_secs` overrides the provider-level value
/// when set — lets a slow-thinking model declare a higher ceiling
/// than the provider default without inflating the timeout for
/// every other model on the same provider. A negative value cannot
/// reach here (rejected at `ProviderConfig::from_partial`); zero
/// disables the per-request timeout.
fn effective_timeout_secs(provider_cfg: &ProviderConfig, model_info: Option<&ModelInfo>) -> i64 {
    model_info
        .and_then(|i| i.timeout_secs)
        .unwrap_or(provider_cfg.timeout_secs)
}

/// Build a `reqwest::Client` honoring the effective timeout and share
/// it across the language-model construction. The SDK accepts
/// `Option<Arc<reqwest::Client>>`; a single shared client lets all
/// requests against this provider reuse the connection pool. If
/// builder construction fails, falls back to the SDK's default
/// (no-timeout) client so the process still starts.
fn build_http_client(timeout_secs: i64) -> Option<Arc<reqwest::Client>> {
    let timeout = if timeout_secs > 0 {
        match u64::try_from(timeout_secs) {
            Ok(s) => Duration::from_secs(s),
            Err(_) => return None,
        }
    } else {
        // Non-positive timeout disables the per-request timeout.
        return None;
    };
    reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .ok()
        .map(Arc::new)
}

#[cfg(test)]
#[path = "model_factory.test.rs"]
mod tests;
