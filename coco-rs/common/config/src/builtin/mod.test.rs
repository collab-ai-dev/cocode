use coco_types::Capability;
use coco_types::ProviderApi;
use coco_types::ReasoningEffort;
use pretty_assertions::assert_eq;

use super::*;

// ---------- Provider catalog ----------

#[test]
fn builtin_provider_count() {
    let providers = builtin_providers().expect("builtin partials must resolve");
    assert_eq!(providers.len(), 9);
}

#[test]
fn builtin_providers_satisfy_identity_invariant() {
    // `name == map_key`. For builtins the "map key" is the partial's
    // slot identifier returned by `builtin_provider_partials`. Pair
    // them up and assert each resolved entry's `name` matches.
    let pairs = builtin_provider_partials();
    let resolved = builtin_providers().expect("builtin partials must resolve");
    assert_eq!(pairs.len(), resolved.len(), "builtin pair count mismatch");
    for ((slot_key, _), cfg) in pairs.iter().zip(resolved.iter()) {
        assert_eq!(
            *slot_key, cfg.name,
            "builtin entry {slot_key} name diverged after from_partial"
        );
    }
}

#[test]
fn builtin_anthropic_resolves_with_canonical_env_key() {
    let providers = builtin_providers().expect("builtin partials must resolve");
    let provider = providers
        .iter()
        .find(|p| p.api == ProviderApi::Anthropic && p.name == "anthropic")
        .expect("anthropic builtin");
    assert_eq!(provider.env_key, "ANTHROPIC_API_KEY");
}

#[test]
fn builtin_openai_resolves_with_canonical_env_key() {
    let providers = builtin_providers().expect("builtin partials must resolve");
    let provider = providers
        .iter()
        .find(|p| p.name == "openai")
        .expect("openai builtin");
    assert_eq!(provider.api, ProviderApi::Openai);
    assert_eq!(provider.env_key, "OPENAI_API_KEY");
    assert_eq!(provider.auth, crate::provider::ProviderAuth::ApiKey);
}

#[test]
fn builtin_openai_chatgpt_resolves_oauth_with_empty_env_key() {
    // The OAuth subscription instance must resolve through `from_partial`
    // despite omitting `env_key` (the relaxation), and carry the codex base.
    let providers = builtin_providers().expect("builtin partials must resolve");
    let provider = providers
        .iter()
        .find(|p| p.name == "openai-chatgpt")
        .expect("openai-chatgpt builtin");
    assert_eq!(provider.api, ProviderApi::Openai);
    assert_eq!(provider.env_key, "");
    assert_eq!(provider.base_url, "https://chatgpt.com/backend-api/codex");
    assert_eq!(
        provider.auth,
        crate::provider::ProviderAuth::OAuth {
            flow: coco_types::OAuthFlowId::OpenAiChatGpt
        }
    );
}

#[test]
fn builtin_deepseek_providers_resolve() {
    let providers = builtin_providers().expect("builtin partials must resolve");

    let ds_openai = providers
        .iter()
        .find(|p| p.name == "deepseek-openai")
        .expect("deepseek-openai builtin");
    assert_eq!(ds_openai.env_key, "DEEPSEEK_API_KEY");
    assert_eq!(ds_openai.base_url, "https://api.deepseek.com/v1");
    assert_eq!(ds_openai.api, ProviderApi::OpenaiCompat);
    assert!(ds_openai.models.contains_key("deepseek-v4-flash"));
    assert!(ds_openai.models.contains_key("deepseek-v4-pro"));

    let ds_anthropic = providers
        .iter()
        .find(|p| p.name == "deepseek-anthropic")
        .expect("deepseek-anthropic builtin");
    assert_eq!(ds_anthropic.env_key, "DEEPSEEK_API_KEY");
    assert_eq!(
        ds_anthropic.base_url,
        "https://api.deepseek.com/anthropic/v1"
    );
    assert_eq!(ds_anthropic.api, ProviderApi::Anthropic);
    assert!(ds_anthropic.models.contains_key("deepseek-v4-flash"));
    assert!(ds_anthropic.models.contains_key("deepseek-v4-pro"));
}

// ---------- Model catalog ----------

#[test]
fn builtin_gpt_and_gemini_models_have_base_instructions() {
    let builtin = builtin_models_partial();
    for model_id in [
        "gpt-5-4",
        "gpt-5-5",
        "gpt-5-3-codex",
        "gemini-3.1-pro-preview",
    ] {
        let instructions = builtin
            .get(model_id)
            .and_then(|info| info.base_instructions.as_deref())
            .expect("builtin base instructions");
        assert!(
            !instructions.trim().is_empty(),
            "{model_id} must have non-empty base instructions"
        );
    }
    assert!(
        builtin["gpt-5-4"]
            .base_instructions
            .as_deref()
            .unwrap()
            .starts_with("You are Coco"),
        "gpt prompt should carry the unified Coco identity"
    );
    assert!(
        builtin["gemini-3.1-pro-preview"]
            .base_instructions
            .as_deref()
            .unwrap()
            .starts_with(
                "You are Coco, an interactive CLI agent specializing in software engineering tasks."
            ),
        "gemini prompt should carry the unified Coco identity"
    );
}

#[test]
fn builtin_claude_models_declare_prompt_cache_capability() {
    let builtin = builtin_models_partial();
    for model_id in ["claude-sonnet-4-6", "claude-opus-4-7", "claude-haiku-4-5"] {
        let caps = builtin
            .get(model_id)
            .and_then(|info| info.capabilities.as_ref())
            .unwrap_or_else(|| panic!("{model_id} must seed capabilities"));
        assert!(
            caps.contains(&Capability::PromptCache),
            "{model_id} must declare PromptCache capability"
        );
        assert!(
            caps.contains(&Capability::ContextManagement),
            "{model_id} must declare ContextManagement capability"
        );
    }
}

#[test]
fn builtin_claude_sonnet_declares_context1m_and_isp() {
    let builtin = builtin_models_partial();
    let caps = builtin["claude-sonnet-4-6"].capabilities.as_ref().unwrap();
    assert!(caps.contains(&Capability::Context1m));
    assert!(caps.contains(&Capability::InterleavedThinking));
}

#[test]
fn builtin_claude_opus_declares_isp_but_not_context1m() {
    let builtin = builtin_models_partial();
    let caps = builtin["claude-opus-4-7"].capabilities.as_ref().unwrap();
    assert!(caps.contains(&Capability::InterleavedThinking));
    assert!(!caps.contains(&Capability::Context1m));
}

#[test]
fn builtin_claude_haiku_does_not_declare_isp_or_context1m() {
    // Haiku is the small/fast helper model: no interleaved thinking, no 1M ctx.
    let builtin = builtin_models_partial();
    let caps = builtin["claude-haiku-4-5"].capabilities.as_ref().unwrap();
    assert!(!caps.contains(&Capability::InterleavedThinking));
    assert!(!caps.contains(&Capability::Context1m));
}

#[test]
fn builtin_claude_sonnet_opus_declare_server_side_tool_reference() {
    // `tool-search-tool-2025-10-19` beta is shipped on Claude
    // Sonnet 4.5+/Opus 4+; Haiku ships without it (TS
    // `DEFAULT_UNSUPPORTED_MODEL_PATTERNS=['haiku']`).
    let builtin = builtin_models_partial();

    let sonnet_caps = builtin["claude-sonnet-4-6"].capabilities.as_ref().unwrap();
    assert!(sonnet_caps.contains(&Capability::ServerSideToolReference));

    let opus_caps = builtin["claude-opus-4-7"].capabilities.as_ref().unwrap();
    assert!(opus_caps.contains(&Capability::ServerSideToolReference));

    let haiku_caps = builtin["claude-haiku-4-5"].capabilities.as_ref().unwrap();
    assert!(!haiku_caps.contains(&Capability::ServerSideToolReference));
}

#[test]
fn every_builtin_model_declares_client_side_tool_search() {
    // The client-side `discovered_tool_names` promotion path is the
    // universal fallback — every built-in model is validated against
    // it (TS has no analogue: TS only supports the server-side path
    // and blacklists incompatible models). Custom models added via
    // `~/.coco/models.json` without this capability degrade to
    // eager-load (safe default; ToolSearch hidden).
    let builtin = builtin_models_partial();
    for (model_id, info) in builtin.iter() {
        let caps = info
            .capabilities
            .as_ref()
            .unwrap_or_else(|| panic!("{model_id} must seed capabilities"));
        assert!(
            caps.contains(&Capability::ClientSideToolSearch),
            "{model_id} must declare ClientSideToolSearch (validated client-side path)"
        );
    }
}

#[test]
fn builtin_claude_models_declare_explicit_thinking_budgets() {
    // After dropping the wire-builder `budget_tokens = 1024` fallback in
    // `vercel-ai-anthropic`, ModelInfo is the single source of truth for
    // budget. Anthropic first-party rejects `thinking.type=enabled`
    // without `budget_tokens`, so every Claude builtin level must declare
    // an explicit budget here. Values are aligned with
    // `vercel-ai-provider-utils::map_reasoning_to_provider_budget`
    // defaults at 64k max_output (Low 10% / Medium 30% / High 60%; XHigh
    // pinned to the model's 128k headroom).
    let builtin = builtin_models_partial();
    let expected_budgets = [
        (ReasoningEffort::Low, 6_400_i32),
        (ReasoningEffort::Medium, 19_200),
        (ReasoningEffort::High, 38_400),
        (ReasoningEffort::XHigh, 128_000),
    ];
    for model_id in ["claude-sonnet-4-6", "claude-opus-4-7"] {
        let info = builtin.get(model_id).expect(model_id);
        let levels = info
            .supported_thinking_levels
            .as_ref()
            .unwrap_or_else(|| panic!("{model_id} must seed thinking levels"));
        assert_eq!(
            levels.len(),
            expected_budgets.len(),
            "{model_id} thinking-level count drifted from expected matrix",
        );
        for (level, (expected_effort, expected_budget)) in levels.iter().zip(expected_budgets) {
            assert_eq!(
                level.effort, expected_effort,
                "{model_id} effort order drifted",
            );
            assert_eq!(
                level.budget_tokens,
                Some(expected_budget),
                "{model_id} {expected_effort:?} must declare explicit budget",
            );
        }
    }
}

#[test]
fn builtin_deepseek_v4_declares_three_thinking_levels() {
    let builtin = builtin_models_partial();
    for model_id in ["deepseek-v4-flash", "deepseek-v4-pro"] {
        let info = builtin.get(model_id).expect(model_id);

        // Capability gate.
        let caps = info.capabilities.as_ref().expect("capabilities");
        assert!(
            caps.contains(&Capability::ExtendedThinking),
            "{model_id} must declare ExtendedThinking"
        );

        // Default = Medium (UX "high"). The default-in-supported
        // invariant enforced by `ModelInfo::from_partial` requires the
        // default to match an entry in `supported_thinking_levels`,
        // so Medium is picked from the three explicit states.
        assert_eq!(
            info.default_thinking_level,
            Some(ReasoningEffort::Medium),
            "{model_id} default thinking level must be Medium"
        );

        // Surface: 3 explicit levels [Disable, Medium, XHigh] in that order.
        let levels = info
            .supported_thinking_levels
            .as_ref()
            .expect("thinking levels");
        assert_eq!(levels.len(), 3, "{model_id} must expose 3 thinking levels");
        assert_eq!(levels[0].effort, ReasoningEffort::Off);
        assert_eq!(levels[1].effort, ReasoningEffort::Medium);
        assert_eq!(levels[2].effort, ReasoningEffort::XHigh);

        // Disable carries the explicit-off wire toggle.
        assert_eq!(
            levels[0].options.get("thinking"),
            Some(&serde_json::json!({"type": "disabled"})),
            "{model_id} Disable level must declare disabled toggle"
        );
        // Medium (UX "high") and XHigh (UX "max") carry the enabled toggle.
        assert_eq!(
            levels[1].options.get("thinking"),
            Some(&serde_json::json!({"type": "enabled"})),
            "{model_id} Medium level must declare enabled toggle"
        );
        assert_eq!(
            levels[2].options.get("thinking"),
            Some(&serde_json::json!({"type": "enabled"})),
            "{model_id} XHigh level must declare enabled toggle"
        );
    }
}

#[test]
fn builtin_gpt5_models_declare_full_picker_thinking_levels() {
    // GPT-5 family exposes the 5-rung picker
    // (disable / low / medium / high / xhigh).
    let builtin = builtin_models_partial();
    for model_id in ["gpt-5-4", "gpt-5-5", "gpt-5-3-codex"] {
        let info = builtin.get(model_id).expect(model_id);
        assert_eq!(
            info.default_thinking_level,
            Some(ReasoningEffort::High),
            "{model_id} default thinking level must be High"
        );
        let levels = info
            .supported_thinking_levels
            .as_ref()
            .expect("thinking levels");
        assert_eq!(
            levels.iter().map(|level| level.effort).collect::<Vec<_>>(),
            vec![
                ReasoningEffort::Off,
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
                ReasoningEffort::XHigh,
            ],
            "{model_id} picker thinking levels drifted"
        );
    }
}

#[test]
fn builtin_gemini_models_declare_three_thinking_levels() {
    // Gemini's thinking_config maps cleanly to low / medium / high
    // token budgets — no disable/xhigh rung in the upstream API.
    // `default_thinking_level = Medium` satisfies the default-in-
    // supported invariant enforced by `ModelInfo::from_partial`.
    let builtin = builtin_models_partial();
    let model_id = "gemini-3.1-pro-preview";
    let info = builtin.get(model_id).expect(model_id);
    assert_eq!(
        info.default_thinking_level,
        Some(ReasoningEffort::Medium),
        "{model_id} default thinking level must be Medium"
    );
    let levels = info
        .supported_thinking_levels
        .as_ref()
        .expect("thinking levels");
    assert_eq!(
        levels.iter().map(|level| level.effort).collect::<Vec<_>>(),
        vec![
            ReasoningEffort::Low,
            ReasoningEffort::Medium,
            ReasoningEffort::High,
        ],
        "{model_id} thinking levels drifted"
    );
}

#[test]
fn non_anthropic_builtin_models_do_not_declare_prompt_cache() {
    // Capability::PromptCache is Anthropic wire-shape specific; no GPT/Gemini
    // builtin should declare it (multi-provider isolation invariant).
    let builtin = builtin_models_partial();
    for model_id in [
        "gpt-5-4",
        "gpt-5-5",
        "gpt-5-3-codex",
        "gemini-3.1-pro-preview",
    ] {
        if let Some(caps) = builtin.get(model_id).and_then(|i| i.capabilities.as_ref()) {
            assert!(
                !caps.contains(&Capability::PromptCache),
                "{model_id} must NOT declare PromptCache (Anthropic-only wire shape)"
            );
        }
    }
}
