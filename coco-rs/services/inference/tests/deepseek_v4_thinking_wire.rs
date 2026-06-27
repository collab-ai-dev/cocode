//! Wire-body integration tests for DeepSeek V4 thinking.
//!
//! Verifies the END-TO-END wire body (post `get_args`) for the same
//! `deepseek-v4-flash` builtin `ModelInfo` routed through both:
//!   - `deepseek-openai`     (OpenaiCompat) — DeepSeek's native HTTP path
//!   - `deepseek-anthropic`  (Anthropic)    — DeepSeek's Anthropic-compat path
//!
//! Drives `build_call_options_with_extra` → language model `get_args`
//! and asserts the resulting JSON body matches the documented DeepSeek
//! wire shape.

use std::collections::HashMap;
use std::sync::Arc;

use coco_config::ModelInfo;
use coco_config::PartialModelInfo;
use coco_config::PositiveTokens;
use coco_inference::PerCallOverrides;
use coco_inference::build_call_options_with_extra;
use coco_types::Capability;
use coco_types::ProviderApi;
use coco_types::ReasoningEffort;
use coco_types::ThinkingLevel;
use pretty_assertions::assert_eq;

use vercel_ai_anthropic::AdapterAccountKind;
use vercel_ai_anthropic::AnthropicConfig;
use vercel_ai_anthropic::AnthropicMessagesLanguageModel;
use vercel_ai_anthropic::AnthropicModelCapabilities;
use vercel_ai_anthropic::ProviderTopology;
use vercel_ai_openai_compatible::OpenAICompatibleChatLanguageModel;
use vercel_ai_openai_compatible::OpenAICompatibleConfig;
use vercel_ai_openai_compatible::PromptTokensTotalSemantics;
use vercel_ai_openai_compatible::openai_compatible_error::OpenAICompatibleFailedResponseHandler;
use vercel_ai_provider::LanguageModelV4Message;

/// Builds a `ModelInfo` for `deepseek-v4-flash` mirroring what the
/// builtin registry resolves: 1M context, 12k output, ExtendedThinking,
/// and the 4-level thinking surface (Disable / Auto / Medium / XHigh —
/// `Medium` is the UX label "high", `XHigh` is the UX label "max").
fn deepseek_v4_flash_info(provider_name: &str) -> ModelInfo {
    let levels = vec![
        ThinkingLevel {
            effort: ReasoningEffort::Off,
            budget_tokens: None,
            options: HashMap::from([(
                "thinking".to_string(),
                serde_json::json!({"type": "disabled"}),
            )]),
        },
        ThinkingLevel {
            effort: ReasoningEffort::Auto,
            budget_tokens: None,
            options: HashMap::new(),
        },
        ThinkingLevel {
            effort: ReasoningEffort::Medium,
            budget_tokens: None,
            options: HashMap::from([(
                "thinking".to_string(),
                serde_json::json!({"type": "enabled"}),
            )]),
        },
        ThinkingLevel {
            effort: ReasoningEffort::XHigh,
            budget_tokens: None,
            options: HashMap::from([(
                "thinking".to_string(),
                serde_json::json!({"type": "enabled"}),
            )]),
        },
    ];

    let partial = PartialModelInfo {
        display_name: Some("DeepSeek V4 Flash".into()),
        context_window: Some(PositiveTokens::new(1_000_000)),
        max_output_tokens: Some(PositiveTokens::new(12_288)),
        capabilities: Some(vec![
            Capability::TextGeneration,
            Capability::Streaming,
            Capability::ToolCalling,
            Capability::ExtendedThinking,
            // DeepSeek V4 anthropic-compat supports adaptive thinking.
            // Required so Test B3 (Auto → {type:adaptive}) can produce
            // the wire signal — the convert layer gates this on the
            // capability presence.
            Capability::AdaptiveThinking,
        ]),
        supported_thinking_levels: Some(levels),
        default_thinking_level: Some(ReasoningEffort::Auto),
        ..Default::default()
    };

    ModelInfo::from_partial(provider_name, "deepseek-v4-flash", partial)
        .unwrap_or_else(|e| panic!("ModelInfo::from_partial: {e}"))
}

/// Per-call thinking override mirroring the registry's Medium (UX
/// label "high") entry.
fn medium_thinking_with_enabled_toggle() -> ThinkingLevel {
    ThinkingLevel {
        effort: ReasoningEffort::Medium,
        budget_tokens: None,
        options: HashMap::from([(
            "thinking".to_string(),
            serde_json::json!({"type": "enabled"}),
        )]),
    }
}

fn make_openai_compat_config() -> Arc<OpenAICompatibleConfig> {
    Arc::new(OpenAICompatibleConfig {
        provider: "deepseek-openai.chat".into(),
        base_url: "https://api.deepseek.com".into(),
        headers: Arc::new(HashMap::new),
        query_params: None,
        client: None,
        include_usage: true,
        supports_structured_outputs: false,
        transform_request_body: None,
        metadata_extractor: None,
        supported_urls: None,
        error_handler: Arc::new(OpenAICompatibleFailedResponseHandler::new(
            "deepseek-openai",
        )),
        prompt_tokens_total_semantics: PromptTokensTotalSemantics::Inclusive,
        provider_profile: vercel_ai_openai_compatible::OpenAICompatibleProviderProfile::DeepSeek,
        full_url: None,
    })
}

fn make_anthropic_config() -> Arc<AnthropicConfig> {
    Arc::new(AnthropicConfig {
        provider: "anthropic.messages".into(),
        base_url: "https://api.deepseek.com/anthropic/v1".into(),
        headers: Arc::new(|| {
            let mut h = HashMap::new();
            h.insert("x-api-key".into(), "test-key".into());
            h.insert("anthropic-version".into(), "2023-06-01".into());
            h
        }),
        client: None,
        supports_native_structured_output: None,
        supports_strict_tools: None,
        full_url: None,
        capabilities: AnthropicModelCapabilities::default(),
        provider_topology: ProviderTopology::FirstParty,
        experimental_betas_enabled: false,
        disable_interleaved_thinking: true,
        show_thinking_summaries: false,
        non_interactive: false,
        prompt_cache_allowlist: Vec::new(),
        account_kind: AdapterAccountKind::ApiKey,
        in_overage: false,
    })
}

/// Test A — OpenAI-compat path with explicit Medium level (UX "high").
/// Same `deepseek-v4-flash` ModelInfo driven through the
/// `deepseek-openai` provider with thinking_level = Medium. Final wire
/// body must include the `thinking` enabled toggle and
/// `reasoning_effort: "medium"` (the OpenaiCompat arm derives this from
/// `ReasoningEffort::Display`).
#[test]
fn deepseek_v4_flash_openai_compat_medium_emits_thinking_and_reasoning_effort() {
    let info = deepseek_v4_flash_info("deepseek-openai");
    let per_call = PerCallOverrides {
        thinking_level: Some(medium_thinking_with_enabled_toggle()),
        ..Default::default()
    };

    let (mut call, _merged) = build_call_options_with_extra(
        &info,
        ProviderApi::OpenaiCompat,
        "deepseek-openai",
        &per_call,
        Vec::new(),
        None,
    );
    call.prompt = vec![LanguageModelV4Message::user_text("Hello!")];

    let model =
        OpenAICompatibleChatLanguageModel::new("deepseek-v4-flash", make_openai_compat_config());
    let (body, _warnings) = model
        .get_args(&call)
        .unwrap_or_else(|e| panic!("get_args: {e}"));

    eprintln!(
        "\n===== TEST A · openai-compat · Medium =====\n{}\n",
        serde_json::to_string_pretty(&body).unwrap()
    );

    // Wire body shape:
    //   {"model":"deepseek-v4-flash","messages":[…],
    //    "thinking":{"type":"enabled"},"reasoning_effort":"medium"}
    assert_eq!(body["model"], serde_json::json!("deepseek-v4-flash"));
    assert!(body["messages"].is_array(), "messages must be present");
    assert_eq!(
        body["thinking"],
        serde_json::json!({"type": "enabled"}),
        "thinking toggle must reach the wire body"
    );
    assert_eq!(
        body["reasoning_effort"],
        serde_json::json!("medium"),
        "reasoning_effort must derive from ReasoningEffort::Medium via Display"
    );
}

/// Test A2 — Auto level: NO thinking-related fields on the wire. Per
/// DeepSeek docs the server defaults to enabled+high (or max for Agent
/// requests) when no `thinking` field is sent — the `Auto` level is
/// the explicit "let provider decide" signal at the coco-rs layer.
#[test]
fn deepseek_v4_flash_openai_compat_auto_emits_no_thinking_fields() {
    let info = deepseek_v4_flash_info("deepseek-openai");
    let per_call = PerCallOverrides {
        thinking_level: Some(ThinkingLevel::auto()),
        ..Default::default()
    };

    let (mut call, _merged) = build_call_options_with_extra(
        &info,
        ProviderApi::OpenaiCompat,
        "deepseek-openai",
        &per_call,
        Vec::new(),
        None,
    );
    call.prompt = vec![LanguageModelV4Message::user_text("Hello!")];

    let model =
        OpenAICompatibleChatLanguageModel::new("deepseek-v4-flash", make_openai_compat_config());
    let (body, _warnings) = model
        .get_args(&call)
        .unwrap_or_else(|e| panic!("get_args: {e}"));

    eprintln!(
        "\n===== TEST A2 · openai-compat · Auto =====\n{}\n",
        serde_json::to_string_pretty(&body).unwrap()
    );

    assert_eq!(body["model"], serde_json::json!("deepseek-v4-flash"));
    assert!(
        body.get("thinking").is_none(),
        "Auto must NOT emit `thinking` — provider decides"
    );
    assert!(
        body.get("reasoning_effort").is_none(),
        "Auto must NOT emit `reasoning_effort` — provider decides"
    );
}

/// Test A3 — Disable level: emits `{"thinking":{"type":"disabled"}}`
/// only; no `reasoning_effort` since the typed-arm gate skips Disable.
#[test]
fn deepseek_v4_flash_openai_compat_disable_emits_disabled_toggle_only() {
    let info = deepseek_v4_flash_info("deepseek-openai");

    let mut disable = ThinkingLevel::disable();
    disable
        .options
        .insert("thinking".into(), serde_json::json!({"type": "disabled"}));
    let per_call = PerCallOverrides {
        thinking_level: Some(disable),
        ..Default::default()
    };

    let (mut call, _merged) = build_call_options_with_extra(
        &info,
        ProviderApi::OpenaiCompat,
        "deepseek-openai",
        &per_call,
        Vec::new(),
        None,
    );
    call.prompt = vec![LanguageModelV4Message::user_text("Hello!")];

    let model =
        OpenAICompatibleChatLanguageModel::new("deepseek-v4-flash", make_openai_compat_config());
    let (body, _warnings) = model
        .get_args(&call)
        .unwrap_or_else(|e| panic!("get_args: {e}"));

    eprintln!(
        "\n===== TEST A3 · openai-compat · Disable =====\n{}\n",
        serde_json::to_string_pretty(&body).unwrap()
    );

    assert_eq!(
        body["thinking"],
        serde_json::json!({"type": "disabled"}),
        "Disable must emit explicit-off toggle on the wire"
    );
    assert!(
        body.get("reasoning_effort").is_none(),
        "Disable must NOT emit reasoning_effort"
    );
}

/// Test B — Anthropic path with explicit Medium level (UX "high").
/// Same `deepseek-v4-flash` ModelInfo driven through the
/// `deepseek-anthropic` provider. Asserts the no-fallback contract:
/// ModelInfo declares no `budget_tokens` for DeepSeek levels, so the
/// wire body must emit `{"type":"enabled"}` *only* and `max_tokens`
/// must equal the builtin's `max_output_tokens` — no synthetic 1024
/// budget, no `max_tokens` bump.
#[test]
fn deepseek_v4_flash_anthropic_medium_emits_thinking_enabled() {
    let info = deepseek_v4_flash_info("deepseek-anthropic");
    let per_call = PerCallOverrides {
        thinking_level: Some(medium_thinking_with_enabled_toggle()),
        ..Default::default()
    };

    let (mut call, _merged) = build_call_options_with_extra(
        &info,
        ProviderApi::Anthropic,
        "deepseek-anthropic",
        &per_call,
        Vec::new(),
        None,
    );
    call.prompt = vec![LanguageModelV4Message::user_text("Hello!")];

    let model = AnthropicMessagesLanguageModel::new("deepseek-v4-flash", make_anthropic_config());
    let (body, headers, _warnings) = model
        .get_args(&call, false)
        .unwrap_or_else(|e| panic!("get_args: {e}"));

    eprintln!(
        "\n===== TEST B · anthropic · Medium =====\nheaders: {:#?}\nbody:\n{}\n",
        headers,
        serde_json::to_string_pretty(&body).unwrap()
    );

    assert_eq!(body["model"], serde_json::json!("deepseek-v4-flash"));
    assert!(body["messages"].is_array(), "messages must be present");
    assert_eq!(
        body["thinking"],
        serde_json::json!({"type": "enabled"}),
        "thinking object must be exactly {{type: enabled}} — no synthesized budget_tokens"
    );
    assert!(
        body["thinking"].get("budget_tokens").is_none(),
        "DeepSeek anthropic-compat must NOT carry budget_tokens — ModelInfo declared None"
    );
    // New API surface: output_config.effort derived from level.effort
    // (Medium → "medium"). Goes via raw shallow-merge so no
    // `effort-2025-11-24` beta header is added.
    assert_eq!(
        body["output_config"],
        serde_json::json!({"effort": "medium"}),
        "Medium effort must surface as output_config.effort = \"medium\""
    );
    // builtin max_output_tokens = 12_288 (deepseek_v4_flash_info above).
    // Anthropic provider must NOT bump it when budget is absent.
    assert_eq!(
        body["max_tokens"],
        serde_json::json!(12_288),
        "max_tokens must equal builtin max_output_tokens — no synthetic bump"
    );
    // No `reasoning_effort` on Anthropic wire — that key is
    // OpenaiCompat-specific.
    assert!(
        body.get("reasoning_effort").is_none(),
        "Anthropic wire must NOT carry reasoning_effort"
    );
}

/// Test B2 — Anthropic + XHigh (UX "max"). Wire body has
/// `output_config.effort == "max"` and the standard enabled thinking
/// object.
#[test]
fn deepseek_v4_flash_anthropic_xhigh_emits_output_config_max() {
    let info = deepseek_v4_flash_info("deepseek-anthropic");
    let per_call = PerCallOverrides {
        thinking_level: Some(ThinkingLevel {
            effort: ReasoningEffort::XHigh,
            budget_tokens: None,
            options: HashMap::from([(
                "thinking".to_string(),
                serde_json::json!({"type": "enabled"}),
            )]),
        }),
        ..Default::default()
    };

    let (mut call, _merged) = build_call_options_with_extra(
        &info,
        ProviderApi::Anthropic,
        "deepseek-anthropic",
        &per_call,
        Vec::new(),
        None,
    );
    call.prompt = vec![LanguageModelV4Message::user_text("Hello!")];

    let model = AnthropicMessagesLanguageModel::new("deepseek-v4-flash", make_anthropic_config());
    let (body, _headers, _warnings) = model
        .get_args(&call, false)
        .unwrap_or_else(|e| panic!("get_args: {e}"));

    eprintln!(
        "\n===== TEST B2 · anthropic · XHigh =====\n{}\n",
        serde_json::to_string_pretty(&body).unwrap()
    );

    assert_eq!(body["thinking"], serde_json::json!({"type": "enabled"}),);
    assert_eq!(
        body["output_config"],
        serde_json::json!({"effort": "max"}),
        "XHigh effort must surface as output_config.effort = \"max\""
    );
    assert_eq!(body["max_tokens"], serde_json::json!(12_288));
}

/// Test B3 — Anthropic + Auto. Wire body has
/// `thinking: {"type": "adaptive"}` and NO `output_config`. Server
/// picks effort dynamically.
#[test]
fn deepseek_v4_flash_anthropic_auto_emits_adaptive_thinking() {
    let info = deepseek_v4_flash_info("deepseek-anthropic");
    let per_call = PerCallOverrides {
        thinking_level: Some(ThinkingLevel::auto()),
        ..Default::default()
    };

    let (mut call, _merged) = build_call_options_with_extra(
        &info,
        ProviderApi::Anthropic,
        "deepseek-anthropic",
        &per_call,
        Vec::new(),
        None,
    );
    call.prompt = vec![LanguageModelV4Message::user_text("Hello!")];

    let model = AnthropicMessagesLanguageModel::new("deepseek-v4-flash", make_anthropic_config());
    let (body, _headers, _warnings) = model
        .get_args(&call, false)
        .unwrap_or_else(|e| panic!("get_args: {e}"));

    eprintln!(
        "\n===== TEST B3 · anthropic · Auto =====\n{}\n",
        serde_json::to_string_pretty(&body).unwrap()
    );

    assert_eq!(
        body["thinking"],
        serde_json::json!({"type": "adaptive"}),
        "Auto on Anthropic must surface as thinking: adaptive"
    );
    assert!(
        body.get("output_config").is_none(),
        "Auto must NOT emit output_config — server picks effort"
    );
    assert_eq!(body["max_tokens"], serde_json::json!(12_288));
}

/// Test B4 — Anthropic + Disable. Wire body actively carries the
/// disabled toggle so the server doesn't fall back to thinking-on
/// default. Validates both the convert-layer typed write AND the
/// vercel-ai-anthropic body builder (which previously parsed
/// `ThinkingConfig::Disabled` but never wrote it to the body).
#[test]
fn deepseek_v4_flash_anthropic_disable_emits_disabled_thinking() {
    let info = deepseek_v4_flash_info("deepseek-anthropic");
    let per_call = PerCallOverrides {
        thinking_level: Some(ThinkingLevel::disable()),
        ..Default::default()
    };

    let (mut call, _merged) = build_call_options_with_extra(
        &info,
        ProviderApi::Anthropic,
        "deepseek-anthropic",
        &per_call,
        Vec::new(),
        None,
    );
    call.prompt = vec![LanguageModelV4Message::user_text("Hello!")];

    let model = AnthropicMessagesLanguageModel::new("deepseek-v4-flash", make_anthropic_config());
    let (body, _headers, _warnings) = model
        .get_args(&call, false)
        .unwrap_or_else(|e| panic!("get_args: {e}"));

    eprintln!(
        "\n===== TEST B4 · anthropic · Disable =====\n{}\n",
        serde_json::to_string_pretty(&body).unwrap()
    );

    assert_eq!(
        body["thinking"],
        serde_json::json!({"type": "disabled"}),
        "Disable on Anthropic must surface as thinking: disabled"
    );
    assert!(
        body.get("output_config").is_none(),
        "Disable must NOT emit output_config"
    );
    assert_eq!(body["max_tokens"], serde_json::json!(12_288));
}
