use std::collections::HashMap;
use std::sync::Arc;

use super::*;

fn make_config() -> Arc<AnthropicConfig> {
    make_config_with_caps(crate::anthropic_config::AnthropicModelCapabilities::default())
}

fn make_config_with_caps(
    capabilities: crate::anthropic_config::AnthropicModelCapabilities,
) -> Arc<AnthropicConfig> {
    Arc::new(AnthropicConfig {
        provider: "anthropic.messages".into(),
        base_url: "https://api.anthropic.com/v1".into(),
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
        capabilities,
        provider_topology: crate::anthropic_config::ProviderTopology::FirstParty,
        experimental_betas_enabled: true,
        disable_interleaved_thinking: false,
        show_thinking_summaries: false,
        non_interactive: false,
        prompt_cache_allowlist: Vec::new(),
        account_kind: crate::anthropic_config::AdapterAccountKind::ApiKey,
        in_overage: false,
    })
}

#[test]
fn creates_model_with_provider_and_id() {
    let model = AnthropicMessagesLanguageModel::new("claude-sonnet-4-5", make_config());
    assert_eq!(model.provider(), "anthropic.messages");
    assert_eq!(model.model_id(), "claude-sonnet-4-5");
}

#[test]
fn supported_urls_includes_images_and_pdfs() {
    let model = AnthropicMessagesLanguageModel::new("claude-sonnet-4-5", make_config());
    let urls = model.supported_urls();
    assert!(urls.contains_key("image/*"));
    assert!(urls.contains_key("application/pdf"));
}

#[test]
fn get_args_sets_model_and_max_tokens() {
    let model = AnthropicMessagesLanguageModel::new("claude-sonnet-4-5", make_config());
    let options = LanguageModelV4CallOptions::new(vec![
        vercel_ai_provider::LanguageModelV4Message::user_text("Hello"),
    ]);
    let (body, _headers, _warnings) = model
        .get_args(&options, false)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(body["model"], "claude-sonnet-4-5");
    assert!(body["max_tokens"].is_number());
    assert!(body["system"].is_null());
    assert!(body["messages"].is_array());
}

#[test]
fn layout_system_blocks_override_converter_blocks_and_carry_cache_control() {
    let model = AnthropicMessagesLanguageModel::new("claude-sonnet-4-5", make_config());
    let mut po = vercel_ai_provider::ProviderOptions::default();
    let mut layout_inner = HashMap::new();
    let blocks = serde_json::json!([
        {
            "text": "you are coco",
            "cache_control": { "type": "ephemeral", "ttl": "5m" }
        }
    ]);
    layout_inner.insert("system_blocks".to_string(), blocks);
    po.set("prompt_layout", layout_inner);
    let mut options = LanguageModelV4CallOptions::new(vec![
        vercel_ai_provider::LanguageModelV4Message::System {
            content: vec![vercel_ai_provider::UserContentPart::Text(
                vercel_ai_provider::TextPart {
                    text: "converter-derived".into(),
                    provider_metadata: None,
                },
            )],
            provider_options: None,
        },
        vercel_ai_provider::LanguageModelV4Message::user_text("Hi"),
    ]);
    options.provider_options = Some(po);

    let (body, _headers, warnings) = model
        .get_args(&options, false)
        .unwrap_or_else(|e| panic!("{e}"));
    let system = body["system"].as_array().expect("system array");
    assert_eq!(system.len(), 1);
    assert_eq!(system[0]["type"], "text");
    assert_eq!(system[0]["text"], "you are coco");
    assert_eq!(system[0]["cache_control"]["type"], "ephemeral");
    assert_eq!(system[0]["cache_control"]["ttl"], "5m");
    assert!(
        warnings.iter().any(|w| matches!(
            w,
            Warning::Other { message, .. } if message.contains("layout wins")
        )),
        "expected a Warning::Other documenting layout precedence"
    );
}

#[test]
fn get_args_warns_on_unsupported_params() {
    let model = AnthropicMessagesLanguageModel::new("claude-sonnet-4-5", make_config());
    let mut options = LanguageModelV4CallOptions::new(vec![
        vercel_ai_provider::LanguageModelV4Message::user_text("Hello"),
    ]);
    options.frequency_penalty = Some(0.5);
    options.presence_penalty = Some(0.5);
    options.seed = Some(42);
    let (_body, _headers, warnings) = model
        .get_args(&options, false)
        .unwrap_or_else(|e| panic!("{e}"));
    let features: Vec<&str> = warnings
        .iter()
        .filter_map(|w| match w {
            Warning::Unsupported { feature, .. } => Some(feature.as_str()),
            _ => None,
        })
        .collect();
    assert!(features.contains(&"frequencyPenalty"));
    assert!(features.contains(&"presencePenalty"));
    assert!(features.contains(&"seed"));
}

#[test]
fn get_args_clamps_temperature() {
    let model = AnthropicMessagesLanguageModel::new("claude-sonnet-4-5", make_config());
    let options = LanguageModelV4CallOptions::new(vec![
        vercel_ai_provider::LanguageModelV4Message::user_text("Hello"),
    ])
    .with_temperature(2.0);
    let (body, _headers, warnings) = model
        .get_args(&options, false)
        .unwrap_or_else(|e| panic!("{e}"));
    // Temperature should be clamped to 1.0
    assert_eq!(body["temperature"], 1.0);
    assert!(
        warnings
            .iter()
            .any(|w| matches!(w, Warning::Unsupported { feature, .. } if feature == "temperature"))
    );
}

#[test]
fn get_args_includes_anthropic_beta_header() {
    let model = AnthropicMessagesLanguageModel::new("claude-sonnet-4-5", make_config());

    // Create options with tools to trigger beta header
    let tool = vercel_ai_provider::LanguageModelV4Tool::Provider(
        vercel_ai_provider::LanguageModelV4ProviderTool {
            id: "anthropic.code_execution_20250522".into(),
            name: "code_execution".into(),
            args: HashMap::new(),
        },
    );

    let options = LanguageModelV4CallOptions::new(vec![
        vercel_ai_provider::LanguageModelV4Message::user_text("Hello"),
    ])
    .with_tools(vec![tool]);

    let (_body, headers, _warnings) = model
        .get_args(&options, false)
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(headers.contains_key("anthropic-beta"));
    let beta = &headers["anthropic-beta"];
    assert!(beta.contains("code-execution-2025-05-22"));
}

#[test]
fn get_args_stream_includes_stream_flag() {
    let model = AnthropicMessagesLanguageModel::new("claude-sonnet-4-5", make_config());
    let options = LanguageModelV4CallOptions::new(vec![
        vercel_ai_provider::LanguageModelV4Message::user_text("Hello"),
    ]);
    let (body, headers, _warnings) = model
        .get_args(&options, true)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(body["stream"], true);
    // Should have fine-grained tool streaming beta
    assert!(
        headers
            .get("anthropic-beta")
            .map(|b| b.contains("fine-grained-tool-streaming"))
            .unwrap_or(false)
    );
}

// ---------------------------------------------------------------------------
// Skills validation: code execution tool required
// ---------------------------------------------------------------------------

#[test]
fn skills_without_code_execution_tool_produces_warning() {
    let model = AnthropicMessagesLanguageModel::new("claude-sonnet-4-5", make_config());

    // Container with skills but no code execution tool
    let mut anthropic_opts: HashMap<String, serde_json::Value> = HashMap::new();
    anthropic_opts.insert(
        "container".into(),
        json!({
            "skills": [{"type": "skill", "skillId": "my_skill"}]
        }),
    );
    let mut provider_opts = HashMap::new();
    provider_opts.insert("anthropic".into(), anthropic_opts);

    let options = LanguageModelV4CallOptions::new(vec![
        vercel_ai_provider::LanguageModelV4Message::user_text("Hello"),
    ])
    .with_provider_options(vercel_ai_provider::ProviderOptions(provider_opts));

    let (_body, _headers, warnings) = model
        .get_args(&options, false)
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(
        warnings.iter().any(|w| matches!(
            w,
            Warning::Other { message }
                if message.contains("code execution tool is required when using skills")
        )),
        "expected skills validation warning, got: {warnings:?}",
    );
}

#[test]
fn skills_with_code_execution_tool_no_warning() {
    let model = AnthropicMessagesLanguageModel::new("claude-sonnet-4-5", make_config());

    let mut anthropic_opts: HashMap<String, serde_json::Value> = HashMap::new();
    anthropic_opts.insert(
        "container".into(),
        json!({
            "skills": [{"type": "skill", "skillId": "my_skill"}]
        }),
    );
    let mut provider_opts = HashMap::new();
    provider_opts.insert("anthropic".into(), anthropic_opts);

    let ce_tool = vercel_ai_provider::LanguageModelV4Tool::Provider(
        vercel_ai_provider::LanguageModelV4ProviderTool {
            id: "anthropic.code_execution_20250825".into(),
            name: "code_execution".into(),
            args: HashMap::new(),
        },
    );

    let options = LanguageModelV4CallOptions::new(vec![
        vercel_ai_provider::LanguageModelV4Message::user_text("Hello"),
    ])
    .with_tools(vec![ce_tool])
    .with_provider_options(vercel_ai_provider::ProviderOptions(provider_opts));

    let (_body, _headers, warnings) = model
        .get_args(&options, false)
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(
        !warnings.iter().any(|w| matches!(
            w,
            Warning::Other { message }
                if message.contains("code execution tool is required")
        )),
        "should not warn when code_execution tool is present: {warnings:?}",
    );
}

// ---------------------------------------------------------------------------
// is_known_model + max_tokens clamping
// ---------------------------------------------------------------------------

#[test]
fn known_model_clamps_max_tokens_with_warning() {
    let model = AnthropicMessagesLanguageModel::new("claude-sonnet-4-5", make_config());
    // claude-sonnet-4-5 has max_output_tokens = 64_000
    let options = LanguageModelV4CallOptions::new(vec![
        vercel_ai_provider::LanguageModelV4Message::user_text("Hello"),
    ])
    .with_max_output_tokens(100_000);
    let (body, _headers, warnings) = model
        .get_args(&options, false)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(body["max_tokens"], 64_000);
    assert!(
        warnings.iter().any(|w| matches!(
            w,
            Warning::Unsupported { feature, .. } if feature == "maxOutputTokens"
        )),
        "expected maxOutputTokens warning, got: {warnings:?}",
    );
}

#[test]
fn known_model_clamps_without_warning_when_no_explicit_max() {
    // Unknown model should not clamp
    let model = AnthropicMessagesLanguageModel::new("some-custom-model", make_config());
    let options = LanguageModelV4CallOptions::new(vec![
        vercel_ai_provider::LanguageModelV4Message::user_text("Hello"),
    ]);
    let (body, _headers, warnings) = model
        .get_args(&options, false)
        .unwrap_or_else(|e| panic!("{e}"));
    // Unknown model defaults to 4096
    assert_eq!(body["max_tokens"], 4096);
    assert!(!warnings.iter().any(
        |w| matches!(w, Warning::Unsupported { feature, .. } if feature == "maxOutputTokens")
    ),);
}

// ---------------------------------------------------------------------------
// JSON response format warning when schema is null
// ---------------------------------------------------------------------------

#[test]
fn json_response_format_without_schema_warns() {
    let model = AnthropicMessagesLanguageModel::new("claude-sonnet-4-5", make_config());
    let options = LanguageModelV4CallOptions::new(vec![
        vercel_ai_provider::LanguageModelV4Message::user_text("Hello"),
    ])
    .with_response_format(vercel_ai_provider::ResponseFormat::Json {
        schema: None,
        name: None,
        description: None,
    });
    let (_body, _headers, warnings) = model
        .get_args(&options, false)
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(
        warnings.iter().any(|w| matches!(
            w,
            Warning::Unsupported { feature, details }
                if feature == "responseFormat"
                && details.as_deref().unwrap_or("").contains("requires a schema")
        )),
        "expected responseFormat warning, got: {warnings:?}",
    );
}

// ---------------------------------------------------------------------------
// Thinking enabled without budget — provider does NOT synthesize a default
// ---------------------------------------------------------------------------

#[test]
fn thinking_enabled_without_budget_emits_no_budget_tokens() {
    // ModelInfo is the single source of truth for budget_tokens. When the
    // upper layer supplies `{"type":"enabled"}` without a budget, the wire
    // body must omit the `budget_tokens` key entirely, leave `max_tokens`
    // at the model's `max_output_tokens`, and emit no compatibility
    // warning. (DeepSeek's anthropic-compat endpoint is the motivating
    // case — its `ModelInfo` declares no budget and its server doesn't
    // require one.)
    let model = AnthropicMessagesLanguageModel::new("claude-sonnet-4-5", make_config());

    let mut anthropic_opts: HashMap<String, serde_json::Value> = HashMap::new();
    anthropic_opts.insert(
        "thinking".into(),
        json!({"type": "enabled"}), // no budget_tokens
    );
    let mut provider_opts = HashMap::new();
    provider_opts.insert("anthropic".into(), anthropic_opts);

    let options = LanguageModelV4CallOptions::new(vec![
        vercel_ai_provider::LanguageModelV4Message::user_text("Hello"),
    ])
    .with_provider_options(vercel_ai_provider::ProviderOptions(provider_opts));

    let (body, _headers, warnings) = model
        .get_args(&options, false)
        .unwrap_or_else(|e| panic!("{e}"));

    assert_eq!(
        body["thinking"],
        json!({"type": "enabled"}),
        "thinking object must be exactly {{type: enabled}} — no synthesized budget_tokens",
    );
    assert!(
        body["thinking"].get("budget_tokens").is_none(),
        "budget_tokens key must NOT appear when ModelInfo did not supply one",
    );
    // `claude-sonnet-4-5` capabilities → max_output_tokens = 64_000.
    assert_eq!(
        body["max_tokens"],
        json!(64_000),
        "max_tokens must equal model's max_output_tokens — no synthetic bump",
    );
    assert!(
        !warnings.iter().any(|w| matches!(
            w,
            Warning::Compatibility { feature, .. } if feature == "extended thinking"
        )),
        "no Compatibility warning expected when budget is intentionally absent, got: {warnings:?}",
    );
}

// ---------------------------------------------------------------------------
// Thinking disabled — wire body actively carries the off toggle
// ---------------------------------------------------------------------------

#[test]
fn thinking_disabled_writes_thinking_disabled_to_body() {
    // `ThinkingConfig::Disabled` was previously parsed but silently
    // dropped — `is_thinking` was false so no `body["thinking"]`
    // write fired. The body builder now writes
    // `body["thinking"] = {"type":"disabled"}` explicitly so the
    // server doesn't fall back to its on-by-default behavior. Does
    // not trigger temperature/topK/topP suppression (those only
    // collide with Enabled or Adaptive).
    let model = AnthropicMessagesLanguageModel::new("claude-sonnet-4-5", make_config());

    let mut anthropic_opts: HashMap<String, serde_json::Value> = HashMap::new();
    anthropic_opts.insert("thinking".into(), json!({"type": "disabled"}));
    let mut provider_opts = HashMap::new();
    provider_opts.insert("anthropic".into(), anthropic_opts);

    let options = LanguageModelV4CallOptions::new(vec![
        vercel_ai_provider::LanguageModelV4Message::user_text("Hello"),
    ])
    .with_provider_options(vercel_ai_provider::ProviderOptions(provider_opts));

    let (body, _headers, warnings) = model
        .get_args(&options, false)
        .unwrap_or_else(|e| panic!("{e}"));

    assert_eq!(
        body["thinking"],
        json!({"type": "disabled"}),
        "body must carry the explicit-off toggle verbatim",
    );
    // `claude-sonnet-4-5` capabilities → max_output_tokens = 64_000.
    assert_eq!(
        body["max_tokens"],
        json!(64_000),
        "max_tokens must equal model's max_output_tokens — no thinking-budget bump",
    );
    // No suppression warnings — Disabled isn't "thinking" in the
    // temperature-collision sense.
    assert!(
        !warnings.iter().any(|w| matches!(
            w,
            Warning::Unsupported { feature, .. } if feature == "temperature" || feature == "topK" || feature == "topP"
        )),
        "no temperature/topK/topP warnings expected for Disabled, got: {warnings:?}",
    );
}

// ---------------------------------------------------------------------------
// context_management camelCase → snake_case transform
// ---------------------------------------------------------------------------

#[test]
fn context_management_transforms_camel_to_snake() {
    let caps = crate::anthropic_config::AnthropicModelCapabilities {
        context_management: true,
        ..Default::default()
    };
    let model =
        AnthropicMessagesLanguageModel::new("claude-sonnet-4-5", make_config_with_caps(caps));

    let mut anthropic_opts: HashMap<String, serde_json::Value> = HashMap::new();
    anthropic_opts.insert(
        "contextManagement".into(),
        json!({
            "edits": [
                {
                    "type": "clear_tool_uses_20250919",
                    "trigger": "auto",
                    "keep": 5,
                    "clearAtLeast": 3,
                    "clearToolInputs": true,
                    "excludeTools": ["tool_a"]
                },
                {
                    "type": "clear_thinking_20251015",
                    "keep": 2
                },
                {
                    "type": "compact_20260112",
                    "trigger": "auto",
                    "pauseAfterCompaction": true,
                    "instructions": "summarize"
                }
            ]
        }),
    );
    let mut provider_opts = HashMap::new();
    provider_opts.insert("anthropic".into(), anthropic_opts);

    let options = LanguageModelV4CallOptions::new(vec![
        vercel_ai_provider::LanguageModelV4Message::user_text("Hello"),
    ])
    .with_provider_options(vercel_ai_provider::ProviderOptions(provider_opts));

    let (body, _headers, _warnings) = model
        .get_args(&options, false)
        .unwrap_or_else(|e| panic!("{e}"));

    let ctx = &body["context_management"];
    let edits = ctx["edits"].as_array().expect("edits should be array");
    assert_eq!(edits.len(), 3);

    // clear_tool_uses_20250919: camelCase → snake_case
    let edit0 = &edits[0];
    assert_eq!(edit0["type"], "clear_tool_uses_20250919");
    assert_eq!(edit0["clear_at_least"], 3);
    assert_eq!(edit0["clear_tool_inputs"], true);
    assert_eq!(edit0["exclude_tools"], json!(["tool_a"]));
    // Should NOT have camelCase keys
    assert!(edit0.get("clearAtLeast").is_none());
    assert!(edit0.get("clearToolInputs").is_none());
    assert!(edit0.get("excludeTools").is_none());

    // compact_20260112: pauseAfterCompaction → pause_after_compaction
    let edit2 = &edits[2];
    assert_eq!(edit2["type"], "compact_20260112");
    assert_eq!(edit2["pause_after_compaction"], true);
    assert!(edit2.get("pauseAfterCompaction").is_none());
}

#[test]
fn context_management_unknown_strategy_warns() {
    let caps = crate::anthropic_config::AnthropicModelCapabilities {
        context_management: true,
        ..Default::default()
    };
    let model =
        AnthropicMessagesLanguageModel::new("claude-sonnet-4-5", make_config_with_caps(caps));

    let mut anthropic_opts: HashMap<String, serde_json::Value> = HashMap::new();
    anthropic_opts.insert(
        "contextManagement".into(),
        json!({
            "edits": [
                {"type": "unknown_strategy_20991231"}
            ]
        }),
    );
    let mut provider_opts = HashMap::new();
    provider_opts.insert("anthropic".into(), anthropic_opts);

    let options = LanguageModelV4CallOptions::new(vec![
        vercel_ai_provider::LanguageModelV4Message::user_text("Hello"),
    ])
    .with_provider_options(vercel_ai_provider::ProviderOptions(provider_opts));

    let (body, _headers, warnings) = model
        .get_args(&options, false)
        .unwrap_or_else(|e| panic!("{e}"));

    // Unknown strategy should be filtered out
    let edits = body["context_management"]["edits"]
        .as_array()
        .expect("edits should be array");
    assert_eq!(edits.len(), 0);

    // Should have warning
    assert!(
        warnings.iter().any(|w| matches!(
            w,
            Warning::Other { message } if message.contains("Unknown context management strategy")
        )),
        "expected unknown strategy warning, got: {warnings:?}",
    );
}

// ---------------------------------------------------------------------------
// Prompt-cache E2E (Phase 1): user passes cache_strategy via provider_options;
// verify the wire body carries (a) the auto-placed marker on the last user
// message block, (b) the deterministic sorted beta-header join, and (c) that
// internal-only signals (cacheStrategy / requestedBetas / agenticQuery /
// querySource) never appear in the wire body.
// ---------------------------------------------------------------------------

#[test]
fn prompt_cache_e2e_auto_marker_attached_to_last_user_block() {
    let caps = crate::anthropic_config::AnthropicModelCapabilities {
        prompt_cache: true,
        ..Default::default()
    };
    let model =
        AnthropicMessagesLanguageModel::new("claude-sonnet-4-5", make_config_with_caps(caps));

    let mut anthropic_opts: HashMap<String, serde_json::Value> = HashMap::new();
    anthropic_opts.insert(
        "cacheStrategy".into(),
        json!({
            "mode": "auto",
            "ttl": "five_minutes",
            "scope": null,
            "skipCacheWrite": false,
        }),
    );
    anthropic_opts.insert("agenticQuery".into(), json!(true));
    anthropic_opts.insert("querySource".into(), json!("main"));

    let mut provider_opts = HashMap::new();
    provider_opts.insert("anthropic".into(), anthropic_opts);

    let options = LanguageModelV4CallOptions::new(vec![
        vercel_ai_provider::LanguageModelV4Message::user_text("Hello cache"),
    ])
    .with_provider_options(vercel_ai_provider::ProviderOptions(provider_opts));

    let (body, headers, _warnings) = model
        .get_args(&options, false)
        .unwrap_or_else(|e| panic!("{e}"));

    // (a) marker placed on last block of last user message.
    let messages = body["messages"].as_array().expect("messages array");
    let last_msg = messages.last().expect("non-empty");
    assert_eq!(last_msg["role"], "user");
    let last_content = last_msg["content"].as_array().expect("user content array");
    let last_block = last_content.last().expect("non-empty content");
    assert_eq!(last_block["cache_control"], json!({"type": "ephemeral"}));

    // (b) baseline beta present.
    let betas = headers
        .get("anthropic-beta")
        .expect("anthropic-beta header set");
    assert!(
        betas.contains("claude-code-20250219"),
        "agentic baseline missing: {betas}"
    );

    // (c) internal signals stripped from wire body.
    assert!(
        body.get("cacheStrategy").is_none(),
        "cacheStrategy leaked into wire body"
    );
    assert!(
        body.get("requestedBetas").is_none(),
        "requestedBetas leaked into wire body"
    );
    assert!(
        body.get("agenticQuery").is_none(),
        "agenticQuery leaked into wire body"
    );
    assert!(
        body.get("querySource").is_none(),
        "querySource leaked into wire body"
    );
}

#[test]
fn prompt_cache_disabled_strategy_attaches_no_marker() {
    let caps = crate::anthropic_config::AnthropicModelCapabilities {
        prompt_cache: true,
        ..Default::default()
    };
    let model =
        AnthropicMessagesLanguageModel::new("claude-sonnet-4-5", make_config_with_caps(caps));

    let mut anthropic_opts: HashMap<String, serde_json::Value> = HashMap::new();
    anthropic_opts.insert(
        "cacheStrategy".into(),
        json!({
            "mode": "disabled",
            "ttl": "five_minutes",
        }),
    );
    let mut provider_opts = HashMap::new();
    provider_opts.insert("anthropic".into(), anthropic_opts);

    let options = LanguageModelV4CallOptions::new(vec![
        vercel_ai_provider::LanguageModelV4Message::user_text("Hello cache"),
    ])
    .with_provider_options(vercel_ai_provider::ProviderOptions(provider_opts));

    let (body, _headers, _warnings) = model
        .get_args(&options, false)
        .unwrap_or_else(|e| panic!("{e}"));

    let messages = body["messages"].as_array().expect("messages array");
    let last_block = messages
        .last()
        .and_then(|m| m["content"].as_array())
        .and_then(|c| c.last())
        .expect("non-empty content");
    assert!(
        last_block.get("cache_control").is_none(),
        "Disabled strategy must not attach a cache marker"
    );
}

#[test]
fn prompt_cache_one_hour_downgraded_for_unallowlisted_query_source() {
    let caps = crate::anthropic_config::AnthropicModelCapabilities {
        prompt_cache: true,
        ..Default::default()
    };
    // Empty allowlist: any query_source falls through to 5m.
    let model =
        AnthropicMessagesLanguageModel::new("claude-sonnet-4-5", make_config_with_caps(caps));

    let mut anthropic_opts: HashMap<String, serde_json::Value> = HashMap::new();
    anthropic_opts.insert(
        "cacheStrategy".into(),
        json!({
            "mode": "auto",
            "ttl": "one_hour",
        }),
    );
    anthropic_opts.insert("querySource".into(), json!("compaction"));
    let mut provider_opts = HashMap::new();
    provider_opts.insert("anthropic".into(), anthropic_opts);

    let options = LanguageModelV4CallOptions::new(vec![
        vercel_ai_provider::LanguageModelV4Message::user_text("Hi"),
    ])
    .with_provider_options(vercel_ai_provider::ProviderOptions(provider_opts));

    let (body, _headers, _warnings) = model
        .get_args(&options, false)
        .unwrap_or_else(|e| panic!("{e}"));

    let messages = body["messages"].as_array().expect("messages array");
    let last_block = messages
        .last()
        .and_then(|m| m["content"].as_array())
        .and_then(|c| c.last())
        .expect("non-empty content");
    // 1h request without allowlist match → 5m wire (no `ttl` field).
    assert_eq!(last_block["cache_control"], json!({"type": "ephemeral"}));
}

#[test]
fn prompt_cache_beta_header_join_is_deterministic_sorted() {
    let caps = crate::anthropic_config::AnthropicModelCapabilities {
        prompt_cache: true,
        context_1m: true,
        interleaved_thinking: true,
        ..Default::default()
    };
    let model =
        AnthropicMessagesLanguageModel::new("claude-sonnet-4-5", make_config_with_caps(caps));

    let options = LanguageModelV4CallOptions::new(vec![
        vercel_ai_provider::LanguageModelV4Message::user_text("Hi"),
    ]);

    // Run get_args twice; header value MUST be byte-identical.
    let (_b1, h1, _w1) = model
        .get_args(&options, false)
        .unwrap_or_else(|e| panic!("{e}"));
    let (_b2, h2, _w2) = model
        .get_args(&options, false)
        .unwrap_or_else(|e| panic!("{e}"));

    let v1 = h1.get("anthropic-beta").expect("h1");
    let v2 = h2.get("anthropic-beta").expect("h2");
    assert_eq!(v1, v2, "beta header must be byte-stable across runs");

    // Sorted check: split, compare against sorted copy.
    let parts: Vec<&str> = v1.split(',').collect();
    let mut sorted = parts.clone();
    sorted.sort_unstable();
    assert_eq!(parts, sorted, "betas must be sorted before join");
}

// ─── parallel_tool_calls translation ─────────────────────────────
//
// Anthropic uses inverted polarity AND nests the flag inside
// `tool_choice`, NOT at the request body root. The provider crate
// reads the generic `options.parallel_tool_calls` toggle, inverts it,
// and threads it through `prepare_anthropic_tools` which folds it
// into `tool_choice`. `prepare_anthropic_tools` only writes the key
// when `disable == true` (matches the server-side default of "parallel
// enabled"), so `parallel_tool_calls = Some(true)` is wire-silent.

fn echo_tool() -> vercel_ai_provider::LanguageModelV4Tool {
    vercel_ai_provider::LanguageModelV4Tool::Function(
        vercel_ai_provider::language_model::v4::LanguageModelV4FunctionTool {
            name: "echo".into(),
            description: Some("Echo input back".into()),
            input_schema: serde_json::json!({"type": "object", "properties": {}}),
            input_examples: None,
            strict: None,
            provider_options: None,
        },
    )
}

#[test]
fn parallel_tool_calls_false_writes_disable_into_tool_choice() {
    let model = AnthropicMessagesLanguageModel::new("claude-sonnet-4-5", make_config());
    let mut options = LanguageModelV4CallOptions::new(vec![
        vercel_ai_provider::LanguageModelV4Message::user_text("hi"),
    ]);
    options.tools = Some(vec![echo_tool()]);
    options.tool_choice = Some(vercel_ai_provider::LanguageModelV4ToolChoice::Auto);
    options.parallel_tool_calls = Some(false);

    let (body, _, _) = model
        .get_args(&options, false)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        body["tool_choice"]["type"], "auto",
        "tool_choice must remain `auto`"
    );
    assert_eq!(
        body["tool_choice"]["disable_parallel_tool_use"], true,
        "Generic `parallel_tool_calls = false` must invert into nested \
         `tool_choice.disable_parallel_tool_use = true` per Anthropic API contract"
    );
    assert!(
        body.get("disable_parallel_tool_use").is_none(),
        "Anthropic API rejects root-level `disable_parallel_tool_use`; \
         must be nested in tool_choice"
    );
}

#[test]
fn parallel_tool_calls_true_omits_disable_key() {
    // disable=false matches Anthropic server default → prepare_tools
    // intentionally skips emitting the key (wire-silent).
    let model = AnthropicMessagesLanguageModel::new("claude-sonnet-4-5", make_config());
    let mut options = LanguageModelV4CallOptions::new(vec![
        vercel_ai_provider::LanguageModelV4Message::user_text("hi"),
    ]);
    options.tools = Some(vec![echo_tool()]);
    options.tool_choice = Some(vercel_ai_provider::LanguageModelV4ToolChoice::Auto);
    options.parallel_tool_calls = Some(true);

    let (body, _, _) = model
        .get_args(&options, false)
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(
        body["tool_choice"]
            .get("disable_parallel_tool_use")
            .is_none(),
        "disable=false matches Anthropic default; the key must NOT appear on the wire"
    );
}

#[test]
fn parallel_tool_calls_typed_provider_option_wins_over_generic() {
    let model = AnthropicMessagesLanguageModel::new("claude-sonnet-4-5", make_config());
    let mut po = vercel_ai_provider::ProviderOptions::default();
    let mut inner = HashMap::new();
    inner.insert(
        "disableParallelToolUse".to_string(),
        serde_json::Value::Bool(true),
    );
    po.set("anthropic", inner);

    let mut options = LanguageModelV4CallOptions::new(vec![
        vercel_ai_provider::LanguageModelV4Message::user_text("hi"),
    ]);
    options.tools = Some(vec![echo_tool()]);
    options.tool_choice = Some(vercel_ai_provider::LanguageModelV4ToolChoice::Auto);
    options.provider_options = Some(po);
    options.parallel_tool_calls = Some(true); // would map to disable=false

    let (body, _, _) = model
        .get_args(&options, false)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        body["tool_choice"]["disable_parallel_tool_use"], true,
        "Typed provider_options.disableParallelToolUse must win over the generic toggle"
    );
}
