//! `ThinkingLevel` → flat camelCase `extra_body` keys.
//!
//! Output is a `BTreeMap<String, JSONValue>` — provider-aware mapping
//! that produces the same shape a user would write directly into
//! `ModelInfo.extra_body`. Layer 2 (`build_call_options`) merges this
//! into the per-call `extra_body` and wraps under the SDK namespace
//! key.
//!
//! All keys are camelCase so they match each provider's typed-options
//! struct (`#[serde(rename_all = "camelCase")]` on
//! `AnthropicProviderOptions`, `OpenAIResponsesProviderOptions`,
//! `GoogleLanguageModelOptions`, etc.).
//!
//! **Routing key is `ProviderApi`, not the runtime instance name.**
//! A `ProviderConfig.name = "azure-east"` backed by
//! `api: ProviderApi::Anthropic` still emits Anthropic's
//! `thinking { type, budgetTokens }` shape — the family is what
//! determines wire body, not the user-facing instance label.

use coco_types::Capability;
use coco_types::ProviderApi;
use coco_types::ReasoningEffort;
use coco_types::ThinkingLevel;
use std::collections::BTreeMap;

/// Convert a `ThinkingLevel` into provider-neutral flat camelCase
/// keys. Routing key is the wire-protocol family.
///
/// `level.options` is passed through unconditionally so models can
/// declare provider-specific wire shapes (e.g. DeepSeek's
/// `{"thinking":{"type":"disabled"}}` toggle).
///
/// The `ProviderApi::Anthropic` arm has full coverage of
/// `ReasoningEffort` via an exhaustive inner match — there is no
/// fallthrough that could emit `{"type":"enabled"}` for `Disable`/`Auto`:
///   - `Disable` → `{"thinking":{"type":"disabled"}}`
///   - `Auto`    → `{"thinking":{"type":"adaptive"}}` **only when
///     `capabilities` contains [`Capability::AdaptiveThinking`]**.
///     Without the capability, the field is omitted so the server-side
///     default applies — protects non-adaptive Claude models (e.g.
///     Sonnet 4.5) from receiving a value they would reject with 400.
///   - `Minimal` → mapped to `Low`
///   - `Low/Medium/High/XHigh` → both
///     `{"thinking":{"type":"enabled","budgetTokens"?}}` and
///     `{"output_config":{"effort":<wire>}}`
///
/// The `output_config` write goes through raw shallow-merge — it does
/// NOT set `AnthropicProviderOptions.effort`, so the Anthropic-specific
/// `effort-2025-11-24` beta header is not added. Callers wanting that
/// beta opt in by setting `provider_options["anthropic"]["effort"]`.
///
/// The `Openai` arm emits `reasoningSummary: "auto"` for `Auto` and
/// every explicit level (only `Off` is skipped) — requesting the
/// reasoning summary is independent of the effort level, matching
/// codex's `summary.unwrap_or(Auto)` default. Without it, gpt-5 models
/// (whose builtin `default_thinking_level` is `Auto`) return reasoning
/// items with an empty summary and the stream carries no thinking text.
///
/// The remaining arms (Gemini/OpenaiCompat/Volcengine/Zai) gate on
/// `is_explicit_level()` — `Disable`/`Auto` emit nothing typed for them
/// (server default applies; `level.options` pass-through is preserved).
/// `capabilities` is consulted only by the Anthropic arm today.
pub fn to_extra_body(
    level: &ThinkingLevel,
    api: ProviderApi,
    capabilities: &[Capability],
) -> BTreeMap<String, serde_json::Value> {
    let mut out = BTreeMap::new();

    // Pass `level.options` through unconditionally so models can declare
    // a wire toggle for the disabled state. Typed-arm emission below
    // overwrites overlapping keys when the arm produces a definitive
    // shape — current order matches existing Claude builtin behavior.
    for (key, value) in &level.options {
        out.insert(key.clone(), value.clone());
    }

    match api {
        ProviderApi::Anthropic => {
            // Exhaustive on ReasoningEffort: the wire `thinking.type`
            // (and presence of `output_config`) is computed from
            // `level.effort` directly. Adding a new effort variant
            // forces this match to be updated — there is no path
            // that can silently emit `{"type":"enabled"}` for
            // Disable/Auto.
            match level.effort {
                ReasoningEffort::Off => {
                    out.insert("thinking".into(), serde_json::json!({"type": "disabled"}));
                }
                ReasoningEffort::Auto => {
                    // Adaptive thinking is gated on a model capability:
                    // emit `{type:adaptive}` only when the registry
                    // declares the model supports it. Otherwise, fall
                    // through silently — the wire body carries no
                    // `thinking` field and the server-side default
                    // applies. Prevents 400 errors when callers run
                    // `--thinking auto` against a non-adaptive Claude
                    // model.
                    if capabilities.contains(&Capability::AdaptiveThinking) {
                        out.insert("thinking".into(), serde_json::json!({"type": "adaptive"}));
                    }
                }
                ReasoningEffort::Minimal
                | ReasoningEffort::Low
                | ReasoningEffort::Medium
                | ReasoningEffort::High
                | ReasoningEffort::XHigh => {
                    // Legacy thinking object — kept for back-compat
                    // with pre-output_config Anthropic API; budgetTokens
                    // honored only when ModelInfo declares one.
                    let mut thinking = serde_json::Map::new();
                    thinking.insert("type".into(), serde_json::Value::String("enabled".into()));
                    if let Some(budget) = level.budget_tokens {
                        thinking.insert(
                            "budgetTokens".into(),
                            serde_json::Value::Number(budget.into()),
                        );
                    }
                    out.insert("thinking".into(), serde_json::Value::Object(thinking));

                    // output_config.effort (new Anthropic API surface).
                    // Goes via raw shallow-merge to avoid the
                    // `effort-2025-11-24` beta header — DeepSeek
                    // anthropic-compat doesn't accept it. Minimal has
                    // no Anthropic equivalent, so it collapses to Low.
                    let wire_effort = match level.effort {
                        ReasoningEffort::Minimal | ReasoningEffort::Low => "low",
                        ReasoningEffort::Medium => "medium",
                        ReasoningEffort::High => "high",
                        ReasoningEffort::XHigh => "max",
                        ReasoningEffort::Off | ReasoningEffort::Auto => unreachable!(),
                    };
                    out.insert(
                        "output_config".into(),
                        serde_json::json!({"effort": wire_effort}),
                    );
                }
            }
        }
        ProviderApi::Openai => {
            // OpenAI Responses: request a human-readable reasoning
            // summary so the UI can render "thinking". This is
            // INDEPENDENT of effort — codex (the authoritative reference)
            // always defaults `summary: auto` for reasoning models
            // regardless of the effort knob
            // (`summary.unwrap_or(ReasoningSummary::Auto)`). Without it,
            // the Responses API returns reasoning items with an empty
            // summary array and the stream carries no reasoning deltas
            // (observed as `reasoning_chars=0`).
            //
            // Emit for `Auto` + every explicit level; skip only for
            // `Off` (reasoning disabled → nothing to summarize). The
            // effort itself rides the typed `reasoning` field on
            // `LanguageModelV4CallOptions`, so `Auto` still sends no
            // effort hint (server default) — only the summary request.
            //
            // The wire body only writes `reasoning.summary` when the
            // model is a reasoning model (`is_reasoning_model` gate in
            // the Responses language model), so a stray key on a
            // non-reasoning model is a harmless no-op.
            if level.effort != ReasoningEffort::Off {
                out.insert(
                    "reasoningSummary".into(),
                    serde_json::Value::String("auto".into()),
                );
            }
        }
        ProviderApi::Gemini => {
            // Effort → wire shape (Gemini-3 `thinkingLevel` vs
            // Gemini-2.5 `thinkingBudget`) is **version-aware** and
            // lives in the adapter (`resolve_thinking_config` in
            // `google_generative_ai_language_model.rs`), driven off
            // the typed `call.reasoning` channel. This convert layer
            // cannot see `model_id`, so it would have to guess which
            // knob to emit. Emitting `thinkingBudget` here
            // unconditionally produced a key Gemini-3 rejects (it
            // dropped `thinkingBudget` support in favor of
            // `thinkingLevel`), and double-wrote with the typed path
            // under deep merge.
            //
            // The only Gemini-specific signal that does NOT have a
            // typed channel is `includeThoughts: true` — without it
            // Gemini's stream omits reasoning deltas. Emit just that
            // here, nested correctly so deep-merge composes with the
            // adapter's typed write at
            // `generationConfig.thinkingConfig.{thinkingLevel,thinkingBudget}`:
            //
            //     { "generationConfig": {
            //         "thinkingConfig": { "includeThoughts": true }
            //     } }
            if level.effort.is_explicit_level() {
                let mut thinking_config = serde_json::Map::new();
                thinking_config.insert("includeThoughts".into(), serde_json::Value::Bool(true));
                let mut generation_config = serde_json::Map::new();
                generation_config.insert(
                    "thinkingConfig".into(),
                    serde_json::Value::Object(thinking_config),
                );
                out.insert(
                    "generationConfig".into(),
                    serde_json::Value::Object(generation_config),
                );
            }
        }
        ProviderApi::Volcengine | ProviderApi::Zai | ProviderApi::OpenaiCompat => {
            // xAI / DeepSeek / Volcengine / Z.AI / generic compat:
            // { "reasoningEffort": "<level>" }. Disable/Auto: only
            // level.options pass-through (e.g. DeepSeek's wire toggle).
            if level.effort.is_explicit_level() {
                out.insert(
                    "reasoningEffort".into(),
                    serde_json::Value::String(level.effort.to_string()),
                );
            }
        }
    }

    out
}

#[cfg(test)]
#[path = "thinking_convert.test.rs"]
mod tests;
