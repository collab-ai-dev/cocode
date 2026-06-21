use super::*;
use coco_types::Capability;
use coco_types::ProviderApi;
use coco_types::ReasoningEffort;
use coco_types::ThinkingLevel;
use pretty_assertions::assert_eq;

/// Capability slice for tests modelling an adaptive-capable Anthropic
/// model (Claude Sonnet 4.6 / Opus 4.6 / DeepSeek-anthropic-compat).
const ADAPTIVE: &[Capability] = &[Capability::AdaptiveThinking];

/// Empty capability slice — models with no AdaptiveThinking declared
/// (Claude Sonnet 4.5, custom-registered third-party Anthropic-compat
/// without registry override, etc.). Auto on these MUST emit nothing.
const NO_CAPS: &[Capability] = &[];

#[test]
fn anthropic_thinking_uses_camelcase_budget_tokens() {
    let level = ThinkingLevel::with_budget(ReasoningEffort::High, 32_000);
    let out = to_extra_body(&level, ProviderApi::Anthropic, ADAPTIVE);
    let thinking = out.get("thinking").unwrap();
    assert_eq!(
        thinking.get("type").and_then(serde_json::Value::as_str),
        Some("enabled")
    );
    assert_eq!(
        thinking
            .get("budgetTokens")
            .and_then(serde_json::Value::as_i64),
        Some(32_000)
    );
    assert!(
        thinking.get("budget_tokens").is_none(),
        "snake_case key must not appear in extra_body output"
    );
    // The typed arm now also emits `output_config.effort` for explicit
    // levels (new Anthropic API surface). High → "high".
    assert_eq!(
        out.get("output_config")
            .and_then(|v| v.get("effort"))
            .and_then(serde_json::Value::as_str),
        Some("high"),
    );
}

#[test]
fn renamed_anthropic_instance_still_emits_anthropic_shape() {
    // Routing key is ProviderApi family, not ProviderConfig.name. A
    // user-renamed instance (e.g., "azure-east" backed by
    // ProviderApi::Anthropic) MUST still get the typed Anthropic wire
    // body, not the OpenAI-compat fallback.
    let level = ThinkingLevel::with_budget(ReasoningEffort::High, 12_000);
    let out = to_extra_body(&level, ProviderApi::Anthropic, ADAPTIVE);
    assert!(out.contains_key("thinking"));
    assert!(!out.contains_key("reasoningEffort"));
}

#[test]
fn openai_responses_uses_reasoning_summary_camelcase() {
    let level = ThinkingLevel::high();
    let out = to_extra_body(&level, ProviderApi::Openai, NO_CAPS);
    assert_eq!(
        out.get("reasoningSummary")
            .and_then(serde_json::Value::as_str),
        Some("auto")
    );
}

#[test]
fn google_thinking_config_nested_under_generation_config() {
    // Producer responsibility: emit wire-correct nesting. Gemini's
    // `thinkingConfig` lives INSIDE `generationConfig`, not at the
    // body root — the language model deep-merges this onto the body.
    //
    // Effort → wire-shape (Gemini-3 `thinkingLevel` vs Gemini-2.5
    // `thinkingBudget`) is version-aware and lives in the adapter
    // (`resolve_thinking_config`), so this convert layer only
    // contributes the `includeThoughts: true` signal that has no
    // typed channel. `level.budget_tokens` is NOT emitted here:
    // Gemini-3 dropped `thinkingBudget` support, and `level` already
    // flows through the typed `call.reasoning` channel for
    // version-aware mapping. Test passes a budget to lock the
    // contract — passing one must NOT cause us to emit
    // `thinkingBudget`.
    let level = ThinkingLevel::with_budget(ReasoningEffort::Medium, 8_000);
    let out = to_extra_body(&level, ProviderApi::Gemini, NO_CAPS);

    assert!(!out.contains_key("thinkingConfig"));

    let config = out
        .get("generationConfig")
        .and_then(|v| v.get("thinkingConfig"))
        .expect("generationConfig.thinkingConfig should be set");
    assert_eq!(
        config
            .get("includeThoughts")
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );
    // Critical: thinking budget is NOT emitted here — the adapter's
    // version-aware typed channel owns level/budget mapping.
    assert!(config.get("thinkingBudget").is_none());
    assert!(config.get("thinkingLevel").is_none());
}

#[test]
fn openai_compat_emits_reasoning_effort_camelcase() {
    let level = ThinkingLevel::high();
    let out = to_extra_body(&level, ProviderApi::OpenaiCompat, NO_CAPS);
    assert_eq!(
        out.get("reasoningEffort")
            .and_then(serde_json::Value::as_str),
        Some("high")
    );
}

#[test]
fn volcengine_uses_reasoning_effort_camelcase() {
    let level = ThinkingLevel::high();
    let out = to_extra_body(&level, ProviderApi::Volcengine, NO_CAPS);
    assert_eq!(
        out.get("reasoningEffort")
            .and_then(serde_json::Value::as_str),
        Some("high")
    );
}

#[test]
fn anthropic_arm_disable_emits_disabled_thinking_typed() {
    // Disable on the Anthropic arm now writes the explicit-off
    // toggle on the wire (vercel-ai-anthropic body builder picks it
    // up via the typed `ThinkingConfig::Disabled` parse). Capability
    // slice is irrelevant for Disable — the gate only affects Auto.
    let level = ThinkingLevel::disable();
    let out = to_extra_body(&level, ProviderApi::Anthropic, NO_CAPS);
    assert_eq!(
        out.get("thinking"),
        Some(&serde_json::json!({"type": "disabled"})),
        "Disable on Anthropic must emit {{type: disabled}}",
    );
    assert!(
        !out.contains_key("output_config"),
        "Disable must NOT emit output_config",
    );
}

#[test]
fn anthropic_arm_auto_emits_adaptive_thinking_only() {
    // Auto on Anthropic emits the adaptive variant — server picks
    // effort dynamically. Requires `Capability::AdaptiveThinking`.
    let level = ThinkingLevel::auto();
    let out = to_extra_body(&level, ProviderApi::Anthropic, ADAPTIVE);
    assert_eq!(
        out.get("thinking"),
        Some(&serde_json::json!({"type": "adaptive"})),
        "Auto on adaptive-capable Anthropic must emit {{type: adaptive}}",
    );
    assert!(
        !out.contains_key("output_config"),
        "Auto must NOT emit output_config — server picks effort",
    );
}

#[test]
fn anthropic_arm_minimal_maps_to_low_effort() {
    // No `Effort::Minimal` exists in Anthropic's enum, so Minimal
    // collapses to the closest neighbor (Low) for output_config and
    // emits the standard enabled thinking object.
    let mut level = ThinkingLevel::auto();
    level.effort = ReasoningEffort::Minimal;
    let out = to_extra_body(&level, ProviderApi::Anthropic, NO_CAPS);
    assert_eq!(
        out.get("thinking")
            .and_then(|v| v.get("type"))
            .and_then(serde_json::Value::as_str),
        Some("enabled"),
    );
    assert_eq!(
        out.get("output_config")
            .and_then(|v| v.get("effort"))
            .and_then(serde_json::Value::as_str),
        Some("low"),
        "Minimal must map to output_config.effort = \"low\"",
    );
}

#[test]
fn anthropic_arm_xhigh_maps_to_max_effort() {
    let mut level = ThinkingLevel::auto();
    level.effort = ReasoningEffort::XHigh;
    let out = to_extra_body(&level, ProviderApi::Anthropic, NO_CAPS);
    assert_eq!(
        out.get("output_config")
            .and_then(|v| v.get("effort"))
            .and_then(serde_json::Value::as_str),
        Some("max"),
        "XHigh must map to output_config.effort = \"max\" (Anthropic Effort::Max)",
    );
}

#[test]
fn anthropic_arm_medium_emits_thinking_and_output_config_medium() {
    // DeepSeek UX label "high" → coco Medium; wire effort string is
    // "medium" (matches Anthropic Effort enum, consistent with the
    // existing OpenaiCompat path which emits reasoning_effort:
    // "medium").
    let mut level = ThinkingLevel::auto();
    level.effort = ReasoningEffort::Medium;
    let out = to_extra_body(&level, ProviderApi::Anthropic, NO_CAPS);
    assert_eq!(
        out.get("thinking")
            .and_then(|v| v.get("type"))
            .and_then(serde_json::Value::as_str),
        Some("enabled"),
    );
    assert_eq!(
        out.get("output_config")
            .and_then(|v| v.get("effort"))
            .and_then(serde_json::Value::as_str),
        Some("medium"),
    );
}

#[test]
fn auto_returns_empty_map_when_no_options() {
    let level = ThinkingLevel::auto();
    let out = to_extra_body(&level, ProviderApi::OpenaiCompat, NO_CAPS);
    assert!(
        out.is_empty(),
        "Auto with empty options on OpenaiCompat means: provider decides; no fields emitted",
    );
}

#[test]
fn disable_returns_empty_map_on_openai_compat() {
    // OpenaiCompat path keeps the is_explicit_level gate — Disable
    // emits nothing typed (only level.options pass-through).
    let level = ThinkingLevel::disable();
    let out = to_extra_body(&level, ProviderApi::OpenaiCompat, NO_CAPS);
    assert!(out.is_empty());
}

#[test]
fn level_options_are_passed_through() {
    let mut level = ThinkingLevel::high();
    level
        .options
        .insert("interleaved".into(), serde_json::Value::Bool(true));
    let out = to_extra_body(&level, ProviderApi::Anthropic, ADAPTIVE);
    assert_eq!(
        out.get("interleaved").and_then(serde_json::Value::as_bool),
        Some(true)
    );
}

#[test]
fn disable_effort_passes_options_through() {
    // DeepSeek V4 declares `{"thinking":{"type":"disabled"}}` in
    // `level.options` for its `Disable` level. The convert layer must
    // pass options through even when effort is Disable so the wire
    // toggle reaches the body.
    let mut level = ThinkingLevel::disable();
    level
        .options
        .insert("thinking".into(), serde_json::json!({"type": "disabled"}));
    let out = to_extra_body(&level, ProviderApi::OpenaiCompat, NO_CAPS);
    assert_eq!(
        out.get("thinking"),
        Some(&serde_json::json!({"type": "disabled"})),
    );
    // No `reasoningEffort` for Disable — typed-arm emission is gated on
    // `is_explicit_level()`, which Disable / Auto fail.
    assert!(
        !out.contains_key("reasoningEffort"),
        "Disable level must not emit reasoningEffort"
    );
}

#[test]
fn deepseek_medium_emits_thinking_enabled_and_reasoning_effort() {
    // Mirrors the registry surface: Medium effort + options carrying
    // the wire-enabled toggle. Output must contain BOTH the
    // `thinking` toggle (from level.options) AND the `reasoningEffort`
    // (from the OpenaiCompat default arm via `Display`).
    let mut level = ThinkingLevel {
        effort: ReasoningEffort::Medium,
        budget_tokens: None,
        options: std::collections::HashMap::new(),
    };
    level
        .options
        .insert("thinking".into(), serde_json::json!({"type": "enabled"}));
    let out = to_extra_body(&level, ProviderApi::OpenaiCompat, NO_CAPS);
    assert_eq!(
        out.get("thinking"),
        Some(&serde_json::json!({"type": "enabled"})),
    );
    assert_eq!(
        out.get("reasoningEffort")
            .and_then(serde_json::Value::as_str),
        Some("medium"),
    );
}

#[test]
fn deepseek_xhigh_emits_thinking_enabled_and_xhigh_reasoning_effort() {
    let mut level = ThinkingLevel {
        effort: ReasoningEffort::XHigh,
        budget_tokens: None,
        options: std::collections::HashMap::new(),
    };
    level
        .options
        .insert("thinking".into(), serde_json::json!({"type": "enabled"}));
    let out = to_extra_body(&level, ProviderApi::OpenaiCompat, NO_CAPS);
    assert_eq!(
        out.get("thinking"),
        Some(&serde_json::json!({"type": "enabled"})),
    );
    assert_eq!(
        out.get("reasoningEffort")
            .and_then(serde_json::Value::as_str),
        Some("xhigh"),
    );
}

// ---------------------------------------------------------------------
// Exhaustive matrix: every ReasoningEffort × every ProviderApi.
//
// Locks the convert-layer contract. Adding a new `ReasoningEffort`
// variant or `ProviderApi` family forces this matrix to be updated —
// the inner `match level.effort` in `to_extra_body` is exhaustive, so
// missing a case becomes a compile error AND a missing test row here
// is caught at review time.
// ---------------------------------------------------------------------

fn level_with_effort(effort: ReasoningEffort) -> ThinkingLevel {
    ThinkingLevel {
        effort,
        budget_tokens: None,
        options: std::collections::HashMap::new(),
    }
}

#[test]
fn matrix_anthropic_disable_emits_disabled_thinking_no_output_config() {
    let out = to_extra_body(
        &level_with_effort(ReasoningEffort::Off),
        ProviderApi::Anthropic,
        NO_CAPS,
    );
    assert_eq!(
        out.get("thinking"),
        Some(&serde_json::json!({"type": "disabled"}))
    );
    assert!(!out.contains_key("output_config"));
    assert!(!out.contains_key("reasoningEffort"));
    assert!(!out.contains_key("reasoningSummary"));
    assert!(!out.contains_key("thinkingConfig"));
}

#[test]
fn matrix_anthropic_auto_with_adaptive_capability_emits_adaptive() {
    // Adaptive-capable Anthropic model + Auto → emit `{type:adaptive}`.
    let out = to_extra_body(
        &level_with_effort(ReasoningEffort::Auto),
        ProviderApi::Anthropic,
        ADAPTIVE,
    );
    assert_eq!(
        out.get("thinking"),
        Some(&serde_json::json!({"type": "adaptive"}))
    );
    assert!(!out.contains_key("output_config"));
}

#[test]
fn matrix_anthropic_auto_without_adaptive_capability_emits_nothing() {
    // Non-adaptive Anthropic model + Auto → emit NOTHING. Server-side
    // default applies. Protects callers from `--thinking auto` against
    // models that would reject the value (e.g. Claude Sonnet 4.5).
    let out = to_extra_body(
        &level_with_effort(ReasoningEffort::Auto),
        ProviderApi::Anthropic,
        // Has ExtendedThinking but NOT AdaptiveThinking.
        &[Capability::ExtendedThinking],
    );
    assert!(
        out.is_empty(),
        "Auto on non-adaptive Anthropic must emit nothing — got {out:?}"
    );
}

#[test]
fn matrix_anthropic_auto_with_empty_capabilities_emits_nothing() {
    // Defensive: ModelInfo.capabilities = None falls back to
    // `&[]` at the call site; verify Auto still degrades gracefully.
    let out = to_extra_body(
        &level_with_effort(ReasoningEffort::Auto),
        ProviderApi::Anthropic,
        NO_CAPS,
    );
    assert!(out.is_empty());
}

#[test]
fn matrix_anthropic_minimal_emits_enabled_thinking_and_low_effort() {
    let out = to_extra_body(
        &level_with_effort(ReasoningEffort::Minimal),
        ProviderApi::Anthropic,
        NO_CAPS,
    );
    assert_eq!(
        out.get("thinking"),
        Some(&serde_json::json!({"type": "enabled"}))
    );
    assert_eq!(
        out.get("output_config"),
        Some(&serde_json::json!({"effort": "low"})),
    );
}

#[test]
fn matrix_anthropic_low_emits_enabled_thinking_and_low_effort() {
    let out = to_extra_body(
        &level_with_effort(ReasoningEffort::Low),
        ProviderApi::Anthropic,
        NO_CAPS,
    );
    assert_eq!(
        out.get("thinking"),
        Some(&serde_json::json!({"type": "enabled"}))
    );
    assert_eq!(
        out.get("output_config"),
        Some(&serde_json::json!({"effort": "low"})),
    );
}

#[test]
fn matrix_anthropic_medium_emits_enabled_thinking_and_medium_effort() {
    let out = to_extra_body(
        &level_with_effort(ReasoningEffort::Medium),
        ProviderApi::Anthropic,
        NO_CAPS,
    );
    assert_eq!(
        out.get("thinking"),
        Some(&serde_json::json!({"type": "enabled"}))
    );
    assert_eq!(
        out.get("output_config"),
        Some(&serde_json::json!({"effort": "medium"})),
    );
}

#[test]
fn matrix_anthropic_high_emits_enabled_thinking_and_high_effort() {
    let out = to_extra_body(
        &level_with_effort(ReasoningEffort::High),
        ProviderApi::Anthropic,
        NO_CAPS,
    );
    assert_eq!(
        out.get("thinking"),
        Some(&serde_json::json!({"type": "enabled"}))
    );
    assert_eq!(
        out.get("output_config"),
        Some(&serde_json::json!({"effort": "high"})),
    );
}

#[test]
fn matrix_anthropic_xhigh_emits_enabled_thinking_and_max_effort() {
    let out = to_extra_body(
        &level_with_effort(ReasoningEffort::XHigh),
        ProviderApi::Anthropic,
        NO_CAPS,
    );
    assert_eq!(
        out.get("thinking"),
        Some(&serde_json::json!({"type": "enabled"}))
    );
    assert_eq!(
        out.get("output_config"),
        Some(&serde_json::json!({"effort": "max"})),
    );
}

#[test]
fn matrix_anthropic_explicit_levels_ignore_adaptive_capability() {
    // Capability gate is Auto-only; explicit levels emit the same
    // shape regardless of AdaptiveThinking presence. Locks the
    // intended scope of the gate.
    for effort in [
        ReasoningEffort::Minimal,
        ReasoningEffort::Low,
        ReasoningEffort::Medium,
        ReasoningEffort::High,
        ReasoningEffort::XHigh,
    ] {
        let with = to_extra_body(&level_with_effort(effort), ProviderApi::Anthropic, ADAPTIVE);
        let without = to_extra_body(&level_with_effort(effort), ProviderApi::Anthropic, NO_CAPS);
        assert_eq!(
            with, without,
            "{effort:?} wire shape must not depend on AdaptiveThinking",
        );
    }
}

#[test]
fn matrix_openai_disable_emits_nothing() {
    let out = to_extra_body(
        &level_with_effort(ReasoningEffort::Off),
        ProviderApi::Openai,
        NO_CAPS,
    );
    assert!(
        out.is_empty(),
        "OpenAI Disable: server default applies, no typed emission"
    );
}

#[test]
fn matrix_openai_auto_emits_reasoning_summary() {
    // Auto is the builtin default thinking level for every gpt-5 model.
    // OpenAI must still request the reasoning summary (independent of
    // effort) so the UI can render "thinking" — only the effort hint is
    // left to the server default. Regression guard for the bug where
    // `is_explicit_level()` suppressed the summary on the default path.
    let out = to_extra_body(
        &level_with_effort(ReasoningEffort::Auto),
        ProviderApi::Openai,
        NO_CAPS,
    );
    assert_eq!(
        out.get("reasoningSummary")
            .and_then(serde_json::Value::as_str),
        Some("auto"),
        "OpenAI Auto must emit reasoningSummary=auto",
    );
    // Effort itself is NOT emitted here — it rides the typed `reasoning`
    // channel, and Auto leaves it unset (server default).
    assert!(!out.contains_key("thinking"));
    assert!(!out.contains_key("output_config"));
    assert!(!out.contains_key("reasoningEffort"));
}

#[test]
fn matrix_openai_explicit_levels_emit_reasoning_summary() {
    for effort in [
        ReasoningEffort::Minimal,
        ReasoningEffort::Low,
        ReasoningEffort::Medium,
        ReasoningEffort::High,
        ReasoningEffort::XHigh,
    ] {
        let out = to_extra_body(&level_with_effort(effort), ProviderApi::Openai, NO_CAPS);
        assert_eq!(
            out.get("reasoningSummary")
                .and_then(serde_json::Value::as_str),
            Some("auto"),
            "OpenAI {effort:?} must emit reasoningSummary=auto",
        );
        assert!(!out.contains_key("thinking"));
        assert!(!out.contains_key("output_config"));
    }
}

#[test]
fn matrix_gemini_disable_emits_nothing() {
    let out = to_extra_body(
        &level_with_effort(ReasoningEffort::Off),
        ProviderApi::Gemini,
        NO_CAPS,
    );
    assert!(out.is_empty());
}

#[test]
fn matrix_gemini_auto_emits_nothing() {
    let out = to_extra_body(
        &level_with_effort(ReasoningEffort::Auto),
        ProviderApi::Gemini,
        NO_CAPS,
    );
    assert!(out.is_empty());
}

#[test]
fn matrix_gemini_explicit_levels_emit_thinking_config() {
    for effort in [
        ReasoningEffort::Minimal,
        ReasoningEffort::Low,
        ReasoningEffort::Medium,
        ReasoningEffort::High,
        ReasoningEffort::XHigh,
    ] {
        let out = to_extra_body(&level_with_effort(effort), ProviderApi::Gemini, NO_CAPS);
        // Nested under generationConfig — adapter deep-merges this onto
        // the wire body, so the nesting must match Gemini's wire shape.
        assert_eq!(
            out.get("generationConfig")
                .and_then(|v| v.get("thinkingConfig"))
                .and_then(|v| v.get("includeThoughts"))
                .and_then(serde_json::Value::as_bool),
            Some(true),
            "Gemini {effort:?} must emit generationConfig.thinkingConfig.includeThoughts=true",
        );
        assert!(!out.contains_key("thinkingConfig"));
        assert!(!out.contains_key("thinking"));
        assert!(!out.contains_key("output_config"));
    }
}

#[test]
fn matrix_openai_compat_disable_emits_nothing() {
    let out = to_extra_body(
        &level_with_effort(ReasoningEffort::Off),
        ProviderApi::OpenaiCompat,
        NO_CAPS,
    );
    assert!(out.is_empty());
}

#[test]
fn matrix_openai_compat_auto_emits_nothing() {
    // OpenaiCompat (DeepSeek-openai path) ignores AdaptiveThinking
    // capability — the gate is Anthropic-arm-only. Verify the same
    // empty-emit behavior whether or not the capability is declared.
    let with = to_extra_body(
        &level_with_effort(ReasoningEffort::Auto),
        ProviderApi::OpenaiCompat,
        ADAPTIVE,
    );
    let without = to_extra_body(
        &level_with_effort(ReasoningEffort::Auto),
        ProviderApi::OpenaiCompat,
        NO_CAPS,
    );
    assert!(with.is_empty());
    assert!(without.is_empty());
}

#[test]
fn matrix_openai_compat_explicit_levels_emit_reasoning_effort() {
    let cases = [
        (ReasoningEffort::Minimal, "minimal"),
        (ReasoningEffort::Low, "low"),
        (ReasoningEffort::Medium, "medium"),
        (ReasoningEffort::High, "high"),
        (ReasoningEffort::XHigh, "xhigh"),
    ];
    for (effort, wire) in cases {
        let out = to_extra_body(
            &level_with_effort(effort),
            ProviderApi::OpenaiCompat,
            NO_CAPS,
        );
        assert_eq!(
            out.get("reasoningEffort")
                .and_then(serde_json::Value::as_str),
            Some(wire),
            "OpenaiCompat {effort:?} must emit reasoningEffort={wire}",
        );
        assert!(!out.contains_key("thinking"));
        assert!(!out.contains_key("output_config"));
    }
}

#[test]
fn matrix_volcengine_explicit_levels_emit_reasoning_effort() {
    // Volcengine shares the OpenaiCompat code path; verify the wire shape
    // matches.
    for effort in [
        ReasoningEffort::Minimal,
        ReasoningEffort::Low,
        ReasoningEffort::Medium,
        ReasoningEffort::High,
        ReasoningEffort::XHigh,
    ] {
        let out = to_extra_body(&level_with_effort(effort), ProviderApi::Volcengine, NO_CAPS);
        assert!(
            out.contains_key("reasoningEffort"),
            "Volcengine {effort:?} must emit reasoningEffort",
        );
    }
    assert!(
        to_extra_body(
            &level_with_effort(ReasoningEffort::Off),
            ProviderApi::Volcengine,
            NO_CAPS,
        )
        .is_empty()
    );
    assert!(
        to_extra_body(
            &level_with_effort(ReasoningEffort::Auto),
            ProviderApi::Volcengine,
            NO_CAPS,
        )
        .is_empty()
    );
}

#[test]
fn matrix_zai_explicit_levels_emit_reasoning_effort() {
    // Zai shares the OpenaiCompat code path; verify the wire shape
    // matches.
    for effort in [
        ReasoningEffort::Minimal,
        ReasoningEffort::Low,
        ReasoningEffort::Medium,
        ReasoningEffort::High,
        ReasoningEffort::XHigh,
    ] {
        let out = to_extra_body(&level_with_effort(effort), ProviderApi::Zai, NO_CAPS);
        assert!(
            out.contains_key("reasoningEffort"),
            "Zai {effort:?} must emit reasoningEffort",
        );
    }
    assert!(
        to_extra_body(
            &level_with_effort(ReasoningEffort::Off),
            ProviderApi::Zai,
            NO_CAPS,
        )
        .is_empty()
    );
    assert!(
        to_extra_body(
            &level_with_effort(ReasoningEffort::Auto),
            ProviderApi::Zai,
            NO_CAPS,
        )
        .is_empty()
    );
}

#[test]
fn matrix_anthropic_disable_with_options_pass_through_overwritten_by_typed_arm() {
    // Even when `level.options` declares a competing `thinking` shape,
    // the Anthropic typed arm has the final say. Disable wins
    // because the inner match runs after the pass-through loop.
    let mut level = level_with_effort(ReasoningEffort::Off);
    level
        .options
        .insert("thinking".into(), serde_json::json!({"type": "enabled"}));
    let out = to_extra_body(&level, ProviderApi::Anthropic, ADAPTIVE);
    assert_eq!(
        out.get("thinking"),
        Some(&serde_json::json!({"type": "disabled"})),
        "Anthropic typed-arm Disable must override pass-through {{type: enabled}}",
    );
}

#[test]
fn matrix_anthropic_explicit_level_with_budget_emits_camel_case_budget_tokens() {
    // Lock the budget plumbing for every explicit level.
    for (effort, wire) in [
        (ReasoningEffort::Minimal, "low"),
        (ReasoningEffort::Low, "low"),
        (ReasoningEffort::Medium, "medium"),
        (ReasoningEffort::High, "high"),
        (ReasoningEffort::XHigh, "max"),
    ] {
        let level = ThinkingLevel::with_budget(effort, 12_345);
        let out = to_extra_body(&level, ProviderApi::Anthropic, NO_CAPS);
        let thinking = out.get("thinking").unwrap();
        assert_eq!(
            thinking.get("type").and_then(serde_json::Value::as_str),
            Some("enabled"),
        );
        assert_eq!(
            thinking
                .get("budgetTokens")
                .and_then(serde_json::Value::as_i64),
            Some(12_345),
            "{effort:?} with budget must emit camelCase budgetTokens=12345",
        );
        assert!(thinking.get("budget_tokens").is_none());
        assert_eq!(
            out.get("output_config")
                .and_then(|v| v.get("effort"))
                .and_then(serde_json::Value::as_str),
            Some(wire),
        );
    }
}
