use super::*;
use coco_config::ModelInfo;
use coco_config::PartialModelInfo;
use coco_config::PositiveTokens;
use coco_types::CacheTtl;
use coco_types::Capability;
use coco_types::PromptCacheConfig;
use coco_types::PromptCacheMode;
use coco_types::ProviderApi;
use coco_types::ReasoningEffort;
use coco_types::ThinkingLevel;
use pretty_assertions::assert_eq;

fn info_with_defaults(extra_body: BTreeMap<String, serde_json::Value>) -> ModelInfo {
    let partial = PartialModelInfo {
        context_window: Some(PositiveTokens::new(200_000)),
        max_output_tokens: Some(PositiveTokens::new(64_000)),
        extra_body: if extra_body.is_empty() {
            None
        } else {
            Some(extra_body)
        },
        ..Default::default()
    };
    ModelInfo::from_partial("test-provider", "test-model", partial).unwrap()
}

#[test]
fn anthropic_renamed_instance_wraps_under_anthropic() {
    let mut extra = BTreeMap::new();
    extra.insert("store".into(), serde_json::Value::Bool(false));
    let info = info_with_defaults(extra);

    let call = build_call_options(
        &info,
        ProviderApi::Anthropic,
        "azure-east",
        &PerCallOverrides::default(),
        Vec::new(),
        None,
    );

    let po = call.provider_options.expect("provider_options set");
    assert_eq!(po.0.len(), 1, "exactly one outer namespace key");
    assert!(
        po.0.contains_key("anthropic"),
        "Anthropic SDK reads provider_options[anthropic] regardless of instance name; got keys: {:?}",
        po.0.keys().collect::<Vec<_>>()
    );
    assert!(
        !po.0.contains_key("azure-east"),
        "non-canonical namespace key would be silently dropped by the SDK"
    );
}

#[test]
fn openai_compat_uses_instance_name_as_namespace() {
    let mut extra = BTreeMap::new();
    extra.insert("foo".into(), serde_json::Value::Bool(true));
    let info = info_with_defaults(extra);

    let call = build_call_options(
        &info,
        ProviderApi::OpenaiCompat,
        "internal-router",
        &PerCallOverrides::default(),
        Vec::new(),
        None,
    );

    let po = call.provider_options.unwrap();
    assert!(po.0.contains_key("internal-router"));
}

#[test]
fn per_call_extra_body_wins_over_model_level() {
    let mut model_extra = BTreeMap::new();
    model_extra.insert("store".into(), serde_json::Value::Bool(true));
    let info = info_with_defaults(model_extra);

    let mut per_call = PerCallOverrides::default();
    per_call
        .extra_body
        .insert("store".into(), serde_json::Value::Bool(false));

    let call = build_call_options(
        &info,
        ProviderApi::Openai,
        "openai",
        &per_call,
        Vec::new(),
        None,
    );
    let inner = call
        .provider_options
        .as_ref()
        .unwrap()
        .get("openai")
        .unwrap();
    assert_eq!(
        inner.get("store").and_then(serde_json::Value::as_bool),
        Some(false)
    );
}

#[test]
fn max_output_tokens_uses_from_positive_tokens_no_cast() {
    let info = info_with_defaults(BTreeMap::new());
    let call = build_call_options(
        &info,
        ProviderApi::Anthropic,
        "anthropic",
        &PerCallOverrides::default(),
        Vec::new(),
        None,
    );
    assert_eq!(call.max_output_tokens, Some(64_000));
}

#[test]
fn no_extra_body_no_provider_options() {
    let info = info_with_defaults(BTreeMap::new());
    let call = build_call_options(
        &info,
        ProviderApi::Anthropic,
        "anthropic",
        &PerCallOverrides::default(),
        Vec::new(),
        None,
    );
    assert!(call.provider_options.is_none());
}

#[test]
fn explicit_per_call_disable_thinking_disables_default() {
    // Model has a default thinking level. Per-call sets thinking_level
    // to Some(effort=Disable) — this MUST disable thinking, not fall
    // through to the model default.
    let partial = PartialModelInfo {
        context_window: Some(PositiveTokens::new(200_000)),
        max_output_tokens: Some(PositiveTokens::new(64_000)),
        supported_thinking_levels: Some(vec![ThinkingLevel::medium()]),
        default_thinking_level: Some(ReasoningEffort::Medium),
        ..Default::default()
    };
    let info = ModelInfo::from_partial("test", "test", partial).unwrap();

    let per_call = PerCallOverrides {
        thinking_level: Some(ThinkingLevel::disable()),
        ..Default::default()
    };

    let call = build_call_options(
        &info,
        ProviderApi::Anthropic,
        "anthropic",
        &per_call,
        Vec::new(),
        None,
    );

    assert!(
        call.reasoning.is_none(),
        "explicit per-call Disable must keep reasoning unset"
    );
    // The Anthropic typed arm now writes
    // `provider_options["anthropic"]["thinking"] = {"type":"disabled"}`
    // so the wire body actively carries the explicit-off toggle (the
    // vercel-ai-anthropic body builder picks this up via the typed
    // `ThinkingConfig::Disabled` parse and writes `body["thinking"]`).
    let provider_options = call
        .provider_options
        .as_ref()
        .expect("Disable on Anthropic must populate provider_options with the disabled toggle");
    let anthropic_ns = provider_options
        .0
        .get("anthropic")
        .expect("anthropic namespace must be present");
    assert_eq!(
        anthropic_ns.get("thinking"),
        Some(&serde_json::json!({"type": "disabled"})),
        "Disable on Anthropic must surface as thinking: disabled in provider_options",
    );
}

#[test]
fn unset_per_call_falls_through_to_model_default_thinking() {
    let partial = PartialModelInfo {
        context_window: Some(PositiveTokens::new(200_000)),
        max_output_tokens: Some(PositiveTokens::new(64_000)),
        supported_thinking_levels: Some(vec![ThinkingLevel::medium()]),
        default_thinking_level: Some(ReasoningEffort::Medium),
        ..Default::default()
    };
    let info = ModelInfo::from_partial("test", "test", partial).unwrap();

    let call = build_call_options(
        &info,
        ProviderApi::Anthropic,
        "anthropic",
        &PerCallOverrides::default(),
        Vec::new(),
        None,
    );

    assert!(
        call.reasoning.is_some(),
        "unset per-call should fall through to model default"
    );
    let po = call.provider_options.expect("anthropic thinking emitted");
    let inner = po.get("anthropic").unwrap();
    assert!(inner.contains_key("thinking"));
}

/// Build a ModelInfo declaring an explicit thinking ladder topping out
/// at `high` (no `xhigh`) — the canonical "model that doesn't support max
/// effort" shape.
fn info_with_ladder_low_to_high() -> ModelInfo {
    let partial = PartialModelInfo {
        context_window: Some(PositiveTokens::new(200_000)),
        max_output_tokens: Some(PositiveTokens::new(64_000)),
        supported_thinking_levels: Some(vec![
            ThinkingLevel::low(),
            ThinkingLevel::medium(),
            ThinkingLevel::high(),
        ]),
        default_thinking_level: Some(ReasoningEffort::Medium),
        ..Default::default()
    };
    ModelInfo::from_partial("test", "test", partial).unwrap()
}

fn anthropic_wire_effort(call: &LanguageModelV4CallOptions) -> Option<String> {
    call.provider_options
        .as_ref()?
        .get("anthropic")?
        .get("output_config")
        .and_then(|oc| oc.get("effort"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

#[test]
fn per_call_xhigh_clamps_to_top_declared_level() {
    // Model declares [low, medium, high] — no xhigh. An explicit
    // `--effort max` (XHigh) override must clamp to the top declared level
    // (high), NOT pass through as wire `output_config.effort="max"` (which
    // 400s on models that reject it).
    let info = info_with_ladder_low_to_high();
    let per_call = PerCallOverrides {
        thinking_level: Some(ThinkingLevel::xhigh()),
        ..Default::default()
    };

    let call = build_call_options(
        &info,
        ProviderApi::Anthropic,
        "anthropic",
        &per_call,
        Vec::new(),
        None,
    );

    assert_eq!(
        call.reasoning,
        Some(ReasoningLevel::High),
        "XHigh over the model's ceiling must clamp to the top declared level (High)"
    );
    assert_eq!(
        anthropic_wire_effort(&call).as_deref(),
        Some("high"),
        "wire effort must be 'high', never 'max', for a model whose ladder tops at high"
    );
}

#[test]
fn per_call_exact_match_effort_is_preserved() {
    // An in-ladder request resolves to that exact declared level.
    let info = info_with_ladder_low_to_high();
    let per_call = PerCallOverrides {
        thinking_level: Some(ThinkingLevel::low()),
        ..Default::default()
    };

    let call = build_call_options(
        &info,
        ProviderApi::Anthropic,
        "anthropic",
        &per_call,
        Vec::new(),
        None,
    );

    assert_eq!(call.reasoning, Some(ReasoningLevel::Low));
    assert_eq!(anthropic_wire_effort(&call).as_deref(), Some("low"));
}

#[test]
fn per_call_no_declared_ladder_passes_effort_through() {
    // Model declares no thinking ladder (None) — typical for a custom 3P
    // model in ~/.coco/models.json. We cannot probe a ceiling, so
    // `resolve_thinking_level` trusts the caller and passes the requested
    // effort through verbatim (documented "trust the caller" contract).
    let info = info_with_defaults(BTreeMap::new());
    let per_call = PerCallOverrides {
        thinking_level: Some(ThinkingLevel::xhigh()),
        ..Default::default()
    };

    let call = build_call_options(
        &info,
        ProviderApi::Anthropic,
        "anthropic",
        &per_call,
        Vec::new(),
        None,
    );

    assert_eq!(
        call.reasoning,
        Some(ReasoningLevel::Xhigh),
        "no declared ladder → trust the caller, pass XHigh through"
    );
    assert_eq!(anthropic_wire_effort(&call).as_deref(), Some("max"));
}

#[test]
fn per_call_auto_is_not_clamped_to_nearest_numeric_level() {
    // CRITICAL edge case: Auto (and Off) are not numeric efforts — they
    // mean "let the provider default decide" / "off". They must bypass the
    // effort-distance clamp, which would otherwise snap Auto to the nearest
    // declared numeric level (High here) and force reasoning on.
    let partial = PartialModelInfo {
        context_window: Some(PositiveTokens::new(200_000)),
        max_output_tokens: Some(PositiveTokens::new(64_000)),
        supported_thinking_levels: Some(vec![ThinkingLevel::high()]),
        default_thinking_level: Some(ReasoningEffort::High),
        ..Default::default()
    };
    let info = ModelInfo::from_partial("test", "test", partial).unwrap();
    let per_call = PerCallOverrides {
        thinking_level: Some(ThinkingLevel::auto()),
        ..Default::default()
    };

    let call = build_call_options(
        &info,
        ProviderApi::Anthropic,
        "anthropic",
        &per_call,
        Vec::new(),
        None,
    );

    assert!(
        call.reasoning.is_none(),
        "Auto must not be clamped to a numeric level — reasoning stays unset"
    );
}

#[test]
fn extra_body_per_call_deep_merges_into_model_extra_body() {
    // model-level: reasoning has effort=low + summary=auto, plus a sibling key.
    let mut model_extra = BTreeMap::new();
    model_extra.insert(
        "reasoning".to_string(),
        serde_json::json!({ "effort": "low", "summary": "auto" }),
    );
    model_extra.insert("store".to_string(), serde_json::json!(true));

    let partial = PartialModelInfo {
        context_window: Some(PositiveTokens::new(200_000)),
        max_output_tokens: Some(PositiveTokens::new(64_000)),
        extra_body: Some(model_extra),
        ..Default::default()
    };
    let info = ModelInfo::from_partial("test", "test", partial).unwrap();

    // per-call: override reasoning.effort, add a new sibling.
    let mut per_call_extra = BTreeMap::new();
    per_call_extra.insert(
        "reasoning".to_string(),
        serde_json::json!({ "effort": "high" }),
    );
    per_call_extra.insert("metadata".to_string(), serde_json::json!({ "tag": "x" }));

    let per_call = PerCallOverrides {
        extra_body: per_call_extra,
        ..Default::default()
    };

    let call = build_call_options(
        &info,
        ProviderApi::Openai,
        "openai",
        &per_call,
        Vec::new(),
        None,
    );

    let po = call.provider_options.expect("provider options emitted");
    let inner = po.get("openai").expect("openai namespace");

    // reasoning.summary is preserved (would be lost under shallow merge).
    assert_eq!(
        inner.get("reasoning"),
        Some(&serde_json::json!({ "effort": "high", "summary": "auto" })),
        "per-call reasoning.effort overrides; summary survives from model-level"
    );
    // unrelated model-level key passes through.
    assert_eq!(inner.get("store"), Some(&serde_json::json!(true)));
    // per-call-only key is added.
    assert_eq!(
        inner.get("metadata"),
        Some(&serde_json::json!({ "tag": "x" }))
    );
}

#[test]
fn extra_body_merge_drops_prototype_polluting_keys() {
    // model-level: reasoning has a value at "safe" key.
    let mut model_extra = BTreeMap::new();
    model_extra.insert("safe".to_string(), serde_json::json!({ "ok": 0 }));
    let partial = PartialModelInfo {
        context_window: Some(PositiveTokens::new(200_000)),
        max_output_tokens: Some(PositiveTokens::new(64_000)),
        extra_body: Some(model_extra),
        ..Default::default()
    };
    let info = ModelInfo::from_partial("test", "test", partial).unwrap();

    // per-call carries a __proto__ key inside the nested object.
    let mut per_call_extra = BTreeMap::new();
    per_call_extra.insert(
        "safe".to_string(),
        serde_json::json!({ "__proto__": { "polluted": true }, "ok": 1 }),
    );
    let per_call = PerCallOverrides {
        extra_body: per_call_extra,
        ..Default::default()
    };

    let call = build_call_options(
        &info,
        ProviderApi::Openai,
        "openai",
        &per_call,
        Vec::new(),
        None,
    );

    let po = call.provider_options.expect("provider options emitted");
    let inner = po.get("openai").expect("openai namespace");
    let safe = inner.get("safe").expect("safe key").as_object().unwrap();
    assert!(
        !safe.contains_key("__proto__"),
        "prototype-polluting key must be filtered during deep merge"
    );
    assert_eq!(safe.get("ok"), Some(&serde_json::json!(1)));
}

#[test]
fn cache_strategy_per_call_writes_anthropic_namespace() {
    let info = info_with_defaults(BTreeMap::new());
    let per_call = PerCallOverrides {
        cache_strategy: Some(PromptCacheConfig {
            mode: PromptCacheMode::Auto,
            ttl: CacheTtl::OneHour,
            ..Default::default()
        }),
        agentic_query: true,
        query_source: Some("repl_main_thread".into()),
        ..Default::default()
    };
    let (call, merged) = build_call_options_with_extra(
        &info,
        ProviderApi::Anthropic,
        "anthropic",
        &per_call,
        Vec::new(),
        None,
    );
    let inner = call
        .provider_options
        .as_ref()
        .unwrap()
        .get("anthropic")
        .unwrap();
    assert_eq!(
        inner.get("cacheStrategy").and_then(|v| v.get("mode")),
        Some(&serde_json::json!("auto"))
    );
    assert_eq!(inner.get("agenticQuery"), Some(&serde_json::json!(true)));
    assert_eq!(
        inner.get("querySource"),
        Some(&serde_json::json!("repl_main_thread"))
    );
    // Merged map (pre-namespace-wrap) carries the same keys for detector hashing.
    assert!(merged.contains_key("cacheStrategy"));
    assert!(merged.contains_key("agenticQuery"));
    assert!(merged.contains_key("querySource"));
}

#[test]
fn cache_strategy_skipped_for_openai_namespace() {
    let info = info_with_defaults(BTreeMap::new());
    let per_call = PerCallOverrides {
        cache_strategy: Some(PromptCacheConfig {
            mode: PromptCacheMode::Auto,
            ..Default::default()
        }),
        agentic_query: true,
        query_source: Some("repl_main_thread".into()),
        ..Default::default()
    };
    let (call, merged) = build_call_options_with_extra(
        &info,
        ProviderApi::Openai,
        "openai",
        &per_call,
        Vec::new(),
        None,
    );
    // No prompt-cache keys in either the wire body or the merged map.
    assert!(
        call.provider_options.is_none() || {
            let inner = call
                .provider_options
                .as_ref()
                .unwrap()
                .get("openai")
                .unwrap();
            !inner.contains_key("cacheStrategy")
                && !inner.contains_key("agenticQuery")
                && !inner.contains_key("querySource")
        }
    );
    assert!(!merged.contains_key("cacheStrategy"));
    assert!(!merged.contains_key("agenticQuery"));
    assert!(!merged.contains_key("querySource"));
}

#[test]
fn merged_extra_returned_for_detector_input() {
    let mut model_extra = BTreeMap::new();
    model_extra.insert("store".into(), serde_json::Value::Bool(true));
    let info = info_with_defaults(model_extra);

    let mut per_call = PerCallOverrides::default();
    per_call
        .extra_body
        .insert("metadata".into(), serde_json::json!({ "tag": "x" }));
    per_call.cache_strategy = Some(PromptCacheConfig {
        mode: PromptCacheMode::Auto,
        ttl: CacheTtl::FiveMinutes,
        ..Default::default()
    });
    per_call.query_source = Some("compact".into());

    let (call, merged) = build_call_options_with_extra(
        &info,
        ProviderApi::Anthropic,
        "anthropic",
        &per_call,
        Vec::new(),
        None,
    );
    // Merged map sees every key the wire body sees, in canonical order.
    let inner = call
        .provider_options
        .as_ref()
        .unwrap()
        .get("anthropic")
        .unwrap();
    for key in inner.keys() {
        assert!(
            merged.contains_key(key),
            "merged_extra missing wire key {key}; cannot feed detector accurately"
        );
    }
    // And nothing else — merged is the post-merge, pre-wrap snapshot.
    assert_eq!(merged.len(), inner.len());
}

#[test]
fn cache_strategy_disabled_emits_no_session_context() {
    // Finding 4: query_source MUST NOT change the merged map when caching
    // is off. Otherwise extra_body_hash flips for callers that never opted in.
    let info = info_with_defaults(BTreeMap::new());
    let per_call = PerCallOverrides {
        // mode is Disabled by default
        cache_strategy: Some(PromptCacheConfig::default()),
        agentic_query: true,
        query_source: Some("repl_main_thread".into()),
        ..Default::default()
    };
    let (call, merged) = build_call_options_with_extra(
        &info,
        ProviderApi::Anthropic,
        "anthropic",
        &per_call,
        Vec::new(),
        None,
    );
    assert!(
        call.provider_options.is_none(),
        "no keys → no provider_options"
    );
    assert!(merged.is_empty());
}

#[test]
fn deepseek_disabled_thinking_emits_disabled_toggle() {
    // Per-call sets a Disable-effort thinking level that carries the
    // DeepSeek-style disabled wire toggle in `level.options`. Effort=Disable
    // keeps the typed reasoning lane unset (it's not an explicit level),
    // and the `level.options` payload flows into `extra_body` and wraps
    // into `provider_options`.
    let partial = PartialModelInfo {
        context_window: Some(PositiveTokens::new(1_000_000)),
        max_output_tokens: Some(PositiveTokens::new(12_288)),
        ..Default::default()
    };
    let info = ModelInfo::from_partial("deepseek-openai", "deepseek-v4-flash", partial).unwrap();

    let mut disabled = ThinkingLevel::disable();
    disabled
        .options
        .insert("thinking".into(), serde_json::json!({"type": "disabled"}));
    let per_call = PerCallOverrides {
        thinking_level: Some(disabled),
        ..Default::default()
    };

    let (call, merged) = build_call_options_with_extra(
        &info,
        ProviderApi::OpenaiCompat,
        "deepseek-openai",
        &per_call,
        Vec::new(),
        None,
    );

    // Lane A2: typed reasoning stays unset for Disable effort.
    assert!(
        call.reasoning.is_none(),
        "Disable effort must NOT set typed reasoning"
    );

    // Lane B: the wire toggle from `level.options` flows through.
    assert_eq!(
        merged.get("thinking"),
        Some(&serde_json::json!({"type": "disabled"})),
    );
    assert!(
        !merged.contains_key("reasoningEffort"),
        "OpenaiCompat default arm must not emit reasoningEffort for Disable effort"
    );

    // The merged map ends up under provider_options[<instance>].
    let po = call.provider_options.expect("provider_options set");
    let inner = po.get("deepseek-openai").expect("namespace");
    assert_eq!(
        inner.get("thinking"),
        Some(&serde_json::json!({"type": "disabled"})),
    );
}

// ─── Lane E: parallel tool calls capability gate ─────────────────
//
// After the refactor, Lane E sets a single provider-agnostic field
// (`call.parallel_tool_calls`) and provider crates own the wire
// translation. The inference layer therefore has no per-provider
// branches to test here — wire-emission tests live alongside each
// provider's language model impl (`vercel-ai-openai`,
// `vercel-ai-anthropic`, `vercel-ai-openai-compatible`).

fn info_with_capabilities(caps: Vec<Capability>) -> ModelInfo {
    let partial = PartialModelInfo {
        context_window: Some(PositiveTokens::new(200_000)),
        max_output_tokens: Some(PositiveTokens::new(64_000)),
        capabilities: Some(caps),
        ..Default::default()
    };
    ModelInfo::from_partial("test-provider", "test-model", partial).unwrap()
}

#[test]
fn parallel_tool_calls_capability_sets_generic_flag() {
    let info = info_with_capabilities(vec![Capability::ParallelToolCalls]);
    // Provider arm is irrelevant — Lane E doesn't branch on it
    // anymore. Use Openai as a representative call site.
    let (call, merged) = build_call_options_with_extra(
        &info,
        ProviderApi::Openai,
        "openai",
        &PerCallOverrides::default(),
        Vec::new(),
        None,
    );
    assert_eq!(
        call.parallel_tool_calls,
        Some(true),
        "Capability present ⇒ generic `parallel_tool_calls` toggle is set to Some(true)",
    );
    assert!(
        !merged.contains_key("parallelToolCalls")
            && !merged.contains_key("parallel_tool_calls")
            && !merged.contains_key("disableParallelToolUse"),
        "Lane E must NOT write per-provider wire keys into the merged extra map — \
         that translation belongs in the provider crates"
    );
}

#[test]
fn parallel_tool_calls_capability_flag_set_regardless_of_provider() {
    let info = info_with_capabilities(vec![Capability::ParallelToolCalls]);
    for (api, name) in [
        (ProviderApi::Openai, "openai"),
        (ProviderApi::Anthropic, "anthropic"),
        (ProviderApi::Gemini, "google"),
        (ProviderApi::OpenaiCompat, "deepseek-openai"),
        (ProviderApi::Volcengine, "volcengine"),
        (ProviderApi::Zai, "zai"),
    ] {
        let (call, _) = build_call_options_with_extra(
            &info,
            api,
            name,
            &PerCallOverrides::default(),
            Vec::new(),
            None,
        );
        assert_eq!(
            call.parallel_tool_calls,
            Some(true),
            "Capability ⇒ generic flag must be set independent of ProviderApi ({api:?})",
        );
    }
}

#[test]
fn parallel_tool_calls_absent_capability_leaves_flag_unset() {
    // Capabilities present but ParallelToolCalls NOT in the set.
    let info = info_with_capabilities(vec![Capability::TextGeneration, Capability::ToolCalling]);
    let (call, _) = build_call_options_with_extra(
        &info,
        ProviderApi::Openai,
        "openai",
        &PerCallOverrides::default(),
        Vec::new(),
        None,
    );
    assert_eq!(
        call.parallel_tool_calls, None,
        "Capability absent ⇒ generic flag stays None (provider default applies)"
    );
}

// ── config#247: fast-mode → Anthropic `speed=fast` wire translation ──

#[test]
fn fast_mode_writes_anthropic_speed_when_enabled() {
    let info = info_with_defaults(BTreeMap::new());
    let per_call = PerCallOverrides {
        fast_mode: true,
        ..Default::default()
    };
    let (_call, merged) = build_call_options_with_extra(
        &info,
        ProviderApi::Anthropic,
        "anthropic",
        &per_call,
        Vec::new(),
        None,
    );
    assert_eq!(
        merged.get("speed"),
        Some(&serde_json::Value::String("fast".into())),
        "anthropic + fast_mode must write speed=fast for the beta to fire"
    );
}

#[test]
fn fast_mode_off_writes_no_speed() {
    let info = info_with_defaults(BTreeMap::new());
    let (_call, merged) = build_call_options_with_extra(
        &info,
        ProviderApi::Anthropic,
        "anthropic",
        &PerCallOverrides::default(),
        Vec::new(),
        None,
    );
    assert!(!merged.contains_key("speed"));
}

#[test]
fn fast_mode_skipped_for_non_anthropic() {
    let info = info_with_defaults(BTreeMap::new());
    let per_call = PerCallOverrides {
        fast_mode: true,
        ..Default::default()
    };
    let (_call, merged) = build_call_options_with_extra(
        &info,
        ProviderApi::Openai,
        "openai",
        &per_call,
        Vec::new(),
        None,
    );
    assert!(
        !merged.contains_key("speed"),
        "non-Anthropic providers have no speed concept and ignore fast_mode"
    );
}
